//! The grab overlay: the screen under the mouse, frozen under a dim layer,
//! to pick a region, a window or the whole screen from, then mark up. It
//! covers that screen and takes the keyboard while it is open; the question
//! keeps recording hands-free underneath.

use log::{debug, error, warn};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewWindowBuilder};
use tauri_specta::Event;

use super::capture::{Capture, CaptureKind};
use super::timeline::{MarkKind, Region, What};
use super::{screenshot, voice, window};

pub const LABEL: &str = "ask_grab";

/// A window on the frozen screen, in its pixels, frontmost first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct GrabWindow {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub label: String,
}

/// A picture already taken, opened from the pill to mark up again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct GrabCapture {
    /// Its name in the question: Snap `n`.
    pub kind: CaptureKind,
    pub n: u32,
    /// Its size in pixels.
    pub width: u32,
    pub height: u32,
}

/// What the grab overlay shows; `None` once it closes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct GrabFrame {
    /// The frozen screen, a BMP file for `convertFileSrc`; or, marking up a
    /// capture again, its PNG.
    pub path: String,
    /// The screen the overlay covers, in its pixels.
    pub width: u32,
    pub height: u32,
    pub windows: Vec<GrabWindow>,
    /// The number the next mark gets: marks count on across the question.
    pub first_mark: u32,
    /// The window under the mouse, for a region that isn't a window; for a
    /// capture, the window it shows.
    pub under_mouse: String,
    pub capture: Option<GrabCapture>,
    /// For a question being typed in the panel: it waits with the draft.
    pub draft: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct AskGrabEvent {
    pub frame: Option<GrabFrame>,
}

/// Esc reached Handy's cancel shortcut instead of the overlay, which
/// treats it as its own.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct AskGrabEscapeEvent {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum GrabMarkKind {
    Arrow,
    Box,
    Highlight,
    Pen,
    Text,
    Pin,
}

impl From<GrabMarkKind> for MarkKind {
    fn from(kind: GrabMarkKind) -> Self {
        match kind {
            GrabMarkKind::Arrow => MarkKind::Arrow,
            GrabMarkKind::Box => MarkKind::Box,
            GrabMarkKind::Highlight => MarkKind::Highlight,
            GrabMarkKind::Pen => MarkKind::Pen,
            GrabMarkKind::Text => MarkKind::Text,
            GrabMarkKind::Pin => MarkKind::Pin,
        }
    }
}

/// A mark as drawn in the overlay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct GrabMark {
    pub n: u32,
    pub kind: GrabMarkKind,
    /// How long before the grab was added it was drawn.
    pub ago_ms: u32,
    /// Where on the grab, in percent of its width and height.
    pub x0: u8,
    pub x1: u8,
    pub y0: u8,
    pub y1: u8,
    /// An arrow's ends, when they start and finish on other marks.
    pub joins: Option<(u32, u32)>,
    /// What a text mark says.
    pub label: Option<String>,
}

impl GrabMark {
    /// The mark on the Ask clock's terms; `on` is filled in once the grab
    /// has its name.
    fn what(&self) -> What {
        What::Mark {
            n: self.n,
            kind: self.kind.into(),
            on: String::new(),
            region: Some(Region {
                x0: self.x0.min(100),
                x1: self.x1.min(100),
                y0: self.y0.min(100),
                y1: self.y1.min(100),
            }),
            joins: self.joins,
            encloses: None,
            label: self
                .label
                .as_ref()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty()),
        }
    }
}

/// The marked-up grab, as the overlay hands it back.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct GrabResult {
    /// The picture with its marks drawn in, as base64 PNG.
    pub png: String,
    /// A small copy as a data URL, for the pill.
    pub thumbnail: String,
    /// The window it shows.
    pub of: String,
    pub marks: Vec<GrabMark>,
    /// Ctrl+Enter: add it and ask now.
    pub ask_now: bool,
}

static OPEN: Lazy<Mutex<Option<GrabFrame>>> = Lazy::new(|| Mutex::new(None));

/// Creates the overlay, hidden.
pub fn create(app: &AppHandle) {
    let url = "src/meeting-widgets/index.html?view=grab";
    let mut builder = WebviewWindowBuilder::new(app, LABEL, tauri::WebviewUrl::App(url.into()))
        .title("Handy Grab")
        .resizable(false)
        .inner_size(800.0, 600.0)
        .shadow(false)
        .maximizable(false)
        .minimizable(false)
        .closable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        // Takes the keys for the tools, Enter and Esc.
        .focusable(true)
        .focused(false)
        .visible(false);
    if let Some(data_dir) = crate::portable::data_dir() {
        builder = builder.data_directory(data_dir.join("webview"));
    }
    if let Err(e) = builder.build() {
        error!("Failed to create the grab overlay: {}", e);
    }
}

pub fn is_open() -> bool {
    OPEN.lock().is_ok_and(|open| open.is_some())
}

