//! The window a question is asked from: the one in front when the Ask
//! shortcut goes down (Handy's own pop-ups never take focus).

use super::store::WindowKey;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    /// The window handle, as a number.
    pub handle: isize,
    pub pid: u32,
    /// The program's file name, e.g. `Code.exe`.
    pub exe: String,
    pub title: String,
}

impl WindowInfo {
    pub fn key(&self) -> WindowKey {
        WindowKey {
            handle: self.handle.to_string(),
            exe: self.exe.clone(),
        }
    }

    /// The program without `.exe`, e.g. `Code`.
    pub fn app(&self) -> &str {
        let exe = self.exe.as_str();
        match exe.len().checked_sub(4) {
            Some(cut) if exe[cut..].eq_ignore_ascii_case(".exe") => &exe[..cut],
            _ => exe,
        }
    }

    /// For a thread's header: `Code · actions.rs - Handy - Visual Studio Code`.
    pub fn label(&self) -> String {
        match (self.app(), self.title.trim()) {
            ("", title) => title.to_string(),
            (app, "") => app.to_string(),
            (app, title) => format!("{app} · {title}"),
        }
    }
}

/// The window in front, or `None` if there isn't one.
#[cfg(windows)]
pub fn foreground() -> Option<WindowInfo> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };
    // SAFETY: plain queries on a window handle; the buffers outlive the calls.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
        let title = String::from_utf16_lossy(&buf[..len]);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        Some(WindowInfo {
            handle: hwnd.0 as isize,
            pid,
            exe: process_exe(pid).unwrap_or_default(),
            title,
        })
    }
}

/// The window in front: the front app's frontmost window. Its title needs
/// Screen Recording permission, as the screenshot does; without it the title
/// is empty.
#[cfg(target_os = "macos")]
pub fn foreground() -> Option<WindowInfo> {
    let app = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication()?;
    let pid = u32::try_from(app.processIdentifier()).ok()?;
    let (handle, title) = front_window(&on_screen_windows(), pid)?;
    Some(WindowInfo {
        handle,
        pid,
        // The app's name (`Code`, `Terminal`): its executable is often a
        // generic `Electron`.
        exe: app
            .localizedName()
            .map(|name| name.to_string())
            .unwrap_or_default(),
        title,
    })
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn foreground() -> Option<WindowInfo> {
    None
}

/// A window on screen, as the window server lists it.
#[cfg(any(target_os = "macos", test))]
struct Listed {
    number: isize,
    pid: u32,
    /// 0 for normal windows; menus, panels and the menu bar sit higher.
    layer: i64,
    title: String,
}

/// `pid`'s frontmost normal window, from `windows` listed front to back.
#[cfg(any(target_os = "macos", test))]
fn front_window(windows: &[Listed], pid: u32) -> Option<(isize, String)> {
    windows
        .iter()
        .find(|w| w.pid == pid && w.layer == 0)
        .map(|w| (w.number, w.title.clone()))
}

/// The windows on screen, front to back.
#[cfg(target_os = "macos")]
fn on_screen_windows() -> Vec<Listed> {
    use objc2_core_foundation::{CFDictionary, CFNumber, CFString, CFType};
    use objc2_core_graphics::{
        kCGWindowLayer, kCGWindowName, kCGWindowNumber, kCGWindowOwnerPID,
        CGWindowListCopyWindowInfo, CGWindowListOption,
    };
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    // 0 is kCGNullWindowID: every window, not those relative to one.
    let Some(list) = CGWindowListCopyWindowInfo(options, 0) else {
        return Vec::new();
    };
    // SAFETY: the window list is an array of dictionaries keyed by strings,
    // and the keys are read-only framework statics.
    unsafe {
        let (number_key, pid_key, layer_key, name_key) = (
            kCGWindowNumber,
            kCGWindowOwnerPID,
            kCGWindowLayer,
            kCGWindowName,
        );
        list.cast_unchecked::<CFDictionary>()
            .iter()
            .filter_map(|window| {
                let window = window.cast_unchecked::<CFString, CFType>();
                let number = |key: &CFString| window.get(key)?.downcast_ref::<CFNumber>()?.as_i64();
                Some(Listed {
                    number: isize::try_from(number(number_key)?).ok()?,
                    pid: u32::try_from(number(pid_key)?).ok()?,
                    layer: number(layer_key)?,
                    title: window
                        .get(name_key)
                        .and_then(|name| name.downcast_ref::<CFString>().map(|s| s.to_string()))
                        .unwrap_or_default(),
                })
            })
            .collect()
    }
}

/// The file name of process `pid`'s program.
#[cfg(windows)]
pub fn process_exe(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: the handle is closed before returning; the buffer outlives the
    // call and its length is passed with it.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let named = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        named.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(exe: &str, title: &str) -> WindowInfo {
        WindowInfo {
            handle: 42,
            pid: 7,
            exe: exe.into(),
            title: title.into(),
        }
    }

    #[test]
    fn labels_drop_the_exe() {
        assert_eq!(
            window("Code.exe", "a.rs - Handy").label(),
            "Code · a.rs - Handy"
        );
        assert_eq!(window("NOTEPAD.EXE", "").label(), "NOTEPAD");
        assert_eq!(window("", "Untitled").label(), "Untitled");
        assert_eq!(window("Code.exe", "x").key().handle, "42");
    }

    fn listed(number: isize, pid: u32, layer: i64, title: &str) -> Listed {
        Listed {
            number,
            pid,
            layer,
            title: title.into(),
        }
    }

    #[test]
    fn the_front_window_is_the_apps_first_normal_one() {
        // Front to back: another app's menu, this app's panel, then its
        // windows.
        let windows = [
            listed(1, 9, 101, "menu"),
            listed(2, 7, 3, "panel"),
            listed(3, 7, 0, "a.rs - Handy"),
            listed(4, 7, 0, "older"),
        ];
        assert_eq!(
            front_window(&windows, 7),
            Some((3, "a.rs - Handy".to_string()))
        );
        assert_eq!(front_window(&windows, 8), None);
    }

    /// Live: in a desktop session (`cargo test -- --ignored live`), finds the
    /// window in front.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn live_finds_the_window_in_front() {
        let window = foreground().expect("a window in front");
        println!("{window:?} -> {}", window.label());
        assert!(window.handle > 0 && window.pid > 0);
        assert!(!window.exe.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn finds_this_process_program() {
        let exe = process_exe(std::process::id()).expect("own exe");
        assert!(exe.to_lowercase().ends_with(".exe"));
    }
}
