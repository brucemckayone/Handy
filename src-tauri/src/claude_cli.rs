//! Finding and running the `claude` command (Claude Code) without a window.
//! Shared by meeting summaries and Ask.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

/// Points Handy at another `claude`, such as a fake one in tests.
const OVERRIDE_VAR: &str = "HANDY_CLAUDE_PATH";

/// The `claude` command, from `HANDY_CLAUDE_PATH`, PATH, or where its
/// installers put it.
pub fn find_claude() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(OVERRIDE_VAR).map(PathBuf::from) {
        return path.is_file().then_some(path);
    }
    let names: &[&str] = if cfg!(windows) {
        &["claude.exe", "claude.cmd"]
    } else {
        &["claude"]
    };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    if let Some(home) = home.map(PathBuf::from) {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".claude").join("local"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("npm"));
    }
    dirs.into_iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
}

/// A command that runs `claude` (through `cmd /C` for an npm `.cmd` shim)
/// without flashing a console window.
pub fn command(claude: &Path) -> Command {
    let is_script = claude
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    let mut command = if is_script {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(claude);
        c
    } else {
        Command::new(claude)
    };
    crate::meeting::hide_console(&mut command);
    command
}

/// Ends `child` and everything it started. Killing only the `cmd` behind a
/// `.cmd` shim would leave Claude Code's own process running.
pub fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let mut taskkill = Command::new("taskkill");
        taskkill
            .args(["/T", "/F", "/PID"])
            .arg(child.id().to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        crate::meeting::hide_console(&mut taskkill);
        let _ = taskkill.status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[cfg(windows)]
    fn running(pid: u32) -> bool {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .expect("tasklist runs");
        String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
    }

    /// A `cmd` running a long `ping`: the `ping` must die too.
    #[cfg(windows)]
    #[test]
    fn kill_tree_ends_grandchildren() {
        let mut child = Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 >nul"])
            .spawn()
            .expect("cmd starts");
        let query = format!(
            "(Get-CimInstance Win32_Process -Filter 'ParentProcessId={}' | Where-Object Name -eq 'PING.EXE').ProcessId",
            child.id()
        );
        let mut grandchild = None;
        let started = Instant::now();
        while grandchild.is_none() && started.elapsed() < Duration::from_secs(10) {
            let out = Command::new("powershell")
                .args(["-NoProfile", "-Command", &query])
                .output()
                .expect("powershell runs");
            grandchild = String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse::<u32>()
                .ok();
        }
        let grandchild = grandchild.expect("ping started under cmd");
        assert!(running(grandchild));

        kill_tree(&mut child);

        let started = Instant::now();
        while running(grandchild) && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!running(grandchild), "ping survived kill_tree");
    }

    #[test]
    fn override_must_be_a_file() {
        // One test owns the variable, so parallel tests don't race on it.
        let dir = std::env::temp_dir().join("handy-claude-cli-test");
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("claude.cmd");
        std::fs::write(&fake, "@echo off\r\n").unwrap();

        std::env::set_var(OVERRIDE_VAR, &fake);
        assert_eq!(find_claude(), Some(fake.clone()));
        std::env::set_var(OVERRIDE_VAR, dir.join("missing.cmd"));
        assert_eq!(find_claude(), None);
        std::env::remove_var(OVERRIDE_VAR);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
