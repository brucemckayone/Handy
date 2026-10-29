//! The speaking pill: shows while Handy reads aloud, with a stop button,
//! and briefly for a notice such as "select some text first" (a toast in a
//! hidden main window would go unseen). It sits where the dictation overlay
//! does; dictation stops speech, so the two never show together.
//!
//! It is also all Ask shows by default: "Thinking…" while a question is
//! answered, then the spoken answer, then "Answered · Open" for a few
//! seconds, its Open button being the way into the Ask panel.

use once_cell::sync::Lazy;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::ask::AskBusyEvent;

pub const LABEL: &str = "speech_pill";
const SIZE: (f64, f64) = (300.0, 60.0);
/// How long a notice stays up.
const NOTICE: Duration = Duration::from_secs(3);
/// How long "Answered · Open" stays up once the answer is done and spoken.
const ANSWERED: Duration = Duration::from_secs(8);

/// Bumped by every show, so an old notice's timer can't hide a newer one.
static SHOWN: AtomicU64 = AtomicU64::new(0);
/// Ask's part in the pill.
static ASK: Lazy<Mutex<AskBusyEvent>> = Lazy::new(|| {
    Mutex::new(AskBusyEvent {
        busy: false,
        thread_id: None,
        failed: false,
    })
});

pub fn create(app: &AppHandle) {
    crate::meeting::widgets::build(app, LABEL, "speech", SIZE);
}

/// Shows the pill while speaking and hides it after.
pub fn sync(app: &AppHandle, speaking: bool) {
    let shown = SHOWN.fetch_add(1, Ordering::AcqRel) + 1;
    let ask = ask_state();
    if speaking || ask.busy {
        show(app);
    } else if ask.thread_id.is_some() {
        show(app);
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(ANSWERED);
            if SHOWN.load(Ordering::Acquire) == shown {
                ask_dismissed(&app);
            }
        });
    } else {
        hide(app);
    }
}

/// What Ask shows in the pill now.
pub fn ask_state() -> AskBusyEvent {
    ASK.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

fn set_ask(app: &AppHandle, state: AskBusyEvent) {
    *ASK.lock().unwrap_or_else(|p| p.into_inner()) = state.clone();
    let _ = state.emit(app);
    let speaking = super::manager(app).is_some_and(|s| s.is_speaking());
    sync(app, speaking);
}

/// A question is being answered in `thread_id`.
pub fn ask_started(app: &AppHandle, thread_id: &str) {
    set_ask(
        app,
        AskBusyEvent {
            busy: true,
            thread_id: Some(thread_id.to_string()),
            failed: false,
        },
    );
}

/// The answer is done (or failed): the pill offers to open it.
pub fn ask_finished(app: &AppHandle, thread_id: &str, failed: bool) {
    set_ask(
        app,
        AskBusyEvent {
            busy: false,
            thread_id: Some(thread_id.to_string()),
            failed,
        },
    );
}

/// The answer was opened or left: the pill goes back to plain speaking.
pub fn ask_dismissed(app: &AppHandle) {
    set_ask(
        app,
        AskBusyEvent {
            busy: false,
            thread_id: None,
            failed: false,
        },
    );
}

/// Shows the pill for a notice, then hides it unless speaking began.
pub fn notice(app: &AppHandle) {
    let shown = SHOWN.fetch_add(1, Ordering::AcqRel) + 1;
    show(app);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(NOTICE);
        let speaking = super::manager(&app).is_some_and(|s| s.is_speaking());
        if SHOWN.load(Ordering::Acquire) == shown && !speaking && !ask_state().busy {
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
