//! Ask threads, in their own `asks.db`. A thread is small and always read
//! and written whole, so each is one JSON document beside its list columns.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use crate::speech::SpeakRefusal;

/// How a question was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    Voice,
    Typed,
    /// `handy --ask` from a script or another tool.
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AnswerStatus {
    /// Waiting for the first words.
    Thinking,
    Streaming,
    Done,
    Failed,
    /// Stopped by a newer question or by the user.
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AskErrorCode {
    ClaudeNotFound,
    TimedOut,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct AskError {
    pub code: AskErrorCode,
    pub detail: Option<String>,
}

/// What went with a question, shown as chips under it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct ContextNote {
    pub selection_lines: Option<u32>,
    /// Text piped in with `handy --ask`.
    pub input_lines: Option<u32>,
    pub screenshot: bool,
    /// Something was read aloud in that window shortly before.
    pub read_aloud: bool,
    /// The running agent whose conversation was copied.
    pub agent: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct Question {
    pub id: String,
    pub text: String,
    pub via: Via,
    pub at: i64,
    #[serde(default)]
    pub context: ContextNote,
    /// Everything sent to Claude Code, screen context included, so a
    /// question moved to another thread can be asked there the same way.
    #[serde(default)]
    pub prompt: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct Answer {
    pub id: String,
    pub text: String,
    pub status: AnswerStatus,
    pub at: i64,
    pub model: Option<String>,
    /// From asking to the first words, in milliseconds.
    pub first_words_ms: Option<u32>,
    pub error: Option<AskError>,
    /// Why it wasn't read aloud, when it wasn't.
    #[serde(default)]
    pub not_spoken: Option<SpeakRefusal>,
    /// Some of it was read aloud.
    #[serde(default)]
    pub spoken: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct Exchange {
    pub question: Question,
    pub answer: Answer,
}

/// The window a thread belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WindowKey {
    /// The window handle, as a number; only meaningful while it is open.
    pub handle: String,
    /// The program's file name, e.g. `Code.exe`.
    pub exe: String,
}

/// A running Claude Code session a thread answers from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct AgentLink {
    pub session_id: String,
    pub name: String,
    pub cwd: String,
    /// Off once the user unlinks it: later questions start fresh.
    pub linked: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct Thread {
    pub id: String,
    pub title: String,
    /// Replaced by a better one in the background tidy-up.
    pub title_is_suggested: bool,
    pub created_at: i64,
    pub last_used: i64,
    pub window: Option<WindowKey>,
    /// The app and window title it started in, for the header.
    pub app_label: Option<String>,
    pub project_dir: Option<String>,
    pub agent: Option<AgentLink>,
    /// Handy's own Claude Code conversation for this thread.
    pub claude_session: Option<String>,
    /// Every Claude Code session Handy made for it, deleted with it.
    #[serde(default)]
    pub sessions: Vec<String>,
    /// Another thread the tidy-up thinks is about the same thing.
    #[serde(default)]
    pub merge: Option<MergeSuggestion>,
    /// Threads the user chose to keep apart from this one.
    #[serde(default)]
    pub kept_apart: Vec<String>,
    pub exchanges: Vec<Exchange>,
}

#[cfg(test)]
impl Thread {
    /// An empty thread with nothing set, for tests to fill in.
    pub fn blank(id: &str, last_used: i64) -> Self {
        Self {
            id: id.into(),
            title: id.into(),
            title_is_suggested: true,
            created_at: last_used,
            last_used,
            window: None,
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
}

/// A suggestion to merge another thread into this one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct MergeSuggestion {
    pub thread_id: String,
    pub title: String,
}

/// A thread in the switcher.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct ThreadListItem {
    pub id: String,
    pub title: String,
    pub app_label: Option<String>,
    pub last_used: i64,
}

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS threads (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    app_label TEXT,
    last_used INTEGER NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS threads_last_used ON threads (last_used);";

pub struct AskStore {
    conn: Mutex<Connection>,
}

impl AskStore {
    pub fn open(path: &Path) -> Result<Self> {
        Self::with(Connection::open(path)?)
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        Self::with(Connection::open_in_memory()?)
    }

    fn with(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Adds the thread, or replaces the one with its id.
    pub fn save(&self, thread: &Thread) -> Result<()> {
        Self::write(&self.lock(), thread)
    }

    fn write(conn: &Connection, t: &Thread) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO threads (id, title, app_label, last_used, data) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                t.id,
                t.title,
                t.app_label,
                t.last_used,
                serde_json::to_string(t)?
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Option<Thread>> {
        Self::read(&self.lock(), id)
    }

    fn read(conn: &Connection, id: &str) -> Result<Option<Thread>> {
        conn.query_row("SELECT data FROM threads WHERE id = ?1", [id], |row| {
            row.get::<_, String>(0)
        })
        .optional()?
        .map(|data| {
            serde_json::from_str(&data).map_err(|e| anyhow!("Thread {} is unreadable: {}", id, e))
        })
        .transpose()
    }

    /// Reads, changes and writes back one thread as a unit. `None` if it no
    /// longer exists.
    pub fn update(&self, id: &str, change: impl FnOnce(&mut Thread)) -> Result<Option<Thread>> {
        let conn = self.lock();
        let Some(mut thread) = Self::read(&conn, id)? else {
            return Ok(None);
        };
        change(&mut thread);
        Self::write(&conn, &thread)?;
        Ok(Some(thread))
    }

    /// Most recently used first.
    pub fn list(&self) -> Result<Vec<ThreadListItem>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, title, app_label, last_used FROM threads ORDER BY last_used DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ThreadListItem {
                id: row.get(0)?,
                title: row.get(1)?,
                app_label: row.get(2)?,
                last_used: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every thread, most recently used first.
    pub fn all(&self) -> Result<Vec<Thread>> {
        let ids: Vec<String> = self.list()?.into_iter().map(|t| t.id).collect();
        let conn = self.lock();
        let mut threads = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(thread) = Self::read(&conn, &id)? {
                threads.push(thread);
            }
        }
        Ok(threads)
    }

    /// Removes the thread and returns it, so its Claude sessions can go too.
    pub fn delete(&self, id: &str) -> Result<Option<Thread>> {
        let conn = self.lock();
        let thread = Self::read(&conn, id)?;
        conn.execute("DELETE FROM threads WHERE id = ?1", [id])?;
        Ok(thread)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(id: &str, last_used: i64) -> Thread {
        Thread {
            title: format!("Thread {id}"),
            app_label: Some("Code · Handy".into()),
            exchanges: vec![Exchange {
                question: Question {
                    id: "q1".into(),
                    text: "What is this?".into(),
                    via: Via::Voice,
                    at: last_used,
                    context: ContextNote {
                        selection_lines: Some(3),
                        ..Default::default()
                    },
                    prompt: None,
                },
                answer: Answer {
                    id: "a1".into(),
                    text: "A test.".into(),
                    status: AnswerStatus::Done,
                    at: last_used,
                    model: Some("sonnet".into()),
                    first_words_ms: Some(2100),
                    error: None,
                    not_spoken: None,
                    spoken: true,
                },
            }],
            ..Thread::blank(id, last_used)
        }
    }

    #[test]
    fn round_trips_and_lists_newest_first() {
        let store = AskStore::in_memory().unwrap();
        store.save(&thread("a", 10)).unwrap();
        store.save(&thread("b", 20)).unwrap();
        assert_eq!(store.get("a").unwrap(), Some(thread("a", 10)));
        let ids: Vec<_> = store.list().unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(ids, ["b", "a"]);
    }

    #[test]
    fn updates_and_deletes() {
        let store = AskStore::in_memory().unwrap();
        store.save(&thread("a", 10)).unwrap();
        let updated = store
            .update("a", |t| t.title = "Renamed".into())
            .unwrap()
            .unwrap();
        assert_eq!(updated.title, "Renamed");
        assert_eq!(store.list().unwrap()[0].title, "Renamed");
        assert!(store.update("missing", |_| {}).unwrap().is_none());
        assert_eq!(store.delete("a").unwrap().map(|t| t.id), Some("a".into()));
        assert!(store.get("a").unwrap().is_none());
    }
}