/// What the overlay shows now, for a page that loads late.
pub fn frame() -> Option<GrabFrame> {
    OPEN.lock().ok().and_then(|open| open.clone())
}

/// The screen under the mouse: where it starts and its size.
fn screen_under_mouse(app: &AppHandle) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|at| app.monitor_from_point(at.x, at.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten())?;
    Some((*monitor.position(), *monitor.size()))
}

/// Freezes the screen under the mouse and opens the overlay on it, for the
/// question being asked. Call off the main thread: copying a 4K screen
/// takes a moment.
pub fn open(app: &AppHandle) {
    open_screen(app, false);
}

/// As [`open`], for the question being typed in the panel.
pub fn open_for_draft(app: &AppHandle) {
    open_screen(app, true);
}

fn open_screen(app: &AppHandle, draft: bool) {
    if is_open() {
        return;
    }
    let Some((origin, size)) = screen_under_mouse(app) else {
        warn!("Ask: no screen to grab");
        return;
    };
    let Some(screen) =
        screenshot::copy_screen(origin.x, origin.y, size.width as i32, size.height as i32)
    else {
        warn!("Ask: couldn't copy the screen to grab from");
        return;
    };
    let Some(path) = screenshot::save_frame(&screen) else {
        return;
    };
    let under_mouse = window::under_mouse().map(|w| w.label()).unwrap_or_default();
    let frame = GrabFrame {
        path: path.to_string_lossy().into_owned(),
        width: screen.width,
        height: screen.height,
        windows: windows_on(origin.x, origin.y, screen.width, screen.height),
        first_mark: if draft {
            super::draft::marks_so_far()
        } else {
            voice::marks_so_far()
        } + 1,
        under_mouse,
        capture: None,
        draft,
    };
    present(app, frame, origin, size);
}

/// Opens capture `n` (from the pill) in the overlay, to mark it up again or
/// take it out of the question.
pub fn open_capture(app: &AppHandle, n: u32) {
    if is_open() {
        return;
    }
    let Some((kind, path, of)) = voice::capture(n) else {
        return;
    };
    let Some((width, height)) = screenshot::png_size(&path) else {
        warn!("Ask: couldn't read capture {n} to mark it up");
        return;
    };
    let Some((origin, size)) = screen_under_mouse(app) else {
        return;
    };
    let frame = GrabFrame {
        path: path.to_string_lossy().into_owned(),
        width: size.width,
        height: size.height,
        windows: Vec::new(),
        first_mark: voice::marks_so_far() + 1,
        under_mouse: of,
        capture: Some(GrabCapture {
            kind,
            n,
            width,
            height,
        }),
        draft: false,
    };
    present(app, frame, origin, size);
}

/// Shows `frame` in the overlay, covering the screen at `origin`.
fn present(
    app: &AppHandle,
    frame: GrabFrame,
    origin: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
) {
    if let Ok(mut open) = OPEN.lock() {
        *open = Some(frame.clone());
    }
    let _ = AskGrabEvent { frame: Some(frame) }.emit_to(app, LABEL);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(window) = handle.get_webview_window(LABEL) else {
            return;
        };
        let _ = window.set_position(PhysicalPosition::new(origin.x, origin.y));
        let _ = window.set_size(PhysicalSize::new(size.width, size.height));
        let _ = window.show();
        raise(&window);
        let _ = window.set_focus();
        // The pill stays in sight above it.
        crate::overlay::raise_recording_overlay(&handle);
    });
}

/// Closes the overlay. A frozen screen is deleted; a capture being marked
/// up again stays with the question.
pub fn close(app: &AppHandle) {
    let frame = OPEN.lock().ok().and_then(|mut open| open.take());
    let Some(frame) = frame else { return };
    if frame.capture.is_none() {
        let _ = std::fs::remove_file(&frame.path);
    }
    let _ = AskGrabEvent { frame: None }.emit_to(app, LABEL);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(LABEL) {
            let _ = window.hide();
        }
    });
}

/// Esc while the overlay is open: Handy's cancel shortcut catches it before
/// the overlay can, so it is passed on.
pub fn escape(app: &AppHandle) {
    let _ = AskGrabEscapeEvent {}.emit_to(app, LABEL);
}

/// The overlay is done with a grab: it joins the question. A capture marked
/// up again takes the new picture and marks in its place.
pub fn finish(app: &AppHandle, result: GrabResult) -> Result<(), String> {
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode(result.png.as_bytes())
        .map_err(|e| format!("not base64: {e}"))?;
    let path: PathBuf = screenshot::save_png_bytes(&png).ok_or("couldn't save the grab")?;
    let open = frame();
    let again = open.as_ref().and_then(|f| f.capture.as_ref()).map(|c| c.n);
    let draft = open.is_some_and(|f| f.draft);
    close(app);
    debug!(
        "Ask: grab of {} with {} marks",
        result.of,
        result.marks.len()
    );
    let marks = result
        .marks
        .iter()
        .map(|m| (u64::from(m.ago_ms), m.what()))
        .collect();
    let capture = |marks| Capture {
        kind: CaptureKind::Grab,
        n: 0,
        of: result.of,
        path: path.clone(),
        thumbnail: result.thumbnail.clone(),
        at: 0,
        marks,
    };
    match again {
        Some(n) => voice::marked_up(app, n, path, result.thumbnail, marks),
        None if draft => {
            super::draft::add(app, capture(marks));
            super::draft::refocus_panel(app);
            // A typed question is sent from the panel.
            return Ok(());
        }
        None => voice::add_capture(app, capture(marks)),
    }
    if result.ask_now {
        if let Some(coordinator) = app.try_state::<crate::TranscriptionCoordinator>() {
            coordinator.finish("ask", "grab");
        }
    }
    Ok(())
}

