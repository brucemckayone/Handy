//! A spoken question from the Ask shortcut. On press: note the window in
//! front (and start its screenshot), and watch what is selected and which
//! window is in front while the key is down. On release, once transcribed:
//! time the words if anything was done meanwhile, gather what else was on
//! screen, and ask.

use log::{debug, warn};
use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;
use tauri::{AppHandle, Manager};

use super::agents::{self, Agent};
use super::prompt::ScreenContext;
use super::store::Via;
use super::timeline::{Spoken, Timeline};
use super::watch::{Watch, Watched};
use super::window::{self, WindowInfo};
use super::NewQuestion;
use crate::audio_toolkit::AudioTiming;
use crate::managers::transcription::TranscriptionManager;
use crate::speech::selection::CopyKeys;

struct Pending {
    window: Option<WindowInfo>,
    screenshot: Option<JoinHandle<Option<PathBuf>>>,
    /// The Claude Code session running in that window, looked up meanwhile.
    agent: Option<JoinHandle<Option<Agent>>>,
    /// The key press: zero on the question's clock.
    started: Instant,
    watch: Option<Watch>,
}

/// The recording a spoken question came from, to time its words.
pub struct AskAudio {
    pub samples: Vec<f32>,
    pub timing: AudioTiming,
}

/// The latest press, waiting for its transcript.
static PENDING: Lazy<Mutex<Option<Pending>>> = Lazy::new(|| Mutex::new(None));

/// The Ask shortcut went down.
pub fn pressed(app: &AppHandle) {
    let started = Instant::now();
    // Asked with the panel in front: it goes to the thread shown there.
    let window = window::foreground().filter(|w| w.pid != std::process::id());
    debug!("Ask from {:?}", window.as_ref().map(|w| w.label()));
    let settings = crate::settings::get_settings(app).ask;
    let screenshot = window.as_ref().filter(|_| settings.screenshot).map(|w| {
        let handle = w.handle;
        std::thread::spawn(move || super::screenshot::capture(handle))
    });
    let agent = window.clone().filter(|_| settings.use_agents).map(|w| {
        std::thread::spawn(move || {
            let agents = agents::list();
            agents::pick(&agents, &agents::parents(), &w).cloned()
        })
    });
    let watch = cfg!(windows).then(|| Watch::start(started, window.clone()));
    let pending = Pending {
        window,
        screenshot,
        agent,
        started,
        watch,
    };
    if let Ok(mut slot) = PENDING.lock() {
        // An older press's screenshot is no longer wanted.
        if let Some(old) = slot.replace(pending) {
            discard(old);
        }
    }
}

fn discard(pending: Pending) {
    if let Some(handle) = pending.screenshot {
        std::thread::spawn(move || {
            if let Ok(Some(path)) = handle.join() {
                let _ = std::fs::remove_file(path);
            }
        });
    }
}

/// The Ask key came up: nothing done from now on is part of the question.
pub fn stopped() {
    if let Ok(slot) = PENDING.lock() {
        if let Some(watch) = slot.as_ref().and_then(|p| p.watch.as_ref()) {
            watch.stop();
        }
    }
}

/// The question's words on its clock; none if the model can't time them.
fn spoken(app: &AppHandle, audio: AskAudio, started: Instant) -> Vec<Spoken> {
    let tm = app.state::<Arc<TranscriptionManager>>();
    let words = match tm.word_times(audio.samples) {
        Ok(words) => words,
        Err(e) => {
            warn!("Ask: no word times, so marks go by time alone: {e:#}");
            return Vec::new();
        }
    };
    words
        .into_iter()
        .filter_map(|w| {
            let heard = audio.timing.heard_at(w.t0_ms.max(0) as u64)?;
            let text = w.text.trim().to_string();
            (!text.is_empty()).then(|| Spoken {
                at: heard.saturating_duration_since(started).as_millis() as u64,
                text,
            })
        })
        .collect()
}

/// The question was transcribed: gathers the rest and asks it. `audio` lets
/// its words be timed against what was done while it was asked.
pub fn released(app: &AppHandle, text: String, audio: Option<AskAudio>) {
    let pending = PENDING.lock().ok().and_then(|mut slot| slot.take());
    let app = app.clone();
    // Copying the selection waits for the shortcut's keys to be let go.
    std::thread::spawn(move || {
        let (window, screenshot, agent, started, watched) = match pending {
            Some(p) => (
                p.window,
                p.screenshot.and_then(|h| h.join().ok().flatten()),
                p.agent.and_then(|h| h.join().ok().flatten()),
                p.started,
                p.watch.map(Watch::finish).unwrap_or_default(),
            ),
            None => (None, None, None, Instant::now(), Watched::default()),
        };
        let timeline = (!watched.events.is_empty()).then(|| Timeline {
            words: audio.map(|a| spoken(&app, a, started)).unwrap_or_default(),
            events: watched.events,
        });
        if let Some(agent) = &agent {
            debug!("Asking about agent {}", agent.name);
        }
        // Read as the key went down where the app can say; otherwise copied,
        // never from Handy's own panel, and never with Ctrl+C, which would
        // stop whatever runs in a terminal when nothing is selected.
        let selection = watched.at_press.or_else(|| {
            window
                .as_ref()
                .and_then(|_| crate::speech::selection::copy_selection_with(&app, CopyKeys::Insert))
        });
        let recent_reads = window
            .as_ref()
            .map(|w| super::recent::for_window(&w.key()))
            .unwrap_or_default();
        let thread_id = window
            .is_none()
            .then(|| super::panel::is_open(&app).then(super::panel::opened_thread))
            .flatten()
            .flatten();
        let question = NewQuestion {
            text,
            via: Via::Voice,
            thread_id,
            window: window.as_ref().map(WindowInfo::key),
            app_label: window.as_ref().map(WindowInfo::label),
            project_dir: agent.as_ref().map(|a| PathBuf::from(&a.cwd)),
            agent: agent.as_ref().map(Agent::link),
            screen: ScreenContext {
                app: window
                    .as_ref()
                    .map(|w| w.exe.clone())
                    .filter(|e| !e.is_empty()),
                window_title: window
                    .as_ref()
                    .map(|w| w.title.clone())
                    .filter(|t| !t.trim().is_empty()),
                selection: selection.filter(|s| !s.trim().is_empty()),
                input: None,
                recent_reads,
                screenshot,
                timeline,
            },
            prompt: None,
        };
        super::ask(&app, question);
    });
}
