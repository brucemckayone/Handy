//! Hands `handy --speak` text (and `handy --ask` questions, in their own
//! queue) to the running Handy through files. The
//! single-instance hand-off splits arguments at `|`, which markdown tables
//! are full of, and command lines are short; so the launched process saves
//! the text here and the running one picks it up when the flag arrives.
//!
//! These flags come from scripts and agents' hooks, so on Windows they never
//! start Handy (that process would never exit, and the caller would wait on
//! it). Elsewhere a launched Handy reads its own text once it's up.

use std::io::Read;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli::CliArgs;

/// More than enough: reading stops at the word limit long before this.
const MAX_BYTES: u64 = 1 << 20;
const SPEAK_FLAGS: &[&str] = &[
    "--speak",
    "--speak-file",
    "--speak-stdin",
    "--speak-claude-hook",
];
/// tauri.conf.json's `identifier`, which names the single-instance window.
#[cfg(windows)]
const IDENTIFIER: &str = "com.pais.handy";

/// Which queue a saved text waits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Queue {
    /// Text to read aloud.
    Speech,
    /// Questions for Ask (`handy --ask`).
    Ask,
}

impl Queue {
    fn dir(self) -> PathBuf {
        std::env::temp_dir().join(match self {
            Queue::Speech => "handy-speech",
            Queue::Ask => "handy-ask-queue",
        })
    }
}

/// Whether the command line asks for something to be read.
pub fn requested(args: &CliArgs) -> bool {
    args.speak.is_some() || args.speak_file.is_some() || args.speak_stdin || args.speak_claude_hook
}

/// Whether a Handy is running to hand the text to: looks for the window
/// tauri-plugin-single-instance listens on.
#[cfg(windows)]
pub fn handy_running() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    let wide = |s: String| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let class = wide(format!("{IDENTIFIER}-sic"));
    let name = wide(format!("{IDENTIFIER}-siw"));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR(name.as_ptr())) }
        .is_ok_and(|hwnd| !hwnd.is_invalid())
}

/// Elsewhere the launched Handy simply becomes the running one.
#[cfg(not(windows))]
pub fn handy_running() -> bool {
    true
}

/// Whether forwarded arguments ask for something to be read.
pub fn requested_in(args: &[String]) -> bool {
    args.iter().any(|arg| {
        SPEAK_FLAGS
            .iter()
            .any(|flag| arg == flag || arg.starts_with(&format!("{flag}=")))
    })
}

/// In the process that got the flags: saves the text to be read.
pub fn save(args: &CliArgs) -> std::io::Result<()> {
    let text = if let Some(text) = &args.speak {
        text.clone()
    } else if let Some(path) = &args.speak_file {
        let mut text = String::new();
        std::fs::File::open(path)?
            .take(MAX_BYTES)
            .read_to_string(&mut text)?;
        text
    } else if args.speak_stdin {
        read_stdin()?
    } else if args.speak_claude_hook {
        match claude_hook_text(&read_stdin()?) {
            Some(text) => text,
            None => return Ok(()),
        }
    } else {
        return Ok(());
    };
    save_text(Queue::Speech, &text)
}

/// Saves `text` in `queue` for the running Handy to pick up.
pub fn save_text(queue: Queue, text: &str) -> std::io::Result<()> {
    let dir = queue.dir();
    std::fs::create_dir_all(&dir)?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    // Names sort in arrival order; written aside, then renamed, so a
    // half-written file is never read.
    let name = format!("{nanos:024}-{}", std::process::id());
    let partial = dir.join(format!("{name}.partial"));
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, dir.join(format!("{name}.txt")))
}

pub(crate) fn read_stdin() -> std::io::Result<String> {
    let mut text = String::new();
    std::io::stdin().take(MAX_BYTES).read_to_string(&mut text)?;
    Ok(text)
}

/// What to read from a Claude Code hook's input: Claude's last reply when it
/// stops, or the notice when it needs the user.
fn claude_hook_text(json: &str) -> Option<String> {
    let input: serde_json::Value = serde_json::from_str(json).ok()?;
    let field = match input.get("hook_event_name")?.as_str()? {
        "Stop" => "last_assistant_message",
        "Notification" => "message",
        _ => return None,
    };
    let text = input.get(field)?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Takes the saved texts to read, oldest first.
pub fn take() -> Vec<String> {
    take_from(Queue::Speech)
}

/// Takes the texts saved in `queue`, oldest first.
pub fn take_from(queue: Queue) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(queue.dir()) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "txt"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok();
            let _ = std::fs::remove_file(&path);
            text
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_speak_flags_in_forwarded_arguments() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(requested_in(&args(&["handy", "--speak", "hi"])));
        assert!(requested_in(&args(&["handy", "--speak-file=notes.md"])));
        assert!(requested_in(&args(&["handy", "--speak-stdin"])));
        assert!(requested_in(&args(&["handy", "--speak-claude-hook"])));
        assert!(!requested_in(&args(&["handy", "--stop-speaking"])));
        assert!(!requested_in(&args(&["handy", "--speaker"])));
    }

    #[test]
    fn reads_claude_replies_and_notices_only() {
        let stop = r#"{"hook_event_name":"Stop","last_assistant_message":" Done. ","stop_hook_active":false}"#;
        assert_eq!(claude_hook_text(stop).as_deref(), Some("Done."));
        let notice =
            r#"{"hook_event_name":"Notification","message":"Claude needs your permission"}"#;
        assert_eq!(
            claude_hook_text(notice).as_deref(),
            Some("Claude needs your permission")
        );
        assert_eq!(
            claude_hook_text(r#"{"hook_event_name":"SubagentStop","last_assistant_message":"x"}"#),
            None
        );
        assert_eq!(
            claude_hook_text(r#"{"hook_event_name":"Stop","last_assistant_message":""}"#),
            None
        );
        assert_eq!(claude_hook_text("not json"), None);
    }

    #[cfg(windows)]
    #[test]
    fn identifier_matches_the_tauri_config() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], IDENTIFIER);
    }
}
