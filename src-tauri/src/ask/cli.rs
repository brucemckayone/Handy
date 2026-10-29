//! `handy --ask` and `--ask-stdin`: the launched process saves the question
//! for the running Handy, as `--speak` does (see [`crate::speech::spool`]).
//!
//! The folder it was run in goes along, so Claude can read that project.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::prompt::ScreenContext;
use super::store::Via;
use super::NewQuestion;
use crate::cli::CliArgs;
use crate::speech::spool::{self, Queue};

const ASK_FLAGS: &[&str] = &["--ask", "--ask-stdin"];

/// A question saved for the running Handy.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub question: String,
    /// Text piped in alongside a `--ask` question.
    #[serde(default)]
    pub input: Option<String>,
    /// The folder `handy --ask` was run in.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

impl Saved {
    /// From the command line's question and piped text: piped text alone is
    /// the question; with `--ask` too it is what the question is about.
    fn from_parts(ask: Option<&str>, piped: Option<String>, cwd: Option<PathBuf>) -> Option<Self> {
        let piped = piped.filter(|p| !p.trim().is_empty());
        let (question, input) = match (ask.map(str::trim).filter(|a| !a.is_empty()), piped) {
            (Some(question), input) => (question.to_string(), input),
            (None, Some(piped)) => (piped.trim().to_string(), None),
            (None, None) => return None,
        };
        Some(Self {
            question,
            input,
            cwd,
        })
    }

    /// Reads a saved question; older saves were the bare question.
    fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_else(|_| Self {
            question: text.to_string(),
            input: None,
            cwd: None,
        })
    }

    pub fn into_question(self) -> NewQuestion {
        NewQuestion {
            project_dir: self.cwd.filter(|dir| is_project(dir)),
            screen: ScreenContext {
                input: self.input,
                ..ScreenContext::default()
            },
            ..NewQuestion::text(self.question, Via::Command, None)
        }
    }
}

/// A folder worth letting Claude read: not a drive root or the home folder.
fn is_project(dir: &Path) -> bool {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    dir.parent().is_some() && home.as_deref() != Some(dir) && dir.is_dir()
}

/// Whether the command line asks a question.
pub fn requested(args: &CliArgs) -> bool {
    args.ask.is_some() || args.ask_stdin
}

/// Whether forwarded arguments ask a question.
pub fn requested_in(args: &[String]) -> bool {
    args.iter().any(|arg| {
        ASK_FLAGS
            .iter()
            .any(|flag| arg == flag || arg.starts_with(&format!("{flag}=")))
    })
}

/// In the process that got the flags: saves the question.
pub fn save(args: &CliArgs) -> std::io::Result<()> {
    let piped = if args.ask_stdin {
        Some(spool::read_stdin()?)
    } else {
        None
    };
    let cwd = std::env::current_dir().ok();
    let Some(saved) = Saved::from_parts(args.ask.as_deref(), piped, cwd) else {
        return Ok(());
    };
    let json = serde_json::to_string(&saved).map_err(std::io::Error::other)?;
    spool::save_text(Queue::Ask, &json)
}

/// Takes the saved questions, oldest first.
pub fn take() -> Vec<Saved> {
    spool::take_from(Queue::Ask)
        .iter()
        .map(|text| Saved::parse(text))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_ask_flags_in_forwarded_arguments() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(requested_in(&args(&["handy", "--ask", "why?"])));
        assert!(requested_in(&args(&["handy", "--ask=why?"])));
        assert!(requested_in(&args(&["handy", "--ask-stdin"])));
        assert!(!requested_in(&args(&["handy", "--asking"])));
        assert!(!requested_in(&args(&["handy", "--speak", "hi"])));
    }

    #[test]
    fn piped_text_is_the_question_or_its_input() {
        let both = Saved::from_parts(Some(" Review this "), Some("diff".into()), None).unwrap();
        assert_eq!(
            (both.question.as_str(), both.input.as_deref()),
            ("Review this", Some("diff"))
        );
        let piped = Saved::from_parts(None, Some(" Why? \n".into()), None).unwrap();
        assert_eq!((piped.question.as_str(), piped.input), ("Why?", None));
        assert_eq!(Saved::from_parts(None, Some("  ".into()), None), None);
        assert_eq!(Saved::from_parts(Some(""), None, None), None);
    }

    #[test]
    fn reads_saved_json_and_bare_text() {
        let saved = Saved {
            question: "q".into(),
            input: Some("i".into()),
            cwd: Some(PathBuf::from("C:/dev")),
        };
        let json = serde_json::to_string(&saved).unwrap();
        assert_eq!(Saved::parse(&json), saved);
        assert_eq!(Saved::parse("plain question").question, "plain question");
    }

    #[test]
    fn home_and_roots_are_not_projects() {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap();
        assert!(!is_project(&home));
        assert!(!is_project(Path::new("C:\\")));
        assert!(is_project(Path::new(env!("CARGO_MANIFEST_DIR"))));
    }
}
