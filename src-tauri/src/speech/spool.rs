//! Hands `handy --speak` text to the running Handy through files. The
//! single-instance hand-off splits arguments at `|`, which markdown tables
//! are full of, and command lines are short; so the launched process saves
//! the text here and the running one picks it up when the flag arrives.

use std::io::Read;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli::CliArgs;

/// More than enough: reading stops at the word limit long before this.
const MAX_BYTES: u64 = 1 << 20;
const SPEAK_FLAGS: &[&str] = &["--speak", "--speak-file", "--speak-stdin"];

fn dir() -> PathBuf {
    std::env::temp_dir().join("handy-speech")
}

/// Whether the command line asks for something to be read.
pub fn requested(args: &CliArgs) -> bool {
    args.speak.is_some() || args.speak_file.is_some() || args.speak_stdin
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
        let mut text = String::new();
        std::io::stdin().take(MAX_BYTES).read_to_string(&mut text)?;
        text
    } else {
        return Ok(());
    };
    let dir = dir();
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

/// Takes the saved texts, oldest first.
pub fn take() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir()) else {
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
        assert!(!requested_in(&args(&["handy", "--stop-speaking"])));
        assert!(!requested_in(&args(&["handy", "--speaker"])));
    }
}
