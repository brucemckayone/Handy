//! Claude Code sessions running on this computer, and which one (if any) is
//! in the window a question is asked from. A question there is answered from
//! a read-only copy of that session's conversation.
//!
//! Window titles don't name Claude sessions, so a window matches a session
//! when the session's process runs under the window's program (a terminal,
//! or an editor's built-in terminal), and, where that could be several, when
//! the title names the session's folder or name.

use serde::Deserialize;
use std::collections::HashMap;
use std::process::Stdio;
use std::time::{Duration, Instant};

use super::store::AgentLink;
use super::window::WindowInfo;
use crate::claude_cli;

/// `claude agents --json` takes ~0.7 s; past this it is given up.
const LIST_TIMEOUT: Duration = Duration::from_secs(5);

/// Programs that are terminals: one window, often one session.
const TERMINALS: &[&str] = &[
    "windowsterminal.exe",
    "openconsole.exe",
    "conhost.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "wezterm-gui.exe",
    "alacritty.exe",
    "mintty.exe",
    "tabby.exe",
    "hyper.exe",
    // macOS: app names, as the window's `exe` holds there.
    "terminal",
    "iterm2",
    "ghostty",
    "wezterm",
    "alacritty",
    "kitty",
    "warp",
    "tabby",
    "hyper",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Agent {
    pub session_id: String,
    pub cwd: String,
    pub name: String,
    pub pid: Option<u32>,
}

impl Agent {
    pub fn link(&self) -> AgentLink {
        AgentLink {
            session_id: self.session_id.clone(),
            name: self.name.clone(),
            cwd: self.cwd.clone(),
            linked: true,
        }
    }

    fn folder(&self) -> &str {
        self.cwd
            .trim_end_matches(['\\', '/'])
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or("")
    }
}

#[derive(Deserialize)]
struct Listed {
    #[serde(rename = "sessionId")]
    session_id: String,
    cwd: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
}

/// Parses `claude agents --json`.
pub fn parse(json: &str) -> Vec<Agent> {
    serde_json::from_str::<Vec<Listed>>(json)
        .unwrap_or_default()
        .into_iter()
        .map(|l| Agent {
            name: l
                .name
                .unwrap_or_else(|| l.session_id.chars().take(8).collect()),
            session_id: l.session_id,
            cwd: l.cwd,
            pid: l.pid,
        })
        .collect()
}

/// The sessions running now; empty if Claude Code isn't installed.
pub fn list() -> Vec<Agent> {
    let Some(claude) = claude_cli::find_claude() else {
        return Vec::new();
    };
    let Ok(mut child) = claude_cli::command(&claude)
        .args(["agents", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let Some(mut stdout) = child.stdout.take() else {
        claude_cli::kill_tree(&mut child);
        return Vec::new();
    };
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut out);
        out
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < LIST_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                claude_cli::kill_tree(&mut child);
                return Vec::new();
            }
        }
    }
    parse(&reader.join().unwrap_or_default())
}

/// Each running process's parent.
#[cfg(windows)]
pub fn parents() -> HashMap<u32, u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let mut map = HashMap::new();
    // SAFETY: the snapshot handle is closed before returning; the entry's
    // size is set as the API requires.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return map;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                map.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    map
}

/// Each running process's parent.
#[cfg(target_os = "macos")]
pub fn parents() -> HashMap<u32, u32> {
    let mut map = HashMap::new();
    // SAFETY: each buffer outlives its call, and its size in bytes is passed
    // with it.
    unsafe {
        let count = libc::proc_listallpids(std::ptr::null_mut(), 0);
        if count <= 0 {
            return map;
        }
        // Room for processes started since counting.
        let mut pids = vec![0 as libc::pid_t; count as usize + 64];
        let bytes = std::mem::size_of_val(pids.as_slice()) as libc::c_int;
        let count = libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes).max(0) as usize;
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        for &pid in &pids[..count.min(pids.len())] {
            let mut info: libc::proc_bsdinfo = std::mem::zeroed();
            let filled =
                libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size);
            // A process can end, or be unreadable, between listing and reading.
            if filled == size {
                map.insert(pid as u32, info.pbi_ppid);
            }
        }
    }
    map
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn parents() -> HashMap<u32, u32> {
    HashMap::new()
}

/// Whether `pid` runs under `ancestor` (directly or further down).
fn descends_from(pid: u32, ancestor: u32, parents: &HashMap<u32, u32>) -> bool {
    let mut current = pid;
    // Bounded: a reused pid can make a loop.
    for _ in 0..64 {
        match parents.get(&current) {
            Some(&parent) if parent == ancestor => return true,
            Some(&parent) if parent != 0 && parent != current => current = parent,
            _ => return false,
        }
    }
    false
}

