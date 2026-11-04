//! Grabs for a question being typed in the panel: they wait with the draft
//! and go with it when it is sent. Marks on them are numbered across the
//! draft, timed from its first grab.

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::Mutex;
use std::time::Instant;
use tauri::{AppHandle, Manager};
use tauri_specta::Event as _;

use super::capture::{Capture, CaptureView};
use super::timeline::What;
use super::voice;
use super::PANEL_LABEL;

struct Draft {
    /// The first grab: zero on the draft's clock.
    started: Instant,
    captures: Vec<Capture>,
}

static DRAFT: Lazy<Mutex<Option<Draft>>> = Lazy::new(|| Mutex::new(None));

/// The grabs waiting with the panel's draft.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct AskDraftEvent {
    pub captures: Vec<CaptureView>,
}

fn views() -> Vec<CaptureView> {
    DRAFT.lock().ok().map_or_else(Vec::new, |slot| {
        slot.as_ref()
            .map(|d| d.captures.iter().map(CaptureView::from).collect())
            .unwrap_or_default()
    })
}

fn show(app: &AppHandle) {
    let _ = AskDraftEvent { captures: views() }.emit_to(app, PANEL_LABEL);
}

/// The panel has the keyboard: a question is being typed there.
pub fn panel_focused(app: &AppHandle) -> bool {
    app.get_webview_window(PANEL_LABEL)
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false)
}

/// Gives the panel the keyboard back after a grab, if it is showing.
pub fn refocus_panel(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(PANEL_LABEL) {
            if window.is_visible().unwrap_or(false) {
                let _ = window.set_focus();
            }
        }
    });
}

/// What the panel shows, for a page that loads late.
pub fn captures() -> Vec<CaptureView> {
    views()
}

/// A grab for the draft was added.
pub fn add(app: &AppHandle, mut capture: Capture) {
    if let Ok(mut slot) = DRAFT.lock() {
        let draft = slot.get_or_insert_with(|| Draft {
            started: Instant::now(),
            captures: Vec::new(),
        });
        capture.n = draft.captures.last().map_or(0, |c| c.n) + 1;
        capture.at = draft.started.elapsed().as_millis() as u64;
        let marks = std::mem::take(&mut capture.marks);
        capture.marks = voice::on_clock(marks, capture.at, &capture.name());
        draft.captures.push(capture);
    }
    show(app);
}

/// Grab `n` was taken off the draft.
pub fn remove(app: &AppHandle, n: u32) {
    if let Ok(mut slot) = DRAFT.lock() {
        if let Some(draft) = slot.as_mut() {
            if let Some(i) = draft.captures.iter().position(|c| c.n == n) {
                let capture = draft.captures.remove(i);
                let _ = std::fs::remove_file(capture.path);
            }
            if draft.captures.is_empty() {
                *slot = None;
            }
        }
    }
    show(app);
}

/// The highest mark number on the draft's grabs so far.
pub fn marks_so_far() -> u32 {
    DRAFT.lock().ok().map_or(0, |slot| {
        slot.as_ref().map_or(0, |d| {
            d.captures
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

/// The draft's grabs, as it is sent.
pub fn take(app: &AppHandle) -> Vec<Capture> {
    let captures = DRAFT
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
        .map(|d| d.captures)
        .unwrap_or_default();
    if !captures.is_empty() {
        show(app);
    }
    captures
}
