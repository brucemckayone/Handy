//! The two small windows meetings show outside the main window: the
//! recording pill (bottom centre, while a meeting records) and the call
//! prompt (bottom right, when a call starts or ends).
//!
//! The pill stays up for as long as a meeting records, whatever the overlay
//! setting: people being recorded deserve a recording that is never hidden
//! from the person recording them.

use log::{debug, error};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewWindowBuilder};
use tauri_specta::Event;

use super::detect::CallApp;
use super::RecordingInfo;
use crate::settings::{get_settings, OverlayPosition};

pub const PILL_LABEL: &str = "meeting_pill";
pub const PROMPT_LABEL: &str = "meeting_prompt";

/// Logical sizes; each only has to fit the card it shows.
const PILL_SIZE: (f64, f64) = (440.0, 66.0);
const PROMPT_SIZE: (f64, f64) = (372.0, 250.0);
/// The pill's distance from the screen edge, as the dictation overlay's.
#[cfg(target_os = "macos")]
const PILL_BOTTOM_OFFSET: f64 = 15.0;
#[cfg(not(target_os = "macos"))]
const PILL_BOTTOM_OFFSET: f64 = 40.0;
#[cfg(target_os = "macos")]
const PILL_TOP_OFFSET: f64 = 46.0;
#[cfg(not(target_os = "macos"))]
const PILL_TOP_OFFSET: f64 = 4.0;
/// Space between the prompt and the screen's corner.
const PROMPT_MARGIN: f64 = 12.0;

/// What the call prompt shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PromptState {
    /// A call app started using the mic: record it?
    AskStart { app: CallApp },
    /// Recording, started from the prompt.
    Recording,
    /// The user said never to ask for this app.
    Never { app: CallApp },
    /// The call ended; the meeting stops in `seconds`.
    AskEnd { app: CallApp, seconds: u32 },
    /// The meeting stopped and is being transcribed.
    Saved { minutes: u32 },
    /// The user kept recording after the call seemed to end.
    Kept,
}

/// The prompt's state; `None` hides it.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingPromptEvent {
    pub state: Option<PromptState>,
}

/// A short message on the pill, such as "bookmarked".
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingPillMessageEvent {
    pub message: String,
}

static PROMPT: Lazy<Mutex<Option<PromptState>>> = Lazy::new(|| Mutex::new(None));

/// What the prompt shows now, for a prompt window that just loaded.
pub fn prompt_state() -> Option<PromptState> {
    PROMPT.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// A small always-on-top window showing one view of the widgets page.
pub(crate) fn build(app: &AppHandle, label: &str, view: &str, (width, height): (f64, f64)) {
    let url = format!("src/meeting-widgets/index.html?view={view}");
    let mut builder = WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App(url.into()))
        .title("Handy")
        .resizable(false)
        .inner_size(width, height)
        .shadow(false)
        .maximizable(false)
        .minimizable(false)
        .closable(false)
        .accept_first_mouse(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .transparent(true)
        .focusable(false)
        .focused(false)
        .visible(false);
    if let Some(data_dir) = crate::portable::data_dir() {
        builder = builder.data_directory(data_dir.join("webview"));
    }
    if let Err(e) = builder.build() {
        error!("Failed to create the {} window: {}", label, e);
    }
}

/// Creates both windows, hidden.
pub fn create(app: &AppHandle) {
    build(app, PILL_LABEL, "pill", PILL_SIZE);
    build(app, PROMPT_LABEL, "prompt", PROMPT_SIZE);
}

fn monitor(app: &AppHandle) -> Option<tauri::Monitor> {
    app.primary_monitor().ok().flatten()
}

/// Shows the pill while a meeting records and hides it after.
pub fn sync_pill(app: &AppHandle, recording: Option<&RecordingInfo>) {
    let shown = recording.is_some();
    crate::overlay::set_meeting_pill_shown(shown);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(window) = handle.get_webview_window(PILL_LABEL) else {
            return;
        };
        if !shown {
            let _ = window.hide();
            return;
        }
        show_where_overlay_shows(&handle, &window, PILL_SIZE);
    });
}

/// Shows `window` at `size`, centred where the dictation overlay appears.
/// Call on the main thread.
pub(crate) fn show_where_overlay_shows(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
    size: (f64, f64),
) {
    if let Some(monitor) = monitor(app) {
        let scale = monitor.scale_factor();
        let (w, h) = (
            (size.0 * scale).round() as u32,
            (size.1 * scale).round() as u32,
        );
        let position = *monitor.position();
        let screen = *monitor.size();
        let x = position.x + (screen.width as i32 - w as i32) / 2;
        let y = match get_settings(app).overlay_position {
            OverlayPosition::Top => position.y + (PILL_TOP_OFFSET * scale).round() as i32,
            OverlayPosition::Bottom => {
                position.y + screen.height as i32
                    - h as i32
                    - (PILL_BOTTOM_OFFSET * scale).round() as i32
            }
        };
        let _ = window.set_size(PhysicalSize::new(w, h));
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
    let _ = window.show();
    let _ = window.set_always_on_top(true);
}

pub fn pill_message(app: &AppHandle, message: &str) {
    let _ = MeetingPillMessageEvent {
        message: message.to_string(),
    }
    .emit_to(app, PILL_LABEL);
}

pub fn show_prompt(app: &AppHandle, state: PromptState) {
    debug!("Meeting prompt: {:?}", state);
    *PROMPT.lock().unwrap_or_else(|p| p.into_inner()) = Some(state.clone());
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(window) = handle.get_webview_window(PROMPT_LABEL) else {
            return;
        };
        if let Some(monitor) = monitor(&handle) {
            let scale = monitor.scale_factor();
            let (w, h) = (
                (PROMPT_SIZE.0 * scale).round() as u32,
                (PROMPT_SIZE.1 * scale).round() as u32,
            );
            let area = monitor.work_area();
            let margin = (PROMPT_MARGIN * scale).round() as i32;
            let x = area.position.x + area.size.width as i32 - w as i32 - margin;
            let y = area.position.y + area.size.height as i32 - h as i32 - margin;
            let _ = window.set_size(PhysicalSize::new(w, h));
            let _ = window.set_position(PhysicalPosition::new(x, y));
        }
        let _ = window.show();
        let _ = window.set_always_on_top(true);
    });
    let _ = MeetingPromptEvent { state: Some(state) }.emit_to(app, PROMPT_LABEL);
}

pub fn hide_prompt(app: &AppHandle) {
    *PROMPT.lock().unwrap_or_else(|p| p.into_inner()) = None;
    let _ = MeetingPromptEvent { state: None }.emit_to(app, PROMPT_LABEL);
    if let Some(window) = app.get_webview_window(PROMPT_LABEL) {
        let _ = window.hide();
    }
}