/// Whether `needle` is in `haystack` as a whole word: "handy" is in
/// "lib.rs - handy - code", but "a" is not in "visual".
fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    haystack.match_indices(needle).any(|(i, _)| {
        let before = haystack[..i].chars().next_back();
        let after = haystack[i + needle.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

/// The session running in `window`, if one clearly is.
pub fn pick<'a>(
    agents: &'a [Agent],
    parents: &HashMap<u32, u32>,
    window: &WindowInfo,
) -> Option<&'a Agent> {
    let under: Vec<&Agent> = agents
        .iter()
        .filter(|a| {
            a.pid
                .is_some_and(|pid| descends_from(pid, window.pid, parents))
        })
        .collect();
    let title = window.title.to_lowercase();
    let named = |a: &&&Agent| {
        contains_word(&title, &a.folder().to_lowercase())
            || contains_word(&title, &a.name.to_lowercase())
    };
    if let Some(agent) = under.iter().find(named) {
        return Some(agent);
    }
    // An editor hosts many windows in one program: only the title can tell
    // them apart. A terminal with one session is that session.
    let terminal = TERMINALS.contains(&window.exe.to_lowercase().as_str());
    match under.as_slice() {
        [only] if terminal => Some(only),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str, cwd: &str, pid: u32) -> Agent {
        Agent {
            session_id: format!("sid-{name}"),
            cwd: cwd.into(),
            name: name.into(),
            pid: Some(pid),
        }
    }

    fn window(exe: &str, title: &str, pid: u32) -> WindowInfo {
        WindowInfo {
            handle: 1,
            pid,
            exe: exe.into(),
            title: title.into(),
        }
    }

    /// claude 100 <- shell 90 <- terminal 80; claude 101 <- shell 91 <- editor
    /// pty 81 <- editor 70; claude 102 <- shell 92 <- editor pty 81.
    fn tree() -> HashMap<u32, u32> {
        HashMap::from([
            (100, 90),
            (90, 80),
            (80, 1),
            (101, 91),
            (91, 81),
            (102, 92),
            (92, 81),
            (81, 70),
            (70, 1),
        ])
    }

    #[test]
    fn parses_claude_agents_json() {
        let json = r#"[{"id":"7301428b","cwd":"C:\\dev\\maverick","kind":"background","sessionId":"7301428b-8dd3","name":"Review","state":"blocked"},
                       {"pid":21444,"cwd":"C:\\dev\\wt","kind":"interactive","sessionId":"73d200ce-af0c","name":"wt-07","status":"idle"}]"#;
        let agents = parse(json);
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].pid, None);
        assert_eq!(agents[1].pid, Some(21444));
        assert_eq!(agents[1].folder(), "wt");
        assert!(parse("not json").is_empty());
    }

    #[test]
    fn a_terminal_with_one_session_is_that_session() {
        let agents = [agent("handy-8c", r"C:\dev\Handy", 100)];
        let found = pick(
            &agents,
            &tree(),
            &window("WindowsTerminal.exe", "HOST: other", 80),
        );
        assert_eq!(found.map(|a| a.name.as_str()), Some("handy-8c"));
    }

    #[test]
    fn an_editor_needs_the_folder_in_its_title() {
        let agents = [
            agent("a", r"C:\dev\wt-toolchains", 101),
            agent("b", r"C:\dev\Handy", 102),
        ];
        let editor = |title: &str| window("Code.exe", title, 70);
        let found = pick(
            &agents,
            &tree(),
            &editor("lib.rs - Handy - Visual Studio Code"),
        );
        assert_eq!(found.map(|a| a.name.as_str()), Some("b"));
        assert!(pick(
            &agents,
            &tree(),
            &editor("notes.md - other - Visual Studio Code")
        )
        .is_none());
        // One session in an editor is still not enough without its folder.
        assert!(pick(&agents[..1], &tree(), &editor("x - y - Visual Studio Code")).is_none());
    }

    /// Live: with Claude Code running in Windows Terminal (`cargo test --
    /// --ignored live`), finds that session from the terminal's process.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_finds_a_session_in_windows_terminal() {
        let parents = parents();
        let agents = list();
        let terminals: Vec<WindowInfo> = parents
            .keys()
            .filter_map(|&pid| {
                let exe = super::super::window::process_exe(pid)?;
                exe.eq_ignore_ascii_case("WindowsTerminal.exe")
                    .then(|| WindowInfo {
                        handle: 0,
                        pid,
                        exe,
                        title: String::new(),
                    })
            })
            .collect();
        let found: Vec<_> = terminals
            .iter()
            .filter_map(|t| pick(&agents, &parents, t))
            .map(|a| a.name.clone())
            .collect();
        println!(
            "{} sessions, {} terminals, picked {found:?}",
            agents.len(),
            terminals.len()
        );
        assert!(!found.is_empty());
    }

    #[test]
    fn a_macos_terminal_with_one_session_is_that_session() {
        let agents = [agent("handy-8c", "/Users/me/dev/Handy", 100)];
        let found = pick(&agents, &tree(), &window("Terminal", "", 80));
        assert_eq!(found.map(|a| a.name.as_str()), Some("handy-8c"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn knows_this_process_parent() {
        // SAFETY: a plain query.
        let parent = unsafe { libc::getppid() } as u32;
        assert_eq!(parents().get(&std::process::id()), Some(&parent));
    }

    #[test]
    fn sessions_elsewhere_never_match() {
        let agents = [agent("handy-8c", r"C:\dev\Handy", 100)];
        // A window whose program is not an ancestor, even with the folder named.
        assert!(pick(&agents, &tree(), &window("Code.exe", "Handy", 70)).is_none());
    }
}
