//! A spoken question from the Ask shortcut. On press: note the window in
//! front (and start its screenshot), and watch what is selected and which
//! window is in front while the key is down. Grabs and snaps taken meanwhile
//! wait here too. On release, once transcribed: time the words if anything
//! was done meanwhile, gather what else was on screen, and ask.

use log::{debug, warn};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tauri_specta::Event as _;

use super::agents::{self, Agent};
use super::capture::{Capture, CaptureKind, CaptureView};
use super::ocr;
use super::prompt::{Picture, ScreenContext};
use super::store::Via;
use super::timeline::{Event, Spoken, Timeline, What};
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
    captures: Vec<Capture>,
    /// Esc was pressed once with captures taken; another soon cancels.
    cancel_armed: Option<Instant>,
    /// The capture keys as the pill shows them.
    keys: (Option<String>, Option<String>),
}

/// With captures taken, the second Esc must come within this to cancel.
const CANCEL_AGAIN: Duration = Duration::from_secs(3);

/// What the recording pill shows of the question being asked.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct AskLiveEvent {
    /// A spoken question is being recorded.
    pub asking: bool,
    /// Grabs and snaps so far, in order.
    pub captures: Vec<CaptureView>,
    /// Esc was pressed once: the next one cancels.
    pub cancel_armed: bool,
    /// The keys that grab and snap, e.g. "G", shown with Ctrl+Alt held.
    pub grab_key: Option<String>,
    pub snap_key: Option<String>,
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
    let bindings = crate::settings::get_settings(app).bindings;
    let key = |id: &str| bindings.get(id).and_then(|b| last_key(&b.current_binding));
    let pending = Pending {
        window,
        screenshot,
        agent,
        started,
        watch,
        captures: Vec::new(),
        cancel_armed: None,
        keys: (key("ask_grab"), key("ask_snap")),
    };
    if let Ok(mut slot) = PENDING.lock() {
        // An older press's screenshot is no longer wanted.
        if let Some(old) = slot.replace(pending) {
            discard(old);
        }
        show(app, slot.as_ref());
    }
}

/// The key a binding ends in, as a keycap shows it: "ctrl+alt+g" is "G".
fn last_key(binding: &str) -> Option<String> {
    let key = binding.rsplit('+').next()?.trim();
    let mut chars = key.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

/// Tells the recording pill where the question is.
fn show(app: &AppHandle, pending: Option<&Pending>) {
    let event = pending.map_or_else(AskLiveEvent::default, |p| AskLiveEvent {
        asking: true,
        captures: p.captures.iter().map(CaptureView::from).collect(),
        cancel_armed: p.cancel_armed.is_some_and(|at| at.elapsed() < CANCEL_AGAIN),
        grab_key: p.keys.0.clone(),
        snap_key: p.keys.1.clone(),
    });
    let _ = event.emit(app);
}

fn discard(pending: Pending) {
    if let Some(handle) = pending.screenshot {
        std::thread::spawn(move || {
            if let Ok(Some(path)) = handle.join() {
                let _ = std::fs::remove_file(path);
            }
        });
    }
    for capture in pending.captures {
        let _ = std::fs::remove_file(capture.path);
    }
}

/// A grab or snap was taken: it goes with the question. Numbered and timed
/// here, so the order is the order they arrive in.
pub fn add_capture(app: &AppHandle, mut capture: Capture) {
    let Ok(mut slot) = PENDING.lock() else { return };
    let Some(pending) = slot.as_mut() else {
        // The question ended while it was being taken.
        let _ = std::fs::remove_file(&capture.path);
        return;
    };
    // Numbers are never reused, even after one is taken out.
    capture.n = pending.captures.last().map_or(0, |c| c.n) + 1;
    capture.at = pending.started.elapsed().as_millis() as u64;
    let marks = std::mem::take(&mut capture.marks);
    capture.marks = on_clock(marks, capture.at, &capture.name());
    pending.captures.push(capture);
    show(app, slot.as_ref());
}

/// Marks timed "this long before the picture was added at `added`", on the
/// Ask clock and on the picture called `name`.
fn on_clock(marks: Vec<(u64, What)>, added: u64, name: &str) -> Vec<(u64, What)> {
    marks
        .into_iter()
        .map(|(ago, mut what)| {
            if let What::Mark { on, .. } = &mut what {
                *on = name.to_string();
            }
            (added.saturating_sub(ago), what)
        })
        .collect()
}

/// The highest mark number so far, across the question's pictures: the next
/// mark gets the one after. Numbers drawn into a picture are never reused.
pub fn marks_so_far() -> u32 {
    PENDING.lock().ok().map_or(0, |slot| {
        slot.as_ref().map_or(0, |p| {
            p.captures
                .iter()
                .flat_map(|c| &c.marks)
                .filter_map(|(_, what)| match what {
                    What::Mark { n, .. } => Some(*n),
                    _ => None,
                })
                .max()
                .unwrap_or(0)
        })
    })
}

/// Capture `n`, to mark it up again: its kind, picture and what it shows.
pub fn capture(n: u32) -> Option<(CaptureKind, PathBuf, String)> {
    let slot = PENDING.lock().ok()?;
    let capture = slot.as_ref()?.captures.iter().find(|c| c.n == n)?;
    Some((capture.kind, capture.path.clone(), capture.of.clone()))
}

/// Capture `n` was marked up again: its picture becomes `path` and the new
/// marks join its old ones.
pub fn marked_up(
    app: &AppHandle,
    n: u32,
    path: PathBuf,
    thumbnail: String,
    marks: Vec<(u64, What)>,
) {
    let Ok(mut slot) = PENDING.lock() else { return };
    let Some(pending) = slot.as_mut() else {
        let _ = std::fs::remove_file(&path);
        return;
    };
    let now = pending.started.elapsed().as_millis() as u64;
    let Some(capture) = pending.captures.iter_mut().find(|c| c.n == n) else {
        let _ = std::fs::remove_file(&path);
        return;
    };
    let name = capture.name();
    capture.marks.extend(on_clock(marks, now, &name));
    let old = std::mem::replace(&mut capture.path, path);
    let _ = std::fs::remove_file(old);
    capture.thumbnail = thumbnail;
    show(app, slot.as_ref());
}

/// Capture `n` is taken out of the question.
pub fn remove_capture(app: &AppHandle, n: u32) {
    let Ok(mut slot) = PENDING.lock() else { return };
    let Some(pending) = slot.as_mut() else { return };
    if let Some(i) = pending.captures.iter().position(|c| c.n == n) {
        let capture = pending.captures.remove(i);
        let _ = std::fs::remove_file(capture.path);
        show(app, slot.as_ref());
    }
}

/// Esc while asking. With nothing taken it cancels at once; with grabs or
/// snaps it takes a second Esc, so one slip doesn't lose them. Returns
/// whether to cancel now.
pub fn cancel_pressed(app: &AppHandle) -> bool {
    let Ok(mut slot) = PENDING.lock() else {
        return true;
    };
    let Some(pending) = slot.as_mut() else {
        return true;
    };
    if pending.captures.is_empty()
        || pending
            .cancel_armed
            .is_some_and(|at| at.elapsed() < CANCEL_AGAIN)
    {
        return true;
    }
    pending.cancel_armed = Some(Instant::now());
    show(app, slot.as_ref());
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(CANCEL_AGAIN);
        if let Ok(slot) = PENDING.lock() {
            if slot.is_some() {
                show(&app, slot.as_ref());
            }
        }
    });
    false
}

