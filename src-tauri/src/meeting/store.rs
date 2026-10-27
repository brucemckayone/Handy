//! Meetings and their transcripts, in their own `meetings.db` so History's
//! database and its migrations stay untouched.
//!
//! The list columns are kept alongside one JSON document per meeting (its
//! transcript, speakers, bookmarks and summary): a meeting is always read and
//! written whole, and the list never has to parse transcripts.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use crate::settings::SummaryEngine;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MeetingStatus {
    Recording,
    /// Handy closed while it was recording; the audio up to then is kept
    /// until the user transcribes or discards it.
    Interrupted,
    /// Queued behind another transcription.
    Waiting,
    Transcribing,
    /// Telling the other speakers apart.
    Labelling,
    Summarising,
    Ready,
    Failed,
}

impl MeetingStatus {
    fn as_str(self) -> &'static str {
        match self {
            MeetingStatus::Recording => "recording",
            MeetingStatus::Interrupted => "interrupted",
            MeetingStatus::Waiting => "waiting",
            MeetingStatus::Transcribing => "transcribing",
            MeetingStatus::Labelling => "labelling",
            MeetingStatus::Summarising => "summarising",
            MeetingStatus::Ready => "ready",
            MeetingStatus::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "recording" => MeetingStatus::Recording,
            "interrupted" => MeetingStatus::Interrupted,
            "waiting" => MeetingStatus::Waiting,
            "transcribing" => MeetingStatus::Transcribing,
            "labelling" => MeetingStatus::Labelling,
            "summarising" => MeetingStatus::Summarising,
            "ready" => MeetingStatus::Ready,
            _ => MeetingStatus::Failed,
        }
    }
}

