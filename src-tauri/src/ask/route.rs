//! Which thread a spoken question goes into, so nobody has to manage
//! conversations: the same window, asked again soon, continues its thread;
//! anything else starts a new one, as does starting with "new topic".

use super::store::{Thread, WindowKey};

/// Spoken openers that start a new thread; removed from the question.
const NEW_TOPIC: &[&str] = &["new topic", "new question", "new thread"];

#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Thread(String),
    New,
}

/// `question` without a leading "new topic", if it had one.
pub fn strip_new_topic(question: &str) -> Option<&str> {
    let trimmed = question.trim_start();
    NEW_TOPIC.iter().find_map(|opener| {
        let head = trimmed.get(..opener.len())?;
        if !head.eq_ignore_ascii_case(opener) {
            return None;
        }
        let rest = &trimmed[opener.len()..];
        // A whole opener, not "new topics are…".
        if rest.chars().next().is_some_and(|c| c.is_alphanumeric()) {
            return None;
        }
        Some(rest.trim_start_matches(|c: char| c.is_whitespace() || ",.:;!-".contains(c)))
    })
}

/// Where a spoken question from `window` goes. `threads` may be in any
/// order; `same_window_minutes` 0 means no time limit.
pub fn route(
    question: &str,
    window: Option<&WindowKey>,
    threads: &[Thread],
    now_ms: i64,
    same_window_minutes: u32,
) -> (Route, String) {
    if let Some(rest) = strip_new_topic(question) {
        return (Route::New, rest.to_string());
    }
    let question = question.trim().to_string();
    let Some(window) = window else {
        return (Route::New, question);
    };
    let fresh = |t: &&Thread| {
        same_window_minutes == 0 || now_ms - t.last_used <= i64::from(same_window_minutes) * 60_000
    };
    let found = threads
        .iter()
        .filter(|t| t.window.as_ref() == Some(window))
        .filter(fresh)
        .max_by_key(|t| t.last_used);
    match found {
        Some(thread) => (Route::Thread(thread.id.clone()), question),
        None => (Route::New, question),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(handle: &str, exe: &str) -> WindowKey {
        WindowKey {
            handle: handle.into(),
            exe: exe.into(),
        }
    }

    fn thread(id: &str, window: Option<WindowKey>, last_used: i64) -> Thread {
        Thread {
            id: id.into(),
            title: id.into(),
            title_is_suggested: true,
            created_at: last_used,
            last_used,
            window,
            app_label: None,
            project_dir: None,
            agent: None,
            claude_session: None,
            sessions: Vec::new(),
            merge: None,
            kept_apart: Vec::new(),
            exchanges: Vec::new(),
        }
    }

    const MIN: i64 = 60_000;

    #[test]
    fn the_same_window_soon_after_continues() {
        let threads = [
            thread("old", Some(key("1", "Code.exe")), 0),
            thread("recent", Some(key("1", "Code.exe")), 50 * MIN),
            thread("other", Some(key("2", "Code.exe")), 55 * MIN),
        ];
        let (route, text) = route(
            " What now? ",
            Some(&key("1", "Code.exe")),
            &threads,
            60 * MIN,
            60,
        );
        assert_eq!(route, Route::Thread("recent".into()));
        assert_eq!(text, "What now?");
    }

    #[test]
    fn too_long_ago_or_another_program_starts_fresh() {
        let threads = [thread("t", Some(key("1", "Code.exe")), 0)];
        let late = route("q", Some(&key("1", "Code.exe")), &threads, 61 * MIN, 60).0;
        assert_eq!(late, Route::New);
        // A reused handle under another program is another window.
        let reused = route("q", Some(&key("1", "notepad.exe")), &threads, MIN, 60).0;
        assert_eq!(reused, Route::New);
        // 0: as long as the window is open.
        let forever = route("q", Some(&key("1", "Code.exe")), &threads, 999 * MIN, 0).0;
        assert_eq!(forever, Route::Thread("t".into()));
    }

    #[test]
    fn new_topic_starts_a_thread_and_is_dropped() {
        let threads = [thread("t", Some(key("1", "Code.exe")), 0)];
        let (route, text) = route(
            "New topic, what's the weather in Leeds?",
            Some(&key("1", "Code.exe")),
            &threads,
            MIN,
            60,
        );
        assert_eq!(route, Route::New);
        assert_eq!(text, "what's the weather in Leeds?");
    }

    #[test]
    fn only_a_whole_opener_counts() {
        assert_eq!(strip_new_topic("New question: why?"), Some("why?"));
        assert_eq!(strip_new_topic("new thread"), Some(""));
        assert_eq!(strip_new_topic("New topics in Rust?"), None);
        assert_eq!(strip_new_topic("Is this a new topic?"), None);
    }

    #[test]
    fn no_window_means_a_new_thread() {
        let threads = [thread("t", None, 0)];
        assert_eq!(route("q", None, &threads, 0, 60).0, Route::New);
    }
}
