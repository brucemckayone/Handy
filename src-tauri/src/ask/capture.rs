//! Pictures taken while asking: a grab (a region picked and marked up) or a
//! snap (the window under the mouse, as it is). Either key keeps the question
//! recording hands-free, and starts one when none is running.

use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

use super::timeline::What;
use super::{grab, screenshot, voice, window};
use crate::TranscriptionCoordinator;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum CaptureKind {
    Grab,
    Snap,
}

/// A picture taken while asking.
#[derive(Debug, Clone)]
pub struct Capture {
    pub kind: CaptureKind,
    /// Numbered across the question: Grab 1, Snap 2.
    pub n: u32,
    /// The window it shows, e.g. "Terminal".
    pub of: String,
    pub path: PathBuf,
    /// A small PNG as a data URL, for the pill.
    pub thumbnail: String,
    /// When it was taken, on the Ask clock.
    pub at: u64,
    /// Marks drawn on it, each with how long before it was added.
    pub marks: Vec<(u64, What)>,
}

impl Capture {
    /// How Claude knows it: "Grab 1".
    pub fn name(&self) -> String {
        let kind = match self.kind {
            CaptureKind::Grab => "Grab",
            CaptureKind::Snap => "Snap",
        };
        format!("{kind} {}", self.n)
    }
}

/// A capture as the pill shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct CaptureView {
    pub kind: CaptureKind,
    pub n: u32,
    pub of: String,
    pub thumbnail: String,
}

impl From<&Capture> for CaptureView {
    fn from(c: &Capture) -> Self {
        CaptureView {
            kind: c.kind,
            n: c.n,
            of: c.of.clone(),
            thumbnail: c.thumbnail.clone(),
        }
    }
}

/// A capture key went down: the question goes hands-free (or starts so),
/// then the picture is taken.
pub fn pressed(app: &AppHandle, kind: CaptureKind, hotkey: &str) {
    let Some(coordinator) = app.try_state::<TranscriptionCoordinator>() else {
        log::warn!("TranscriptionCoordinator is not initialized");
        return;
    };
    let app = app.clone();
    coordinator.hands_free("ask", hotkey, move |asking| {
        if asking {
            take(&app, kind);
        }
    });
}

/// Takes the picture, off the coordinator's thread.
fn take(app: &AppHandle, kind: CaptureKind) {
    match kind {
        CaptureKind::Snap => {
            let app = app.clone();
            std::thread::spawn(move || snap(&app));
        }
        CaptureKind::Grab => {
            let app = app.clone();
            std::thread::spawn(move || grab::open(&app));
        }
    }
}

/// The window under the mouse as it is now; the one in front when the mouse
/// is over Handy's own pill.
fn snap(app: &AppHandle) {
    let own = std::process::id();
    let Some(window) =
        window::under_mouse().or_else(|| window::foreground().filter(|w| w.pid != own))
    else {
        log::debug!("Ask: no window to snap");
        return;
    };
    let Some((path, thumbnail)) = screenshot::capture_with_thumbnail(window.handle) else {
        log::warn!("Ask: couldn't snap {}", window.app());
        return;
    };
    voice::add_capture(
        app,
        Capture {
            kind: CaptureKind::Snap,
            n: 0,
            of: window.label(),
            path,
            thumbnail,
            at: 0,
            marks: Vec::new(),
        },
    );
}
