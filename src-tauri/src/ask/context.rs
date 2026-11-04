//! What the app in front can tell beyond the text it shows: the shells open
//! in a terminal and what runs under them, or the files selected in File
//! Explorer. Read as the Ask key goes down.

use super::agents::Process;
use super::window::WindowInfo;

/// Shells, by program, and what to call them.
const SHELLS: &[(&str, &str)] = &[
    ("pwsh.exe", "PowerShell 7"),
    ("powershell.exe", "Windows PowerShell"),
    ("cmd.exe", "Command Prompt"),
    ("wsl.exe", "WSL"),
    ("bash.exe", "bash"),
    ("nu.exe", "Nushell"),
    ("ssh.exe", "ssh"),
];

/// Programs that are the terminal's own plumbing, not something running.
const PLUMBING: &[&str] = &[
    "conhost.exe",
    "openconsole.exe",
    "wslhost.exe",
    "wslrelay.exe",
    "wsl.exe",
];

/// The most running programs named.
const MAX_RUNNING: usize = 6;
/// The most selected files listed.
pub const MAX_FILES: usize = 50;

/// A terminal's shells and what runs under them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Terminal {
    /// "PowerShell 7", "WSL": one per tab or pane, in no particular order.
    pub shells: Vec<String>,
    /// Programs running under them, e.g. `cargo.exe`.
    pub running: Vec<String>,
}

fn shell_name(exe: &str) -> Option<&'static str> {
    let exe = exe.to_lowercase();
    SHELLS
        .iter()
        .find(|(e, _)| *e == exe)
        .map(|(_, name)| *name)
}

/// The shells under terminal `window` and the programs under those, from
/// a snapshot of the processes. `None` when the window isn't a terminal
/// or has no shell.
pub fn terminal(window: &WindowInfo, processes: &[Process]) -> Option<Terminal> {
    if !super::agents::is_terminal(&window.exe) {
        return None;
    }
    let parent_of = |pid: u32| processes.iter().find(|p| p.pid == pid).map(|p| p.parent);
    let under = |pid: u32, ancestor: u32| {
        let mut current = pid;
        // Bounded: a reused pid can make a loop.
        for _ in 0..64 {
            match parent_of(current) {
                Some(parent) if parent == ancestor => return true,
                Some(parent) if parent != 0 && parent != current => current = parent,
                _ => return false,
            }
        }
        false
    };
    // A classic console window says it belongs to its shell.
    let shells: Vec<&Process> = processes
        .iter()
        .filter(|p| shell_name(&p.exe).is_some())
        .filter(|p| p.pid == window.pid || under(p.pid, window.pid))
        .collect();
    if shells.is_empty() {
        return None;
    }
    let mut running: Vec<String> = Vec::new();
    for p in processes {
        let exe = p.exe.to_lowercase();
        if shell_name(&exe).is_some()
            || PLUMBING.contains(&exe.as_str())
            || !shells.iter().any(|s| under(p.pid, s.pid))
            || running.contains(&p.exe)
        {
            continue;
        }
        if running.len() < MAX_RUNNING {
            running.push(p.exe.clone());
        }
    }
    Some(Terminal {
        shells: shells
            .iter()
            .filter_map(|s| shell_name(&s.exe))
            .map(str::to_string)
            .collect(),
        running,
    })
}

