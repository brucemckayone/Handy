//! What was read aloud in each window lately, so a question asked there
//! knows about it ("what did that paragraph mean?"). Kept in memory only.

use once_cell::sync::Lazy;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::store::WindowKey;

/// Older reads are forgotten.
const KEEP: Duration = Duration::from_secs(30 * 60);
/// At most this many reads per window go with a question.
const PER_WINDOW: usize = 3;

struct Read {
    window: WindowKey,
    text: String,
    at: Instant,
}

static READS: Lazy<Mutex<Vec<Read>>> = Lazy::new(|| Mutex::new(Vec::new()));

fn prune(reads: &mut Vec<Read>, now: Instant) {
    reads.retain(|r| now.duration_since(r.at) < KEEP);
}

/// Remembers that `text` was read aloud in `window`.
pub fn remember(window: WindowKey, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let now = Instant::now();
    if let Ok(mut reads) = READS.lock() {
        prune(&mut reads, now);
        reads.push(Read {
            window,
            text: text.to_string(),
            at: now,
        });
    }
}

/// The latest reads in `window`, oldest first, with how many minutes ago.
pub fn for_window(window: &WindowKey) -> Vec<(String, u64)> {
    let now = Instant::now();
    let Ok(mut reads) = READS.lock() else {
        return Vec::new();
    };
    prune(&mut reads, now);
    let mut found: Vec<(String, u64)> = reads
        .iter()
        .rev()
        .filter(|r| &r.window == window)
        .take(PER_WINDOW)
        .map(|r| (r.text.clone(), now.duration_since(r.at).as_secs() / 60))
        .collect();
    found.reverse();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(handle: &str) -> WindowKey {
        WindowKey {
            handle: handle.into(),
            exe: "Test.exe".into(),
        }
    }

    #[test]
    fn keeps_reads_per_window_newest_three() {
        // Handles unique to this test: the store is shared by the process.
        for i in 0..5 {
            remember(key("recent-test-a"), &format!("read {i}"));
        }
        remember(key("recent-test-b"), "other window");
        remember(key("recent-test-a"), "   ");
        let reads: Vec<String> = for_window(&key("recent-test-a"))
            .into_iter()
            .map(|(text, minutes)| {
                assert_eq!(minutes, 0);
                text
            })
            .collect();
        assert_eq!(reads, ["read 2", "read 3", "read 4"]);
        assert!(for_window(&key("recent-test-none")).is_empty());
    }
}
