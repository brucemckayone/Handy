//! A spoken question from the Ask shortcut. On press: note the window in
//! front (and start its screenshot). On release, once transcribed: copy the
//! selection, gather what else was on screen, and ask.

use log::debug;
use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread::JoinHandle;
use tauri::AppHandle;

use super::agents::{self, Agent};
use super::prompt::ScreenContext;
use super::store::Via;
use super::window::{self, WindowInfo};
use super::NewQuestion;
use crate::speech::selection::CopyKeys;

struct Pending {
    window: Option<WindowInfo>,
    screenshot: Option<JoinHandle<Option<PathBuf>>>,
    /// The Claude Code session running in that window, looked up meanwhile.
    agent: Option<JoinHandle<Option<Agent>>>,
}

/// The latest press, waiting for its transcript.
static PENDING: Lazy<Mutex<Option<Pending>>> = Lazy::new(|| Mutex::new(None));

/// The Ask shortcut went down.
pub fn pressed(app: &AppHandle) {
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
    let pending = Pending {
        window,
        screenshot,
        agent,
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

/// The question was transcribed: gathers the rest and asks it.
pub fn released(app: &AppHandle, text: String) {
    let pending = PENDING.lock().ok().and_then(|mut slot| slot.take());
    let app = app.clone();
    // Copying the selection waits for the shortcut's keys to be let go.
    std::thread::spawn(move || {
        let (window, screenshot, agent) = match pending {
            Some(p) => (
                p.window,
                p.screenshot.and_then(|h| h.join().ok().flatten()),
                p.agent.and_then(|h| h.join().ok().flatten()),
            ),
            None => (None, None, None),
        };
        if let Some(agent) = &agent {
            debug!("Asking about agent {}", agent.name);
        }
        // Never from Handy's own panel; and never with Ctrl+C, which would
        // stop whatever runs in a terminal when nothing is selected.
        let selection = window
            .as_ref()
            .and_then(|_| crate::speech::selection::copy_selection_with(&app, CopyKeys::Insert));
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
            },
            prompt: None,
        };
        super::ask(&app, question);
    });
}
