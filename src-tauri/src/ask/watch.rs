//! What happens on screen while a question is asked: text selected in the
//! app in front, other windows coming to the front, and where the mouse is,
//! for what a "this" points at. Checked every 150 ms
//! on a thread of its own until the Ask key comes up; a Terminal selection
//! reads in a few milliseconds.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::timeline::{Event, What};
use super::uia::{self, Uia};
use super::window::{self, WindowInfo};

const EVERY: Duration = Duration::from_millis(150);
/// A selection counts once it has held still this long: dragging one out
/// changes it on every check.
const SETTLED: Duration = Duration::from_millis(300);
/// An Ask nobody ends stops being watched after this.
const LONGEST: Duration = Duration::from_secs(5 * 60);
const MAX_EVENTS: usize = 40;
/// A focused control showing fewer lines than this is a box, not the
/// window's text: a search field, a single line.
const ENOUGH_LINES: usize = 3;
/// Where the mouse was, for as long as an Ask is watched.
const MAX_POINTER: usize = (LONGEST.as_millis() / EVERY.as_millis()) as usize;

/// What the watch saw.
#[derive(Debug, Default)]
pub struct Watched {
    /// What was selected as the key went down: empty when nothing was,
    /// `None` when the app can't say (then it is copied the old way).
    pub at_press: Option<String>,
    /// The text the app in front was showing as the key went down, where
    /// it can say: a terminal's screen.
    pub text_at_press: Option<String>,
    pub events: Vec<Event>,
    /// Where the mouse was, in screen pixels, on the question's clock.
    pub pointer: Vec<(u64, i32, i32)>,
}

pub struct Watch {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Watched>>,
}

impl Watch {
    /// Watches from `started` (the key press), with `front` in front.
    pub fn start(started: Instant, front: Option<WindowInfo>) -> Watch {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("ask-watch".into())
            .spawn(move || watch(started, front, &flag))
            .ok();
        Watch { stop, thread }
    }

    /// Stops watching without waiting.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    /// Stops and hands back what was seen.
    pub fn finish(mut self) -> Watched {
        self.stop();
        self.thread
            .take()
            .and_then(|t| t.join().ok())
            .unwrap_or_default()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read(uia: &Option<Uia>, w: &WindowInfo) -> Option<String> {
    if uia::is_chromium(w.app()) {
        return None;
    }
    uia.as_ref()?.selection()
}

fn showing(uia: &Option<Uia>, w: &WindowInfo) -> Option<String> {
    if uia::is_chromium(w.app()) {
        return None;
    }
    let text = uia.as_ref()?.visible_text()?;
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    (lines >= ENOUGH_LINES).then_some(text)
}

fn ms_since(started: Instant, at: Instant) -> u64 {
    at.saturating_duration_since(started).as_millis() as u64
}

fn watch(started: Instant, mut front: Option<WindowInfo>, stop: &AtomicBool) -> Watched {
    let uia = Uia::new();
    let own = std::process::id();
    let at_press = front.as_ref().and_then(|w| read(&uia, w));
    let mut watched = Watched {
        at_press: at_press.clone(),
        text_at_press: front.as_ref().and_then(|w| showing(&uia, w)),
        events: Vec::new(),
        pointer: Vec::new(),
    };
    // Already part of the question; only changes from here are news.
    let mut known = at_press.unwrap_or_default();
    let mut seen: Option<(String, Instant)> = None;
    while !stop.load(Ordering::Acquire)
        && started.elapsed() < LONGEST
        && watched.events.len() < MAX_EVENTS
    {
        std::thread::sleep(EVERY);
        if let Some((x, y)) = window::cursor().filter(|_| watched.pointer.len() < MAX_POINTER) {
            watched
                .pointer
                .push((ms_since(started, Instant::now()), x, y));
        }
        if let Some(now) = window::foreground().filter(|w| w.pid != own) {
            if front.as_ref().map(|f| f.handle) != Some(now.handle) {
                watched.events.push(Event {
                    at: ms_since(started, Instant::now()),
                    what: What::Window { label: now.label() },
                });
                known = read(&uia, &now).unwrap_or_default();
                seen = None;
                front = Some(now);
                continue;
            }
        }
        let Some(w) = &front else { continue };
        let Some(text) = read(&uia, w) else { continue };
        if text == known {
            seen = None;
            continue;
        }
        match &seen {
            Some((same, since)) if *same == text => {
                if since.elapsed() >= SETTLED {
                    if !text.trim().is_empty() {
                        watched.events.push(Event {
                            at: ms_since(started, *since),
                            what: What::Selected {
                                text: text.clone(),
                                app: w.app().to_string(),
                            },
                        });
                    }
                    known = text;
                    seen = None;
                }
            }
            _ => seen = Some((text, Instant::now())),
        }
    }
    watched
}
