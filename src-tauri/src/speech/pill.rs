//! The speaking pill: shows while Handy reads aloud, with a stop button,
//! and briefly for a notice such as "select some text first" (a toast in a
//! hidden main window would go unseen). It sits where the dictation overlay
//! does; dictation stops speech, so the two never show together.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Manager};

pub const LABEL: &str = "speech_pill";
const SIZE: (f64, f64) = (300.0, 60.0);
/// How long a notice stays up.
const NOTICE: Duration = Duration::from_secs(3);

/// Bumped by every show, so an old notice's timer can't hide a newer one.
static SHOWN: AtomicU64 = AtomicU64::new(0);

pub fn create(app: &AppHandle) {
    crate::meeting::widgets::build(app, LABEL, "speech", SIZE);
}

/// Shows the pill while speaking and hides it after.
pub fn sync(app: &AppHandle, speaking: bool) {
    SHOWN.fetch_add(1, Ordering::AcqRel);
    if speaking {
        show(app);
    } else {
        hide(app);
    }
}

/// Shows the pill for a notice, then hides it unless speaking began.
pub fn notice(app: &AppHandle) {
    let shown = SHOWN.fetch_add(1, Ordering::AcqRel) + 1;
    show(app);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(NOTICE);
        let speaking = super::manager(&app).is_some_and(|s| s.is_speaking());
        if SHOWN.load(Ordering::Acquire) == shown && !speaking {
            hide(&app);
        }
    });
}

fn show(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(LABEL) {
            crate::meeting::widgets::show_where_overlay_shows(&handle, &window, SIZE);
        }
    });
}

fn hide(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(LABEL) {
            let _ = window.hide();
        }
    });
}