/// The full paths of the files selected in File Explorer window `window`,
/// in its tab in front. Empty when it isn't Explorer or nothing is.
#[cfg(windows)]
pub fn explorer_selection(window: &WindowInfo) -> Vec<String> {
    use windows::core::Interface;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoTaskMemFree, IServiceProvider, CLSCTX_LOCAL_SERVER,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Shell::{
        IFolderView, IShellBrowser, IShellItemArray, IShellWindows, IWebBrowserApp,
        SID_STopLevelBrowser, ShellWindows, SIGDN_FILESYSPATH, SVGIO_SELECTION,
    };
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;

    if !window.exe.eq_ignore_ascii_case("explorer.exe") {
        return Vec::new();
    }
    let mut paths = Vec::new();
    // SAFETY: COM calls on live objects from this thread, which the caller
    // has put in a COM apartment; each returned string is freed once read.
    unsafe {
        let Ok(shell_windows) =
            CoCreateInstance::<_, IShellWindows>(&ShellWindows, None, CLSCTX_LOCAL_SERVER)
        else {
            return paths;
        };
        let count = shell_windows.Count().unwrap_or(0);
        for i in 0..count {
            let Ok(item) = shell_windows.Item(&VARIANT::from(i)) else {
                continue;
            };
            let Ok(browser) = item.cast::<IWebBrowserApp>() else {
                continue;
            };
            if browser.HWND().map(|h| h.0) != Ok(window.handle) {
                continue;
            }
            let Ok(shell) = browser
                .cast::<IServiceProvider>()
                .and_then(|sp| sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser))
            else {
                continue;
            };
            // Each tab of the window is listed; only the one in front shows.
            if !shell
                .GetWindow()
                .is_ok_and(|tab| IsWindowVisible(tab).as_bool())
            {
                continue;
            }
            let Ok(items) = shell
                .QueryActiveShellView()
                .and_then(|view| view.cast::<IFolderView>())
                .and_then(|view| view.Items::<IShellItemArray>(SVGIO_SELECTION))
            else {
                break;
            };
            for j in 0..items.GetCount().unwrap_or(0).min(MAX_FILES as u32) {
                let Ok(name) = items
                    .GetItemAt(j)
                    .and_then(|item| item.GetDisplayName(SIGDN_FILESYSPATH))
                else {
                    continue;
                };
                if let Ok(path) = name.to_string() {
                    paths.push(path);
                }
                CoTaskMemFree(Some(name.0 as *const _));
            }
            break;
        }
    }
    paths
}

#[cfg(not(windows))]
pub fn explorer_selection(_window: &WindowInfo) -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(exe: &str, pid: u32) -> WindowInfo {
        WindowInfo {
            handle: 1,
            pid,
            exe: exe.into(),
            title: String::new(),
        }
    }

    fn process(pid: u32, parent: u32, exe: &str) -> Process {
        Process {
            pid,
            parent,
            exe: exe.into(),
        }
    }

    #[test]
    fn a_terminal_names_its_shells_and_what_runs_under_them() {
        let processes = [
            process(10, 1, "WindowsTerminal.exe"),
            process(11, 10, "OpenConsole.exe"),
            process(12, 11, "pwsh.exe"),
            process(13, 12, "cargo.exe"),
            process(14, 13, "rustc.exe"),
            process(15, 13, "rustc.exe"),
            process(16, 10, "OpenConsole.exe"),
            process(17, 16, "wsl.exe"),
            process(18, 17, "wslhost.exe"),
            // Another terminal's shell.
            process(20, 1, "WindowsTerminal.exe"),
            process(21, 20, "cmd.exe"),
        ];
        let found = terminal(&window("WindowsTerminal.exe", 10), &processes).unwrap();
        assert_eq!(found.shells, ["PowerShell 7", "WSL"]);
        assert_eq!(found.running, ["cargo.exe", "rustc.exe"]);
    }

    #[test]
    fn a_console_window_belongs_to_its_shell() {
        let processes = [process(30, 1, "cmd.exe"), process(31, 30, "conhost.exe")];
        let found = terminal(&window("cmd.exe", 30), &processes).unwrap();
        assert_eq!(found.shells, ["Command Prompt"]);
        assert!(found.running.is_empty());
    }

    #[test]
    fn other_apps_are_not_terminals() {
        let processes = [process(40, 1, "Code.exe"), process(41, 40, "pwsh.exe")];
        assert_eq!(terminal(&window("Code.exe", 40), &processes), None);
    }
}
