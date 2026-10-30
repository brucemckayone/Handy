//! Copies the text selected in whatever app has focus, for the read-aloud
//! and Ask shortcuts, then puts the clipboard back as it was.

use log::warn;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;

/// How long the focused app gets to put its selection on the clipboard.
const COPY_WAIT: Duration = Duration::from_millis(600);
const POLL: Duration = Duration::from_millis(20);

/// How the focused app is asked to copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyKeys {
    /// Ctrl+C (Cmd+C on macOS).
    Standard,
    /// Ctrl+Insert: for Ask, which is often used from terminals, where Ctrl+C
    /// with nothing selected would stop the program running there.
    Insert,
}

/// The selected text, or `None` if nothing is selected.
pub fn copy_selection(app: &AppHandle) -> Option<String> {
    copy_selection_with(app, CopyKeys::Standard)
}

/// [`copy_selection`], copying with `keys`.
pub fn copy_selection_with(app: &AppHandle, keys: CopyKeys) -> Option<String> {
    // Ctrl+C sent while the shortcut's Alt is still held would be Ctrl+Alt+C.
    wait_for_modifiers_up();
    let clipboard = app.clipboard();
    let saved_text = clipboard.read_text().ok().filter(|t| !t.is_empty());
    let saved_image = if saved_text.is_none() {
        clipboard.read_image().ok().map(|image| image.to_owned())
    } else {
        None
    };
    let before = clipboard_version();

    let sent = crate::clipboard::with_enigo(app, move |enigo| match keys {
        CopyKeys::Standard => crate::input::send_copy(enigo, 50),
        CopyKeys::Insert => crate::input::send_copy_insert(enigo, 50),
    });
    if let Err(e) = sent {
        warn!("Couldn't copy the selection: {e}");
        return None;
    }
    let deadline = Instant::now() + COPY_WAIT;
    let mut copied = None;
    while Instant::now() < deadline {
        std::thread::sleep(POLL);
        if clipboard_version() != before {
            copied = clipboard.read_text().ok();
            break;
        }
    }
    if copied.is_some() {
        if let Some(text) = saved_text {
            let _ = clipboard.write_text(text);
        } else if let Some(image) = saved_image {
            let _ = clipboard.write_image(&image);
        } else {
            let _ = clipboard.clear();
        }
    }
    copied.filter(|text| !text.trim().is_empty())
}

/// Changes whenever anything is put on the clipboard.
#[cfg(windows)]
fn clipboard_version() -> u32 {
    // SAFETY: takes no arguments and only reads a counter.
    unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() }
}

#[cfg(target_os = "macos")]
fn clipboard_version() -> isize {
    objc2_app_kit::NSPasteboard::generalPasteboard().changeCount()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn clipboard_version() -> u64 {
    // No cheap counter here: treat the clipboard as changed after a pause.
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALLS: AtomicU64 = AtomicU64::new(0);
    std::thread::sleep(Duration::from_millis(100));
    CALLS.fetch_add(1, Ordering::Relaxed)
}

/// Waits (briefly) until Ctrl, Alt, Shift and Windows are all let go.
#[cfg(windows)]
fn wait_for_modifiers_up() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    let held = || {
        [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
            .iter()
            // SAFETY: reads the key's state; any virtual-key code is valid.
            .any(|key| unsafe { GetAsyncKeyState(i32::from(key.0)) } < 0)
    };
    while held() && Instant::now() < deadline {
        std::thread::sleep(POLL);
    }
}

/// Waits (briefly) until Control, Option, Shift and Command are all let go.
#[cfg(target_os = "macos")]
fn wait_for_modifiers_up() {
    use objc2_core_graphics::{CGEventFlags, CGEventSource, CGEventSourceStateID};
    let modifiers = CGEventFlags::MaskControl
        | CGEventFlags::MaskAlternate
        | CGEventFlags::MaskShift
        | CGEventFlags::MaskCommand;
    let deadline = Instant::now() + Duration::from_secs(2);
    let held = || {
        CGEventSource::flags_state(CGEventSourceStateID::CombinedSessionState).intersects(modifiers)
    };
    while held() && Instant::now() < deadline {
        std::thread::sleep(POLL);
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn wait_for_modifiers_up() {
    std::thread::sleep(Duration::from_millis(300));
}
