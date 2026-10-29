//! Read aloud: speaks text with a local voice model (Supertonic 3).
//!
//! Text arrives from the read-aloud shortcut, a meeting's Listen buttons or
//! the command line (`handy --speak`), is cleaned up for listening
//! ([`text`]), and waits in a queue. One thread renders it a sentence at a
//! time in the speech worker ([`worker`]) and plays each sentence while the
//! next renders. Stopping ends the playback at once and clears the queue.

pub mod commands;
pub mod model;
pub mod pill;
pub mod selection;
pub mod spool;
pub mod text;
pub mod worker;

use log::{info, warn};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::VecDeque;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_specta::Event;

use crate::settings::get_settings;

/// How often playback checks for a stop while it plays out.
const POLL: Duration = Duration::from_millis(40);

/// Why something wasn't read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SpeakRefusal {
    /// The voice model hasn't been downloaded.
    NotInstalled,
    /// A meeting is recording; the call audio would record the speech.
    QuietInMeeting,
    /// Nothing was left to read after the clean-up.
    NothingToRead,
}

/// Whether Handy is speaking, for the speaking pill and the tray.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct SpeechStateEvent {
    pub speaking: bool,
}

struct Request {
    pieces: Vec<String>,
}

pub struct SpeechManager {
    app: AppHandle,
    queue: Mutex<VecDeque<Request>>,
    wake: Condvar,
    /// Bumped by every stop: work started before it is dropped.
    generation: AtomicU64,
    speaking: AtomicBool,
    /// What's playing now, so a stop can silence it straight away.
    player: Mutex<Option<Arc<rodio::Player>>>,
}

impl SpeechManager {
    pub fn new(app: &AppHandle) -> Arc<Self> {
        model::clear_partial(app);
        let manager = Arc::new(Self {
            app: app.clone(),
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
            generation: AtomicU64::new(0),
            speaking: AtomicBool::new(false),
            player: Mutex::new(None),
        });
        let worker = Arc::clone(&manager);
        std::thread::Builder::new()
            .name("speech".into())
            .spawn(move || worker.run())
            .map_err(|e| warn!("Failed to start the speech thread: {e}"))
            .ok();
        manager
    }

    /// Queues `text` to be read after anything already queued.
    pub fn speak(&self, text: &str) -> Result<(), SpeakRefusal> {
        if !model::model_installed(&self.app) {
            return Err(SpeakRefusal::NotInstalled);
        }
        let settings = get_settings(&self.app).speech;
        if settings.quiet_during_meetings && meeting_recording(&self.app) {
            return Err(SpeakRefusal::QuietInMeeting);
        }
        let pieces = text::prepare(text, settings.max_words.max(1) as usize);
        if pieces.is_empty() {
            return Err(SpeakRefusal::NothingToRead);
        }
        if let Ok(mut queue) = self.queue.lock() {
            queue.push_back(Request { pieces });
        }
        self.wake.notify_all();
        Ok(())
    }

