//! Runs one question through headless Claude Code and reads the answer as
//! it streams (`--output-format stream-json`).
//!
//! Launched lean: no MCP servers and no user settings or hooks, which brings
//! the first words from ~5 s down to ~2 s. Claude may read and search files
//! and the web; anything else is refused without asking anyone.

use anyhow::{anyhow, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::claude_cli;

/// Read, search and the web: nothing that edits files or runs commands.
pub const TOOLS: &str = "Read,Grep,Glob,WebSearch,WebFetch";
/// Past this a question is given up and Claude stopped.
pub const TIMEOUT: Duration = Duration::from_secs(3 * 60);
/// How often the wait checks for a cancel.
const POLL: Duration = Duration::from_millis(100);
/// How long Claude Code gets to exit by itself after its answer.
const EXIT_GRACE: Duration = Duration::from_secs(10);

/// Which conversation the question goes into.
#[derive(Debug, Clone, PartialEq)]
pub enum Session {
    /// A new conversation with this id.
    New(String),
    /// More of this conversation.
    Resume(String),
    /// A copy of this one (a running agent's), which stays untouched.
    Fork(String),
}

pub struct Request {
    pub prompt: String,
    pub session: Session,
    /// `None` keeps the conversation's own model (agent forks).
    pub model: Option<String>,
    pub cwd: PathBuf,
    /// Folders Claude may read besides `cwd`.
    pub add_dirs: Vec<PathBuf>,
}

/// What one line of the stream says.
#[derive(Debug, PartialEq)]
pub enum Line {
    /// A new message starts: text before it was narration around a tool.
    MessageStart,
    /// More of the reply's text.
    Text(String),
    /// The finished reply.
    Done(Finished),
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    pub session_id: Option<String>,
    pub text: String,
    pub is_error: bool,
}

pub fn parse_line(line: &str) -> Line {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Line::Other;
    };
    match value["type"].as_str() {
        Some("stream_event") => {
            let event = &value["event"];
            match event["type"].as_str() {
                Some("message_start") => Line::MessageStart,
                Some("content_block_delta") if event["delta"]["type"] == "text_delta" => event
                    ["delta"]["text"]
                    .as_str()
                    .map_or(Line::Other, |t| Line::Text(t.to_string())),
                _ => Line::Other,
            }
        }
        Some("result") => Line::Done(Finished {
            session_id: value["session_id"].as_str().map(str::to_string),
            text: value["result"].as_str().unwrap_or_default().to_string(),
            is_error: value["is_error"].as_bool().unwrap_or(false),
        }),
        _ => Line::Other,
    }
}

/// The command-line arguments for `request`.
pub fn args(request: &Request) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--strict-mcp-config",
        "--setting-sources",
        "project,local",
        "--tools",
        TOOLS,
        "--allowedTools",
        TOOLS,
        "--permission-prompts",
        "none",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(model) = &request.model {
        args.extend(["--model".to_string(), model.clone()]);
    }
    match &request.session {
        Session::New(id) => args.extend(["--session-id".to_string(), id.clone()]),
        Session::Resume(id) => args.extend(["--resume".to_string(), id.clone()]),
        Session::Fork(id) => args.extend([
            "--resume".to_string(),
            id.clone(),
            "--fork-session".to_string(),
        ]),
    }
    for dir in &request.add_dirs {
        args.extend(["--add-dir".to_string(), dir.display().to_string()]);
    }
    args
}

/// Why a question got no answer.
#[derive(Debug, PartialEq)]
pub enum RunError {
    NotFound,
    TimedOut,
    Cancelled,
    Failed(String),
}

/// Asks Claude Code, calling `on_line` for each line as it streams.
/// `cancelled` is checked while it waits.
pub fn run(
    request: &Request,
    cancelled: impl Fn() -> bool,
    on_line: impl FnMut(&Line),
) -> Result<Finished, RunError> {
    let claude = claude_cli::find_claude().ok_or(RunError::NotFound)?;
    run_with(&claude, request, cancelled, on_line)
}