/// Who said a segment: `me` (the mic), `them` (call audio not told apart),
/// or `s1`..`s4` (call audio, by diarized speaker).
pub type SpeakerId = String;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct TranscriptSegment {
    /// Milliseconds from the start of the meeting.
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker: SpeakerId,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct Speaker {
    pub id: SpeakerId,
    /// A name given for this meeting only; `None` shows the default label.
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct Bookmark {
    pub start_ms: i64,
    pub end_ms: i64,
}

/// Time the user spent dictating into another app during the meeting. The
/// mic audio over it is left out of the transcript.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct DictationSpan {
    pub start_ms: i64,
    pub end_ms: i64,
    /// What was dictated, once the dictation is transcribed.
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct Decision {
    pub text: String,
    pub at_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct ActionItem {
    pub text: String,
    /// A speaker id when the owner is one of the speakers, else a name.
    pub owner: Option<String>,
    pub due: Option<String>,
    pub at_ms: Option<i64>,
    pub done: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct BookmarkNote {
    pub start_ms: i64,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct MeetingSummary {
    pub overview: String,
    pub decisions: Vec<Decision>,
    pub action_items: Vec<ActionItem>,
    pub bookmarks: Vec<BookmarkNote>,
    pub engine: SummaryEngine,
    /// Unix milliseconds.
    pub created_at: i64,
}

/// How one recorded track lines up with the meeting's clock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct TrackTiming {
    /// `me` or `them`.
    pub track: String,
    /// When the track's first sample was captured, from the meeting start.
    pub start_offset_ms: i64,
    /// Meeting time per track time: the device clock's drift.
    pub scale: f64,
}

/// Something that went wrong while recording, shown with the meeting.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct CaptureProblem {
    pub at_ms: i64,
    /// `me` or `them`.
    pub track: String,
    /// `device_lost`, `device_reopened`, `device_unavailable` or
    /// `device_missing_used_default`; the UI words it.
    pub kind: String,
}

/// A transcription under way, saved chunk by chunk so it resumes where it
/// stopped rather than from the start.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialTranscript {
    /// Tracks fully transcribed.
    pub done_tracks: Vec<String>,
    /// The track being transcribed, and how many of its samples are done.
    pub track: Option<String>,
    pub samples_done: u64,
    pub segments: Vec<TranscriptSegment>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct MeetingData {
    segments: Vec<TranscriptSegment>,
    speakers: Vec<Speaker>,
    bookmarks: Vec<Bookmark>,
    dictations: Vec<DictationSpan>,
    summary: Option<MeetingSummary>,
    summary_error: Option<String>,
    tracks: Vec<TrackTiming>,
    problems: Vec<CaptureProblem>,
    partial: Option<PartialTranscript>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct Meeting {
    pub id: String,
    pub title: String,
    /// The title is still the one the summary suggested (or the default).
    pub title_is_suggested: bool,
    /// Unix milliseconds.
    pub started_at: i64,
    pub duration_ms: i64,
    pub status: MeetingStatus,
    /// Percent of the audio transcribed, while transcribing.
    pub progress: Option<u8>,
    pub error: Option<String>,
    pub starred: bool,
    /// The recorded audio is still on disk.
    pub has_audio: bool,
    pub segments: Vec<TranscriptSegment>,
    pub speakers: Vec<Speaker>,
    pub bookmarks: Vec<Bookmark>,
    pub dictations: Vec<DictationSpan>,
    pub summary: Option<MeetingSummary>,
    pub summary_error: Option<String>,
    pub tracks: Vec<TrackTiming>,
    pub problems: Vec<CaptureProblem>,
    /// Only the backend needs it.
    #[serde(skip)]
    pub partial: Option<PartialTranscript>,
}

impl Meeting {
    pub fn new(id: String, title: String, started_at: i64) -> Self {
        Self {
            id,
            title,
            title_is_suggested: true,
            started_at,
            duration_ms: 0,
            status: MeetingStatus::Recording,
            progress: None,
            error: None,
            starred: false,
            has_audio: true,
            segments: Vec::new(),
            speakers: Vec::new(),
            bookmarks: Vec::new(),
            dictations: Vec::new(),
            summary: None,
            summary_error: None,
            tracks: Vec::new(),
            problems: Vec::new(),
            partial: None,
        }
    }

    /// Speakers who said something, `me` first.
    pub fn speaker_count(&self) -> usize {
        self.speakers
            .iter()
            .filter(|s| self.segments.iter().any(|seg| seg.speaker == s.id))
            .count()
    }

    pub fn open_action_count(&self) -> usize {
        self.summary
            .as_ref()
            .map_or(0, |s| s.action_items.iter().filter(|a| !a.done).count())
    }

    fn data(&self) -> MeetingData {
        MeetingData {
            segments: self.segments.clone(),
            speakers: self.speakers.clone(),
            bookmarks: self.bookmarks.clone(),
            dictations: self.dictations.clone(),
            summary: self.summary.clone(),
            summary_error: self.summary_error.clone(),
            tracks: self.tracks.clone(),
            problems: self.problems.clone(),
            partial: self.partial.clone(),
        }
    }
}

/// One row of the Meetings list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct MeetingListItem {
    pub id: String,
    pub title: String,
    pub started_at: i64,
    pub duration_ms: i64,
    pub status: MeetingStatus,
    pub progress: Option<u8>,
    pub error: Option<String>,
    pub starred: bool,
    pub has_audio: bool,
    pub speaker_count: u32,
    pub bookmark_count: u32,
    pub open_actions: u32,
    pub overview: Option<String>,
}

impl From<&Meeting> for MeetingListItem {
    fn from(m: &Meeting) -> Self {
        Self {
            id: m.id.clone(),
            title: m.title.clone(),
            started_at: m.started_at,
            duration_ms: m.duration_ms,
            status: m.status,
            progress: m.progress,
            error: m.error.clone(),
            starred: m.starred,
            has_audio: m.has_audio,
            speaker_count: m.speaker_count() as u32,
            bookmark_count: m.bookmarks.len() as u32,
            open_actions: m.open_action_count() as u32,
            overview: m.summary.as_ref().map(|s| s.overview.clone()),
        }
    }
}

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS meetings (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    title_is_suggested INTEGER NOT NULL DEFAULT 1,
    started_at INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL,
    progress INTEGER,
    error TEXT,
    starred INTEGER NOT NULL DEFAULT 0,
    has_audio INTEGER NOT NULL DEFAULT 1,
    speaker_count INTEGER NOT NULL DEFAULT 0,
    bookmark_count INTEGER NOT NULL DEFAULT 0,
    open_actions INTEGER NOT NULL DEFAULT 0,
    overview TEXT,
    search_text TEXT NOT NULL DEFAULT '',
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS meetings_started_at ON meetings (started_at);";

const LIST_COLUMNS: &str = "id, title, started_at, duration_ms, status, progress, error, \
     starred, has_audio, speaker_count, bookmark_count, open_actions, overview";

pub struct MeetingStore {
    conn: Mutex<Connection>,
}

impl MeetingStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[cfg(test)]
    fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
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

    /// Adds the meeting, or replaces the one with its id.
    pub fn save(&self, meeting: &Meeting) -> Result<()> {
        Self::write(&self.lock(), meeting)
    }

    fn write(conn: &Connection, m: &Meeting) -> Result<()> {
        let data = m.data();
        // Search covers the title, summary and transcript text.
        let mut search_text = m.title.clone();
        if let Some(summary) = &m.summary {
            search_text.push('\n');
            search_text.push_str(&summary.overview);
            for item in &summary.action_items {
                search_text.push('\n');
                search_text.push_str(&item.text);
            }
            for decision in &summary.decisions {
                search_text.push('\n');
                search_text.push_str(&decision.text);
            }
        }
        for segment in &m.segments {
            search_text.push('\n');
            search_text.push_str(&segment.text);
        }
        conn.execute(
            "INSERT OR REPLACE INTO meetings (id, title, title_is_suggested, started_at, \
             duration_ms, status, progress, error, starred, has_audio, speaker_count, \
             bookmark_count, open_actions, overview, search_text, data) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                m.id,
                m.title,
                m.title_is_suggested,
                m.started_at,
                m.duration_ms,
                m.status.as_str(),
                m.progress,
                m.error,
                m.starred,
                m.has_audio,
                m.speaker_count() as i64,
                m.bookmarks.len() as i64,
                m.open_action_count() as i64,
                m.summary.as_ref().map(|s| s.overview.clone()),
                search_text.to_lowercase(),
                serde_json::to_string(&data)?,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Option<Meeting>> {
        Self::read(&self.lock(), id)
    }

    fn read(conn: &Connection, id: &str) -> Result<Option<Meeting>> {
        conn.query_row(
            "SELECT id, title, title_is_suggested, started_at, duration_ms, status, progress, \
             error, starred, has_audio, data FROM meetings WHERE id = ?1",
            [id],
            |row| {
                let data: String = row.get("data")?;
                let status: String = row.get("status")?;
                Ok((
                    Meeting {
                        id: row.get("id")?,
                        title: row.get("title")?,
                        title_is_suggested: row.get("title_is_suggested")?,
                        started_at: row.get("started_at")?,
                        duration_ms: row.get("duration_ms")?,
                        status: MeetingStatus::parse(&status),
                        progress: row.get("progress")?,
                        error: row.get("error")?,
                        starred: row.get("starred")?,
                        has_audio: row.get("has_audio")?,
                        segments: Vec::new(),
                        speakers: Vec::new(),
                        bookmarks: Vec::new(),
                        dictations: Vec::new(),
                        summary: None,
                        summary_error: None,
                        tracks: Vec::new(),
                        problems: Vec::new(),
                        partial: None,
                    },
                    data,
                ))
            },
        )
        .optional()?
        .map(|(mut meeting, data)| {
            let data: MeetingData = serde_json::from_str(&data)
                .map_err(|e| anyhow!("Meeting {} is unreadable: {}", meeting.id, e))?;
            meeting.segments = data.segments;
            meeting.speakers = data.speakers;
            meeting.bookmarks = data.bookmarks;
            meeting.dictations = data.dictations;
            meeting.summary = data.summary;
            meeting.summary_error = data.summary_error;
            meeting.tracks = data.tracks;
            meeting.problems = data.problems;
            meeting.partial = data.partial;
            Ok(meeting)
        })
        .transpose()
    }

    /// Reads, changes and writes back one meeting as a unit. `None` if it
    /// no longer exists.
    pub fn update(&self, id: &str, change: impl FnOnce(&mut Meeting)) -> Result<Option<Meeting>> {
        let conn = self.lock();
        let Some(mut meeting) = Self::read(&conn, id)? else {
            return Ok(None);
        };
        change(&mut meeting);
        Self::write(&conn, &meeting)?;
        Ok(Some(meeting))
    }

    /// Newest first; `query` matches titles, summaries and transcripts.
    pub fn list(&self, query: Option<&str>) -> Result<Vec<MeetingListItem>> {
        let conn = self.lock();
        let pattern = query.map(str::trim).filter(|q| !q.is_empty()).map(|q| {
            format!(
                "%{}%",
                q.to_lowercase().replace('%', "\\%").replace('_', "\\_")
            )
        });
        let sql = format!(
            "SELECT {LIST_COLUMNS} FROM meetings {} ORDER BY started_at DESC",
            if pattern.is_some() {
                "WHERE search_text LIKE ?1 ESCAPE '\\'"
            } else {
                ""
            }
        );
        let mut stmt = conn.prepare(&sql)?;
        let map = |row: &rusqlite::Row<'_>| -> rusqlite::Result<MeetingListItem> {
            let status: String = row.get("status")?;
            Ok(MeetingListItem {
                id: row.get("id")?,
                title: row.get("title")?,
                started_at: row.get("started_at")?,
                duration_ms: row.get("duration_ms")?,
                status: MeetingStatus::parse(&status),
                progress: row.get("progress")?,
                error: row.get("error")?,
                starred: row.get("starred")?,
                has_audio: row.get("has_audio")?,
                speaker_count: row.get("speaker_count")?,
                bookmark_count: row.get("bookmark_count")?,
                open_actions: row.get("open_actions")?,
                overview: row.get("overview")?,
            })
        };
        let rows = match &pattern {
            Some(p) => stmt
                .query_map([p], map)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            None => stmt
                .query_map([], map)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        };
        Ok(rows)
    }

    /// Ids of meetings in one of `statuses`, oldest first.
    pub fn ids_with_status(&self, statuses: &[MeetingStatus]) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT id, status FROM meetings ORDER BY started_at ASC")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut ids = Vec::new();
        for row in rows {
            let (id, status) = row?;
            if statuses.contains(&MeetingStatus::parse(&status)) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        self.lock()
            .execute("DELETE FROM meetings WHERE id = ?1", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meeting(id: &str, started_at: i64) -> Meeting {
        Meeting::new(id.to_string(), format!("Meeting {id}"), started_at)
    }

    #[test]
    fn a_meeting_round_trips_whole() {
        let store = MeetingStore::in_memory().unwrap();
        let mut m = meeting("a", 1_000);
        m.segments.push(TranscriptSegment {
            start_ms: 1_200,
            end_ms: 2_500,
            speaker: "me".into(),
            text: "Hello there".into(),
        });
        m.speakers.push(Speaker {
            id: "me".into(),
            name: None,
        });
        m.bookmarks.push(Bookmark {
            start_ms: 0,
            end_ms: 30_000,
        });
        store.save(&m).unwrap();
        assert_eq!(store.get("a").unwrap(), Some(m));
        assert_eq!(store.get("missing").unwrap(), None);
    }

    #[test]
    fn the_list_is_newest_first_and_searchable() {
        let store = MeetingStore::in_memory().unwrap();
        store.save(&meeting("old", 1_000)).unwrap();
        let mut new = meeting("new", 2_000);
        new.segments.push(TranscriptSegment {
            start_ms: 0,
            end_ms: 1_000,
            speaker: "them".into(),
            text: "Pricing for the pilot".into(),
        });
        new.speakers.push(Speaker {
            id: "them".into(),
            name: None,
        });
        store.save(&new).unwrap();

        let all = store.list(None).unwrap();
        assert_eq!(
            all.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["new", "old"]
        );
        assert_eq!(all[0].speaker_count, 1);

        let found = store.list(Some("PILOT")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "new");
        assert!(store.list(Some("100%")).unwrap().is_empty());
    }

    #[test]
    fn update_changes_one_meeting_and_skips_missing_ones() {
        let store = MeetingStore::in_memory().unwrap();
        store.save(&meeting("a", 1_000)).unwrap();
        let updated = store
            .update("a", |m| m.status = MeetingStatus::Ready)
            .unwrap()
            .unwrap();
        assert_eq!(updated.status, MeetingStatus::Ready);
        assert_eq!(
            store.get("a").unwrap().unwrap().status,
            MeetingStatus::Ready
        );
        assert!(store.update("missing", |_| {}).unwrap().is_none());
        assert_eq!(
            store.ids_with_status(&[MeetingStatus::Ready]).unwrap(),
            ["a"]
        );
        store.delete("a").unwrap();
        assert!(store.get("a").unwrap().is_none());
    }
}