    /// Stops speaking and forgets everything queued.
    pub fn stop(&self) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.clear();
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Some(player) = self.player.lock().ok().and_then(|p| p.clone()) {
            player.stop();
        }
        self.wake.notify_all();
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking.load(Ordering::Acquire)
    }

    /// Counts stops: a caller speaking in parts can tell the user stopped it.
    pub fn stops(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn set_speaking(&self, speaking: bool) {
        if self.speaking.swap(speaking, Ordering::AcqRel) != speaking {
            let _ = SpeechStateEvent { speaking }.emit(&self.app);
            pill::sync(&self.app, speaking);
            crate::tray::update_tray_menu(&self.app);
        }
    }

    fn run(self: Arc<Self>) {
        let mut engine: Option<worker::Engine> = None;
        loop {
            let request = {
                let Ok(mut queue) = self.queue.lock() else {
                    return;
                };
                loop {
                    if let Some(request) = queue.pop_front() {
                        break Some(request);
                    }
                    self.set_speaking(false);
                    let idle = idle_unload(&self.app);
                    if engine.is_some() && idle == Some(Duration::ZERO) {
                        break None;
                    }
                    let waited = match (engine.is_some(), idle) {
                        (true, Some(idle)) => self
                            .wake
                            .wait_timeout(queue, idle)
                            .ok()
                            .map(|(q, t)| (q, t.timed_out())),
                        _ => self.wake.wait(queue).ok().map(|q| (q, false)),
                    };
                    let Some((guard, timed_out)) = waited else {
                        return;
                    };
                    queue = guard;
                    if timed_out && queue.is_empty() {
                        break None;
                    }
                }
            };
            let Some(request) = request else {
                info!("Unloading the voice model");
                engine = None;
                continue;
            };
            self.set_speaking(true);
            let generation = self.generation.load(Ordering::Acquire);
            if let Err(e) = self.say(&mut engine, &request, generation) {
                warn!("Reading aloud failed: {e}");
                // A worker that failed may be in a bad state; start afresh.
                engine = None;
                let _ = self.app.emit("speech-error", e.to_string());
            }
        }
    }

    fn say(
        &self,
        engine: &mut Option<worker::Engine>,
        request: &Request,
        generation: u64,
    ) -> anyhow::Result<()> {
        let stopped = || self.generation.load(Ordering::Acquire) != generation;
        let settings = get_settings(&self.app);
        if engine.is_none() {
            info!("Loading the voice model");
            *engine = Some(worker::Engine::start(&model::model_dir(&self.app)?)?);
        }
        let Some(engine) = engine.as_mut() else {
            return Ok(());
        };
        let rate = NonZero::new(engine.rate).ok_or_else(|| anyhow::anyhow!("No sample rate"))?;
        let voice = settings.speech.voice.min(engine.voices.saturating_sub(1));
        let speed = settings.speech.speed.clamp(0.5, 2.0);

        let output = crate::audio_feedback::open_output(settings.selected_output_device)
            .map_err(|e| anyhow::anyhow!("Can't open the speakers: {e}"))?;
        let player = Arc::new(rodio::Player::connect_new(output.mixer()));
        if let Ok(mut current) = self.player.lock() {
            *current = Some(Arc::clone(&player));
        }
        let result: anyhow::Result<()> = (|| {
            for piece in &request.pieces {
                if stopped() {
                    return Ok(());
                }
                let samples = engine.render(piece, voice, speed)?;
                if stopped() {
                    return Ok(());
                }
                player.append(rodio::buffer::SamplesBuffer::new(
                    NonZero::<u16>::MIN,
                    rate,
                    samples,
                ));
            }
            while !player.empty() && !stopped() {
                std::thread::sleep(POLL);
            }
            Ok(())
        })();
        player.stop();
        if let Ok(mut current) = self.player.lock() {
            *current = None;
        }
        result
    }
}

/// How long the voice model stays loaded with nothing to say, from the
/// same setting as the transcription model. `None` keeps it loaded.
fn idle_unload(app: &AppHandle) -> Option<Duration> {
    get_settings(app)
        .model_unload_timeout
        .to_seconds()
        .map(Duration::from_secs)
}

fn meeting_recording(app: &AppHandle) -> bool {
    crate::meeting::manager(app).is_some_and(|m| m.is_recording())
}

/// The speech manager, once Handy has set it up.
pub fn manager(app: &AppHandle) -> Option<Arc<SpeechManager>> {
    app.try_state::<Arc<SpeechManager>>()
        .map(|s| Arc::clone(s.inner()))
}

/// Reads `text`, telling the user when it can't.
pub fn speak(app: &AppHandle, text: &str) {
    let Some(speech) = manager(app) else { return };
    if let Err(refusal) = speech.speak(text) {
        refused(app, refusal);
    }
}

pub(crate) fn refused(app: &AppHandle, refusal: SpeakRefusal) {
    info!("Not reading aloud: {refusal:?}");
    let _ = SpeechRefusedEvent { refusal }.emit(app);
    pill::notice(app);
}

/// Stops speaking, if Handy is.
pub fn stop(app: &AppHandle) {
    if let Some(speech) = manager(app) {
        speech.stop();
    }
}

/// Something wasn't read, and why.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct SpeechRefusedEvent {
    pub refusal: SpeakRefusal,
}

/// The read-aloud shortcut: reads the selection, or stops if speaking.
pub struct ReadAloudAction;

impl crate::actions::ShortcutAction for ReadAloudAction {
    fn start(&self, app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        if manager(app).is_some_and(|s| s.is_speaking()) {
            stop(app);
            return;
        }
        let app = app.clone();
        // Noted now, while the window read from is still in front.
        let window = crate::ask::window::foreground();
        // Copying the selection waits for the shortcut's keys to be let go.
        std::thread::spawn(move || match selection::copy_selection(&app) {
            Some(text) => {
                // A question asked from this window soon after knows about it.
                if let Some(window) = window {
                    crate::ask::recent::remember(window.key(), &text);
                }
                speak(&app, &text);
            }
            None => refused(&app, SpeakRefusal::NothingToRead),
        });
    }

    fn stop(&self, _app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {}
}