/// Takes capture `n` out of the question, closing the overlay if it shows it.
pub fn take_out(app: &AppHandle, n: u32) {
    if frame().and_then(|f| f.capture).is_some_and(|c| c.n == n) {
        close(app);
    }
    voice::remove_capture(app, n);
}

/// Puts the overlay above every other window, always-on-top ones included
/// (tao only acts when the setting changes), and out of Alt+Tab.
#[cfg(windows)]
fn raise(window: &tauri::WebviewWindow) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST, SWP_NOMOVE,
        SWP_NOSIZE, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    };
    let Ok(hwnd) = window.hwnd() else { return };
    // SAFETY: restyles and restacks our own live window.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let style = (style | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE,
        );
    }
}

#[cfg(not(windows))]
fn raise(window: &tauri::WebviewWindow) {
    let _ = window.set_always_on_top(true);
}

/// The windows showing on the screen at (`left`, `top`), frontmost first,
/// in that screen's pixels, so a click can pick one. Handy's own, hidden,
/// minimised and click-through ones are left out, as is the desktop.
#[cfg(windows)]
fn windows_on(left: i32, top: i32, width: u32, height: u32) -> Vec<GrabWindow> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Dwm::{
        DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowLongPtrW, GetWindowThreadProcessId, IsIconic,
        IsWindowVisible, GWL_EXSTYLE, WS_EX_TRANSPARENT,
    };

    unsafe extern "system" fn each(hwnd: HWND, list: LPARAM) -> BOOL {
        // SAFETY: `list` is the Vec passed to EnumWindows below, alive for
        // the whole enumeration.
        let list = unsafe { &mut *(list.0 as *mut Vec<HWND>) };
        list.push(hwnd);
        true.into()
    }

    let mut handles: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes into `handles`, which outlives the call.
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut handles as *mut Vec<HWND> as isize));
    }
    let own = std::process::id();
    let (right, bottom) = (left + width as i32, top + height as i32);
    let mut found = Vec::new();
    for hwnd in handles {
        // SAFETY: plain queries on window handles; every buffer outlives the
        // call that fills it.
        let rect = unsafe {
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                continue;
            }
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if style & WS_EX_TRANSPARENT.0 != 0 {
                continue;
            }
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                (&mut cloaked as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
            if cloaked != 0 {
                continue;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == own {
                continue;
            }
            let mut class = [0u16; 64];
            let len = GetClassNameW(hwnd, &mut class).max(0) as usize;
            let class = String::from_utf16_lossy(&class[..len]);
            if class == "Progman" || class == "WorkerW" {
                continue;
            }
            let mut rect = RECT::default();
            if DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&mut rect as *mut RECT).cast(),
                std::mem::size_of::<RECT>() as u32,
            )
            .is_err()
            {
                continue;
            }
            rect
        };
        let (x0, y0) = (rect.left.max(left), rect.top.max(top));
        let (x1, y1) = (rect.right.min(right), rect.bottom.min(bottom));
        if x1 - x0 < 8 || y1 - y0 < 8 {
            continue;
        }
        let Some(label) = window::info_of(hwnd.0 as isize).map(|w| w.label()) else {
            continue;
        };
        if label.is_empty() {
            continue;
        }
        found.push(GrabWindow {
            x: x0 - left,
            y: y0 - top,
            width: (x1 - x0) as u32,
            height: (y1 - y0) as u32,
            label,
        });
    }
    found
}

#[cfg(not(windows))]
fn windows_on(_left: i32, _top: i32, _width: u32, _height: u32) -> Vec<GrabWindow> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_come_back_on_the_ask_clock_terms() {
        let mark = GrabMark {
            n: 3,
            kind: GrabMarkKind::Arrow,
            ago_ms: 1_200,
            x0: 10,
            x1: 140,
            y0: 5,
            y1: 50,
            joins: Some((1, 2)),
            label: Some("  ".into()),
        };
        let What::Mark {
            n,
            kind,
            region,
            joins,
            label,
            ..
        } = mark.what()
        else {
            panic!("a mark");
        };
        assert_eq!(
            (n, kind, joins, label),
            (3, MarkKind::Arrow, Some((1, 2)), None)
        );
        assert_eq!(region.map(|r| r.x1), Some(100), "kept on the picture");
    }
}