fn run_with(
    claude: &std::path::Path,
    request: &Request,
    cancelled: impl Fn() -> bool,
    mut on_line: impl FnMut(&Line),
) -> Result<Finished, RunError> {
    let mut child = claude_cli::command(claude)
        .args(args(request))
        .current_dir(&request.cwd)
        // Lets `--add-dir` folders bring their CLAUDE.md with them.
        .env("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| RunError::Failed(e.to_string()))?;

    let spawned = (|| -> Result<_> {
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("No stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("No stdout"))?;
        let mut stderr = child.stderr.take().ok_or_else(|| anyhow!("No stderr"))?;
        let prompt = request.prompt.clone();
        // Written on its own thread: a full pipe must not hold up the reader.
        std::thread::spawn(move || {
            let _ = stdin.write_all(prompt.as_bytes());
        });
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).split(b'\n').map_while(|l| l.ok()) {
                let line = String::from_utf8_lossy(&line).trim_end().to_string();
                if !line.is_empty() && tx.send(line).is_err() {
                    break;
                }
            }
        });
        let err_reader = std::thread::spawn(move || {
            let mut out = String::new();
            let _ = stderr.read_to_string(&mut out);
            out
        });
        Ok((rx, err_reader))
    })();
    let (rx, err_reader) = match spawned {
        Ok(parts) => parts,
        Err(e) => {
            claude_cli::kill_tree(&mut child);
            return Err(RunError::Failed(e.to_string()));
        }
    };

    let started = Instant::now();
    let outcome = loop {
        if cancelled() {
            break Err(RunError::Cancelled);
        }
        if started.elapsed() > TIMEOUT {
            break Err(RunError::TimedOut);
        }
        match rx.recv_timeout(POLL) {
            Ok(line) => {
                let parsed = parse_line(&line);
                on_line(&parsed);
                if let Line::Done(finished) = parsed {
                    // The process lingers ~1 s after its result; don't wait.
                    break Ok(finished);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = child.wait();
                let err = err_reader.join().unwrap_or_default();
                let last = err.lines().rev().find(|l| !l.trim().is_empty());
                break Err(RunError::Failed(
                    last.unwrap_or("Claude Code stopped without an answer")
                        .trim()
                        .to_string(),
                ));
            }
        }
    };
    match outcome {
        // It is saving the conversation; give it a moment to exit.
        Ok(_) => {
            std::thread::spawn(move || {
                let started = Instant::now();
                while started.elapsed() < EXIT_GRACE {
                    if child.try_wait().ok().flatten().is_some() {
                        return;
                    }
                    std::thread::sleep(POLL);
                }
                claude_cli::kill_tree(&mut child);
            });
        }
        Err(RunError::Failed(_)) => {}
        Err(_) => claude_cli::kill_tree(&mut child),
    }
    match outcome {
        Ok(finished) if finished.is_error => Err(RunError::Failed(finished.text)),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<Line> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ask")
            .join(name);
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(parse_line)
            .collect()
    }

    fn text_of(lines: &[Line]) -> String {
        lines
            .iter()
            .filter_map(|l| match l {
                Line::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn streams_text_then_the_result() {
        let lines = fixture("stream_new_thread.jsonl");
        assert_eq!(
            lines.iter().filter(|l| **l == Line::MessageStart).count(),
            1
        );
        let Some(Line::Done(done)) = lines.last() else {
            panic!("no result line");
        };
        assert!(!done.is_error);
        assert_eq!(
            done.session_id.as_deref(),
            Some("d4c3a8fb-8b93-4451-aba7-07e53d0cec67")
        );
        // The streamed text adds up to the final answer.
        assert_eq!(text_of(&lines), done.text);
        assert!(done
            .text
            .starts_with("Edinburgh is the capital of Scotland.\n---\n"));
    }

    #[test]
    fn a_resumed_session_keeps_its_id() {
        let lines = fixture("stream_resume.jsonl");
        let Some(Line::Done(done)) = lines.last() else {
            panic!("no result line");
        };
        assert_eq!(
            done.session_id.as_deref(),
            Some("d4c3a8fb-8b93-4451-aba7-07e53d0cec67")
        );
        assert_eq!(done.text, "Cardiff is the capital of Wales.");
    }

    #[test]
    fn ignores_lines_that_are_not_json() {
        assert_eq!(parse_line("not json"), Line::Other);
        assert_eq!(
            parse_line(r#"{"type":"system","subtype":"init"}"#),
            Line::Other
        );
    }

    fn request(cwd: PathBuf) -> Request {
        Request {
            prompt: "Question: hi".into(),
            session: Session::New("x".into()),
            model: Some("haiku".into()),
            cwd,
            add_dirs: Vec::new(),
        }
    }

    /// A stand-in `claude` that runs `body` as a batch file.
    fn fake_claude(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("handy-ask-fake-claude");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.cmd"));
        std::fs::write(&path, format!("@echo off\r\n{body}\r\n")).unwrap();
        path
    }

    #[cfg(windows)]
    #[test]
    fn runs_a_fake_claude_to_the_end() {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ask");
        let claude = fake_claude(
            "streams",
            &format!(
                "type \"{}\"",
                fixtures.join("stream_new_thread.jsonl").display()
            ),
        );
        let mut texts = 0;
        let finished = run_with(
            &claude,
            &request(std::env::temp_dir()),
            || false,
            |line| {
                if matches!(line, Line::Text(_)) {
                    texts += 1;
                }
            },
        )
        .expect("an answer");
        assert!(texts > 5);
        assert!(finished.text.starts_with("Edinburgh"));
    }

    #[cfg(windows)]
    #[test]
    fn a_cancel_stops_a_slow_claude() {
        let claude = fake_claude("slow", "ping -n 30 127.0.0.1 >nul");
        let started = Instant::now();
        let outcome = run_with(
            &claude,
            &request(std::env::temp_dir()),
            || started.elapsed() > Duration::from_millis(300),
            |_| {},
        );
        assert_eq!(outcome, Err(RunError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[cfg(windows)]
    #[test]
    fn a_claude_that_dies_reports_its_last_words() {
        let claude = fake_claude("dies", "echo Not logged in 1>&2\r\nexit /b 1");
        let outcome = run_with(&claude, &request(std::env::temp_dir()), || false, |_| {});
        assert_eq!(outcome, Err(RunError::Failed("Not logged in".into())));
    }

    #[test]
    fn launches_lean_with_read_only_tools() {
        let request = Request {
            prompt: String::new(),
            session: Session::Fork("abc".into()),
            model: None,
            cwd: PathBuf::from("."),
            add_dirs: vec![PathBuf::from("proj")],
        };
        let args = args(&request);
        let joined = args.join(" ");
        assert!(joined.contains("--strict-mcp-config --setting-sources project,local"));
        assert!(joined.contains(&format!("--tools {TOOLS} --allowedTools {TOOLS}")));
        assert!(joined.contains("--permission-prompts none"));
        assert!(joined.contains("--resume abc --fork-session"));
        assert!(joined.ends_with("--add-dir proj"));
        assert!(!joined.contains("--model"), "forks keep the agent's model");
    }
}
