//! Finding and running the `claude` command (Claude Code) without a window.
//! Shared by meeting summaries and Ask.

use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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

/// Asks `claude` once and returns its reply's text: `args` must include
/// `-p --output-format json`; `prompt` goes on stdin; `cwd` is where it
/// runs. Past `timeout` it is stopped. A missing `claude` is the error
/// `claude_not_found`, which settings pages look for.
pub fn run_json(args: &[&str], cwd: &Path, prompt: &str, timeout: Duration) -> Result<String> {
    let claude = find_claude().ok_or_else(|| anyhow!("claude_not_found"))?;
    run_json_with(&claude, args, cwd, prompt, timeout)
}

fn run_json_with(
    claude: &Path,
    args: &[&str],
    cwd: &Path,
    prompt: &str,
    timeout: Duration,
) -> Result<String> {
    let mut child = command(claude)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("No stdin"))?;
    let input = prompt.to_string();
    // Written on its own thread: a full pipe must not hold up the readers.
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let mut stdout = child.stdout.take().ok_or_else(|| anyhow!("No stdout"))?;
    let mut stderr = child.stderr.take().ok_or_else(|| anyhow!("No stderr"))?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        std::io::Read::read_to_string(&mut stdout, &mut out).map(|_| out)
    });
    let err_reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = std::io::Read::read_to_string(&mut stderr, &mut out);
        out
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > timeout {
            kill_tree(&mut child);
            return Err(anyhow!("Claude Code took too long"));
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    let _ = writer.join();
    let out = reader
        .join()
        .map_err(|_| anyhow!("Claude Code's output was lost"))??;
    let err = err_reader.join().unwrap_or_default();
    if !status.success() && out.trim().is_empty() {
        return Err(anyhow!(
            "Claude Code failed: {}",
            err.lines().last().unwrap_or("no output")
        ));
    }
    #[derive(Deserialize)]
    struct ClaudeResult {
        result: Option<String>,
        #[serde(default)]
        is_error: bool,
    }
    let parsed: ClaudeResult = serde_json::from_str(out.trim())
        .map_err(|e| anyhow!("Unexpected output from Claude Code: {}", e))?;
    match parsed.result {
        Some(result) if !parsed.is_error => Ok(result),
        Some(result) => Err(anyhow!("Claude Code failed: {}", result)),
        None => Err(anyhow!("Claude Code gave no answer")),
    }
}

/// A stand-in `claude` for tests that runs `body` as a batch file. Each
/// test names its own, so tests running at once don't share one.
#[cfg(all(test, windows))]
pub(crate) fn fake_claude(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("handy-fake-claude");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.cmd"));
    std::fs::write(&path, format!("@echo off\r\n{body}\r\n")).unwrap();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn run_json_reads_the_reply_and_its_errors() {
        let run = |claude: &Path| {
            run_json_with(
                claude,
                &["-p"],
                &std::env::temp_dir(),
                "hi",
                Duration::from_secs(30),
            )
        };
        let ok = fake_claude("ok", r#"echo {"result":"Hello","is_error":false}"#);
        assert_eq!(run(&ok).unwrap(), "Hello");
        let refused = fake_claude(
            "refused",
            r#"echo {"result":"Not logged in","is_error":true}"#,
        );
        assert_eq!(
            run(&refused).unwrap_err().to_string(),
            "Claude Code failed: Not logged in"
        );
        let crashed = fake_claude("crashed", ">&2 echo boom\r\nexit /b 3");
        assert_eq!(
            run(&crashed).unwrap_err().to_string(),
            "Claude Code failed: boom"
        );
    }

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
