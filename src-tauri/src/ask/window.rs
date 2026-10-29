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

#[cfg(not(windows))]
pub fn foreground() -> Option<WindowInfo> {
    None
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

    #[cfg(windows)]
    #[test]
    fn finds_this_process_program() {
        let exe = process_exe(std::process::id()).expect("own exe");
        assert!(exe.to_lowercase().ends_with(".exe"));
    }
}