/// The question was cancelled, or came to nothing: what was taken for it goes.
pub fn cancelled(app: &AppHandle) {
    let pending = PENDING.lock().ok().and_then(|mut slot| slot.take());
    if let Some(pending) = pending {
        if let Some(watch) = &pending.watch {
            watch.stop();
        }
        discard(pending);
        show(app, None);
    }
    super::grab::close(app);
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
        show(&app, None);
        super::grab::close(&app);
        let (window, screenshot, agent, started, mut watched, captures) = match pending {
            Some(p) => (
                p.window,
                p.screenshot.and_then(|h| h.join().ok().flatten()),
                p.agent.and_then(|h| h.join().ok().flatten()),
                p.started,
                p.watch.map(Watch::finish).unwrap_or_default(),
                p.captures,
            ),
            None => (
                None,
                None,
                None,
                Instant::now(),
                Watched::default(),
                Vec::new(),
            ),
        };
        // Pictures picked by hand say more than the window's own screenshot.
        let screenshot = screenshot.filter(|path| {
            let keep = captures.is_empty();
            if !keep {
                let _ = std::fs::remove_file(path);
            }
            keep
        });
        // What each mark encloses, read off its picture, all at once.
        let reads: Vec<_> = captures
            .iter()
            .filter(|c| !c.marks.is_empty())
            .map(|c| {
                let path = c.path.clone();
                (c.n, std::thread::spawn(move || ocr::read_png(&path)))
            })
            .collect();
        let words: HashMap<u32, Vec<ocr::Word>> = reads
            .into_iter()
            .filter_map(|(n, read)| read.join().ok().map(|words| (n, words)))
            .collect();
        for capture in &captures {
            watched.events.push(Event {
                at: capture.at,
                what: What::Capture {
                    name: capture.name(),
                    of: capture.of.clone(),
                },
            });
            let words = words.get(&capture.n).map_or(&[][..], Vec::as_slice);
            watched
                .events
                .extend(capture.marks.iter().map(|(at, what)| {
                    let mut what = what.clone();
                    let found = ocr::enclosed(words, &what);
                    if let What::Mark { encloses, .. } = &mut what {
                        *encloses = found.or(encloses.take());
                    }
                    Event { at: *at, what }
                }));
        }
        watched.events.sort_by_key(|e| e.at);
        let pictures = captures
            .into_iter()
            .map(|c| Picture {
                name: c.name(),
                of: c.of,
                path: c.path,
            })
            .collect();
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
                pictures,
                timeline,
            },
            prompt: None,
        };
        super::ask(&app, question);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_go_on_the_ask_clock_and_their_picture() {
        let mark = What::Mark {
            n: 1,
            kind: super::super::timeline::MarkKind::Pen,
            on: String::new(),
            region: None,
            joins: None,
            encloses: None,
            label: None,
        };
        let early = What::Window {
            label: "Terminal".into(),
        };
        let timed = on_clock(vec![(1_500, mark), (9_000, early)], 4_000, "Grab 2");
        assert_eq!(timed[0].0, 2_500);
        assert!(matches!(&timed[0].1, What::Mark { on, .. } if on == "Grab 2"));
        assert_eq!(timed[1].0, 0, "never before the question began");
    }

    #[test]
    fn keycaps_show_the_last_key_of_a_binding() {
        assert_eq!(last_key("ctrl+alt+g").as_deref(), Some("G"));
        assert_eq!(last_key("ctrl+option+s").as_deref(), Some("S"));
        assert_eq!(last_key("ctrl+alt+f9").as_deref(), Some("F9"));
        assert_eq!(last_key(""), None);
    }
}
