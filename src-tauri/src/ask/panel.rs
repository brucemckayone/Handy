//! The Ask panel: a pop-up docked on the right, like the dictation overlay
//! rather than a normal window (no title bar, taskbar button or Alt+Tab
//! entry). It opens only when asked to, from the speaking pill, the tray or
//! the setting, and closes on Esc, its x, or a click anywhere else.

use log::error;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::Mutex;
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewWindowBuilder, WindowEvent,
};
use tauri_specta::Event;

use super::PANEL_LABEL;

const SIZE: (f64, f64) = (380.0, 780.0);
/// Gap to the screen's edges.
const MARGIN: f64 = 12.0;

/// Show this thread (`None`: the latest).
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct AskOpenEvent {
    pub thread_id: Option<String>,
}

/// The thread last opened, for a panel page that loads late.
static OPENED: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

pub fn opened_thread() -> Option<String> {
    OPENED.lock().ok().and_then(|o| o.clone())
}

pub fn set_opened_thread(thread_id: Option<String>) {
    if let Ok(mut opened) = OPENED.lock() {
        *opened = thread_id;
    }
}

/// Creates the panel, hidden.
pub fn create(app: &AppHandle) {
    let url = "src/meeting-widgets/index.html?view=ask";
    let mut builder =
        WebviewWindowBuilder::new(app, PANEL_LABEL, tauri::WebviewUrl::App(url.into()))
            .title("Handy Ask")
            .resizable(false)
            .inner_size(SIZE.0, SIZE.1)
            .shadow(false)
            .maximizable(false)
            .minimizable(false)
            .closable(false)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .transparent(true)
            // Takes typing, unlike the other pop-ups.
            .focusable(true)
            .focused(false)
            .visible(false);
    if let Some(data_dir) = crate::portable::data_dir() {
        builder = builder.data_directory(data_dir.join("webview"));
    }
    match builder.build() {
        Ok(window) => {
            hide_from_alt_tab(&window);
            let handle = app.clone();
            window.on_window_event(move |event| {
                // A click anywhere else closes it, like a pop-up.
                if let WindowEvent::Focused(false) = event {
                    close(&handle);
                }
            });
        }
        Err(e) => error!("Failed to create the Ask panel: {}", e),
    }
}

/// A tool window is left out of Alt+Tab and the taskbar.
#[cfg(windows)]
fn hide_from_alt_tab(window: &tauri::WebviewWindow) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    };
    let Ok(hwnd) = window.hwnd() else { return };
    // SAFETY: changes the extended style of our own live window.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let style = (style | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
    }
}

#[cfg(not(windows))]
fn hide_from_alt_tab(_window: &tauri::WebviewWindow) {}

/// Opens the panel on `thread_id` (or the latest thread). `focus` lets it
/// take typing at once; without it, it shows without taking focus.
pub fn open(app: &AppHandle, thread_id: Option<String>, focus: bool) {
    set_opened_thread(thread_id.clone());
    let _ = AskOpenEvent {
        thread_id: thread_id.clone(),
    }
    .emit_to(app, PANEL_LABEL);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(window) = handle.get_webview_window(PANEL_LABEL) else {
            return;
        };
        if let Ok(Some(monitor)) = handle.primary_monitor() {
            let scale = monitor.scale_factor();
            let area = monitor.work_area();
            let margin = (MARGIN * scale).round() as i32;
            let w = (SIZE.0 * scale).round() as i32;
            let h = ((SIZE.1 * scale).round() as i32).min(area.size.height as i32 - 2 * margin);
            let x = area.position.x + area.size.width as i32 - w - margin;
            let y = area.position.y + area.size.height as i32 - h - margin;
            let _ = window.set_size(PhysicalSize::new(w as u32, h.max(1) as u32));
            let _ = window.set_position(PhysicalPosition::new(x, y));
        }
        let _ = window.show();
        let _ = window.set_always_on_top(true);
        if focus {
            let _ = window.set_focus();
        }
    });
}

pub fn close(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(PANEL_LABEL) {
            let _ = window.hide();
        }
    });
}

pub fn is_open(app: &AppHandle) -> bool {
    app.get_webview_window(PANEL_LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}
