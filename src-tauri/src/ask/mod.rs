//! Ask: a spoken (or typed) question, answered out loud by headless Claude
//! Code, with what was on screen sent along.
//!
//! Questions sort into threads; each thread is one Claude Code conversation,
//! run in Handy's own workspace folder so it never shows in a project's
//! `/resume` list. A new question stops the answer still coming.

pub mod agents;
pub mod answer;
pub mod claude;
pub mod cli;
pub mod commands;
pub mod expiry;
pub mod panel;
pub mod prompt;
pub mod recent;
pub mod route;
pub mod screenshot;
pub mod speaker;
pub mod store;
pub mod tidy;
pub mod voice;
pub mod window;

use anyhow::{anyhow, Result};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::settings::get_settings;
use claude::{Line, RunError, Session};
use prompt::ScreenContext;
use speaker::Speaker;
use store::{
    AgentLink, Answer, AnswerStatus, AskError, AskErrorCode, AskStore, ContextNote, Exchange,
    MergeSuggestion, Question, Thread, ThreadListItem, Via, WindowKey,
};

/// The panel's window label.
pub const PANEL_LABEL: &str = "ask_panel";
/// How often the upkeep looks for work.
const UPKEEP_POLL: std::time::Duration = std::time::Duration::from_secs(30);
/// How often old threads are looked for, in milliseconds.
const EXPIRE_EVERY_MS: i64 = 60 * 60 * 1000;
/// The tidy-up waits for this long without questions, in milliseconds.
const TIDY_AFTER_MS: i64 = 2 * 60 * 1000;
/// Thread titles are cut to about this many characters until the tidy-up
/// names them.
const TITLE_CHARS: usize = 60;

/// A thread changed: sent whole.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct AskThreadEvent {
    pub thread: Thread,
}

/// More of an answer arrived. `reset` drops what came before it (narration
/// before Claude used a tool).
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct AskDeltaEvent {
    pub thread_id: String,
    pub answer_id: String,
    pub reset: bool,
    pub text: String,
}

/// A thread was deleted.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct AskDeletedEvent {
    pub thread_id: String,
}

/// What Ask shows in the speaking pill: busy while answering, then the
/// answered thread (with an Open button) for a few seconds.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct AskBusyEvent {
    pub busy: bool,
    pub thread_id: Option<String>,
    /// The answer failed: the pill says so.
    pub failed: bool,
}

/// A question on its way in.
pub struct NewQuestion {
    pub text: String,
    pub via: Via,
    /// Continue this thread; otherwise a new one.
    pub thread_id: Option<String>,
    pub window: Option<WindowKey>,
    pub app_label: Option<String>,
    pub project_dir: Option<PathBuf>,
    pub agent: Option<AgentLink>,
    pub screen: ScreenContext,
    /// Send this instead of building the prompt (a moved question).
    pub prompt: Option<String>,
}

impl NewQuestion {
    pub fn text(text: String, via: Via, thread_id: Option<String>) -> Self {
        Self {
            text,
            via,
            thread_id,
            window: None,
            app_label: None,
            project_dir: None,
            agent: None,
            screen: ScreenContext::default(),
            prompt: None,
        }
    }
}

pub struct AskManager {
    app: AppHandle,
    store: AskStore,
    /// Handy's own threads run here, apart from every project.
    workspace: PathBuf,
    /// Bumped by every question and cancel: older runs stop.
    generation: AtomicU64,
    answering: AtomicBool,
    /// When the last question was asked or answered.
    last_activity: AtomicI64,
    /// Threads changed since the last tidy-up.
    untidy: AtomicBool,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// A random version-4 UUID, as Claude Code's `--session-id` wants.
pub fn new_uuid() -> String {
    // std's hasher keys are random per process; mixing in time and a counter
    // makes every value differ.
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let state = std::collections::hash_map::RandomState::new();
    let half = |salt: u64| {
        let mut h = state.build_hasher();
        h.write_u64(salt);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
        );
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        h.finish()
    };
    let (hi, lo) = (half(1), half(2));
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&hi.to_be_bytes());
    bytes[8..].copy_from_slice(&lo.to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// The first words of the question, as a thread's first title.
fn suggested_title(question: &str) -> String {
    let question = question.trim();
    if question.chars().count() <= TITLE_CHARS {
        return question.to_string();
    }
    let cut: String = question.chars().take(TITLE_CHARS).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', '.', ';', ':']))
}

fn note_for(question: &NewQuestion) -> ContextNote {
    let lines = |text: &Option<String>| {
        text.as_ref()
            .map(|t| t.trim().lines().count().max(1) as u32)
    };
    ContextNote {
        selection_lines: lines(&question.screen.selection),
        input_lines: lines(&question.screen.input),
        screenshot: question.screen.screenshot.is_some(),
        read_aloud: !question.screen.recent_reads.is_empty(),
        agent: question
            .agent
            .as_ref()
            .filter(|a| a.linked)
            .map(|a| a.name.clone()),
    }
}

impl AskManager {
    pub fn new(app: &AppHandle) -> Result<Self> {
        let app_data = crate::portable::app_data_dir(app)?;
        let workspace = app_data.join("ask-workspace");
        std::fs::create_dir_all(&workspace)?;
        let store = AskStore::open(&app_data.join("asks.db"))?;
        // Screenshots left by questions that never got asked (cancelled, or
        // Handy closed) go now.
        let _ = std::fs::remove_dir_all(screenshot::dir());
        Ok(Self {
            app: app.clone(),
            store,
            workspace,
            generation: AtomicU64::new(0),
            answering: AtomicBool::new(false),
            last_activity: AtomicI64::new(0),
            untidy: AtomicBool::new(false),
        })
    }

    pub fn threads(&self) -> Result<Vec<ThreadListItem>> {
        self.store.list()
    }

    pub fn thread(&self, id: &str) -> Result<Option<Thread>> {
        self.store.get(id)
    }

    /// Stops the answer being worked on, if any.
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Deletes a thread and the Claude Code sessions Handy made for it.
    pub fn delete(&self, id: &str) -> Result<()> {
        if let (Some(thread), Some(claude_dir)) = (self.store.delete(id)?, expiry::claude_dir()) {
            for session in &thread.sessions {
                expiry::delete_session(&claude_dir, &self.workspace, session);
            }
        }
        self.deleted(id);
        Ok(())
    }

    fn deleted(&self, id: &str) {
        let _ = AskDeletedEvent {
            thread_id: id.to_string(),
        }
        .emit_to(&self.app, PANEL_LABEL);
    }

    pub fn delete_all(&self) -> Result<()> {
        self.cancel();
        for thread in self.store.list()? {
            self.delete(&thread.id)?;
        }
        Ok(())
    }

    /// Deletes threads that have gone unused past the setting.
    pub fn expire(&self) -> Result<()> {
        let keep_days = get_settings(&self.app).ask.keep_days;
        let now = now_ms();
        for thread in self.store.list()? {
            if expiry::expired(thread.last_used, now, keep_days) {
                info!("Deleting Ask thread unused for {keep_days} days");
                self.delete(&thread.id)?;
            }
        }
        Ok(())
    }

    /// Starts the background upkeep: deleting old threads, and tidying
    /// threads once questions have stopped for a while.
    pub fn start(self: &Arc<Self>) {
        let manager = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("ask-upkeep".into())
            .spawn(move || {
                let mut expired_at = i64::MIN;
                loop {
                    let now = now_ms();
                    if now - expired_at >= EXPIRE_EVERY_MS {
                        expired_at = now;
                        if let Err(e) = manager.expire() {
                            warn!("Failed to delete old Ask threads: {e}");
                        }
                    }
                    let quiet = now - manager.last_activity.load(Ordering::Acquire) > TIDY_AFTER_MS;
                    if quiet
                        && !manager.answering.load(Ordering::Acquire)
                        && manager.untidy.swap(false, Ordering::AcqRel)
                    {
                        if let Err(e) = manager.tidy() {
                            warn!("Ask tidy-up failed: {e}");
                        }
                    }
                    std::thread::sleep(UPKEEP_POLL);
                }
            });
        if let Err(e) = spawned {
            warn!("Failed to start Ask upkeep: {e}");
        }
    }

    /// Names new threads and suggests merging related ones.
    fn tidy(&self) -> Result<()> {
        let threads: Vec<Thread> = self.store.all()?.into_iter().take(tidy::RECENT).collect();
        if !threads.iter().any(|t| t.title_is_suggested) && threads.len() < 2 {
            return Ok(());
        }
        let Some(reply) = tidy::parse(&tidy::run(&tidy::prompt(&threads), &self.workspace)?) else {
            return Ok(());
        };
        for title in reply.titles {
            let name = title
                .title
                .trim()
                .trim_matches('"')
                .trim_end_matches('.')
                .trim();
            if name.is_empty() || name.chars().count() > 80 {
                continue;
            }
            let renamed = self.store.update(&title.id, |t| {
                if t.title_is_suggested {
                    t.title = name.to_string();
                    t.title_is_suggested = false;
                }
            })?;
            if let Some(thread) = renamed {
                self.changed(&thread);
            }
        }
        for pair in reply.related {
            let (Some(a), Some(b)) = (self.store.get(&pair.a)?, self.store.get(&pair.b)?) else {
                continue;
            };
            if a.id == b.id || a.kept_apart.contains(&b.id) || b.kept_apart.contains(&a.id) {
                continue;
            }
            // Offered on the newer thread: the older one merges into it.
            let (newer, older) = if a.last_used >= b.last_used {
                (a, b)
            } else {
                (b, a)
            };
            if newer.merge.is_some() {
                continue;
            }
            let suggested = self.store.update(&newer.id, |t| {
                t.merge = Some(MergeSuggestion {
                    thread_id: older.id.clone(),
                    title: older.title.clone(),
                });
            })?;
            if let Some(thread) = suggested {
                self.changed(&thread);
            }
        }
        Ok(())
    }

    /// Takes the suggested thread into this one; its sessions come along.
    pub fn merge(&self, id: &str) -> Result<()> {
        let Some(suggestion) = self.store.get(id)?.and_then(|t| t.merge) else {
            return Ok(());
        };
        let Some(other) = self.store.get(&suggestion.thread_id)? else {
            return self.keep_apart(id);
        };
        let merged = self.store.update(id, |t| {
            t.exchanges.extend(other.exchanges.iter().cloned());
            t.exchanges.sort_by_key(|e| e.question.at);
            for session in &other.sessions {
                if !t.sessions.contains(session) {
                    t.sessions.push(session.clone());
                }
            }
            t.last_used = t.last_used.max(other.last_used);
            t.merge = None;
        })?;
        // Only the row goes: its sessions now belong to this thread.
        self.store.delete(&other.id)?;
        self.deleted(&other.id);
        if let Some(thread) = merged {
            self.changed(&thread);
        }
        Ok(())
    }

    /// Declines the merge suggestion; it isn't offered again.
    pub fn keep_apart(&self, id: &str) -> Result<()> {
        let thread = self.store.update(id, |t| {
            if let Some(suggestion) = t.merge.take() {
                t.kept_apart.push(suggestion.thread_id);
            }
        })?;
        if let Some(thread) = thread {
            self.changed(&thread);
        }
        Ok(())
    }

    /// Stops answering this thread from its agent: the next question starts
    /// a fresh conversation of Handy's own.
    pub fn unlink_agent(&self, id: &str) -> Result<()> {
        let thread = self.store.update(id, |t| {
            if let Some(agent) = t.agent.as_mut().filter(|a| a.linked) {
                agent.linked = false;
                t.claude_session = None;
            }
        })?;
        if let Some(thread) = thread {
            self.changed(&thread);
        }
        Ok(())
    }

    /// Takes a question out of its thread and asks it again in `to` (or a
    /// new thread), with the same screen context. A thread left empty goes.
    pub fn move_question(
        self: &Arc<Self>,
        from: &str,
        question_id: &str,
        to: Option<String>,
    ) -> Result<String> {
        let mut moved = None;
        let left = self.store.update(from, |t| {
            if let Some(i) = t
                .exchanges
                .iter()
                .position(|e| e.question.id == question_id)
            {
                moved = Some(t.exchanges.remove(i));
            }
        })?;
        let question = moved.ok_or_else(|| anyhow!("No such question"))?.question;
        match left {
            Some(t) if t.exchanges.is_empty() => self.delete(from)?,
            Some(t) => self.changed(&t),
            None => {}
        }
        self.ask(NewQuestion {
            prompt: question.prompt,
            ..NewQuestion::text(question.text, question.via, to)
        })
    }

    fn changed(&self, thread: &Thread) {
        let event = AskThreadEvent {
            thread: thread.clone(),
        };
        if let Err(e) = event.emit_to(&self.app, PANEL_LABEL) {
            warn!("Failed to send the thread to the Ask panel: {e}");
        }
    }

    /// Records the question and answers it in the background. Returns the
    /// thread it went into.
    pub fn ask(self: &Arc<Self>, question: NewQuestion) -> Result<String> {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        crate::speech::stop(&self.app);

        let now = now_ms();
        self.last_activity.store(now, Ordering::Release);
        let settings = get_settings(&self.app).ask;
        let (thread_id, text) = match &question.thread_id {
            Some(id) => (Some(id.clone()), question.text.trim().to_string()),
            None => {
                let threads = self.store.all()?;
                match route::route(
                    &question.text,
                    question.window.as_ref(),
                    &threads,
                    now,
                    settings.same_window_minutes,
                ) {
                    (route::Route::Thread(id), text) => (Some(id), text),
                    (route::Route::New, text) => (None, text),
                }
            }
        };
        if text.is_empty() {
            return Err(anyhow!("Nothing was asked"));
        }
        let existing = match &thread_id {
            Some(id) => self.store.get(id)?,
            None => None,
        };
        let mut thread = existing.unwrap_or_else(|| Thread {
            id: new_uuid(),
            title: suggested_title(&text),
            title_is_suggested: true,
            created_at: now,
            last_used: now,
            window: question.window.clone(),
            app_label: question.app_label.clone(),
            project_dir: question
                .project_dir
                .as_ref()
                .map(|p| p.display().to_string()),
            agent: question.agent.clone(),
            claude_session: None,
            sessions: Vec::new(),
            merge: None,
            kept_apart: Vec::new(),
            exchanges: Vec::new(),
        });
        let answer_id = new_uuid();
        let prompt = question
            .prompt
            .clone()
            .unwrap_or_else(|| prompt::build(&text, &question.screen));
        thread.last_used = now;
        thread.exchanges.push(Exchange {
            question: Question {
                id: new_uuid(),
                text: text.clone(),
                via: question.via,
                at: now,
                context: note_for(&question),
                prompt: Some(prompt.clone()),
            },
            answer: Answer {
                id: answer_id.clone(),
                text: String::new(),
                status: AnswerStatus::Thinking,
                at: now,
                model: None,
                first_words_ms: None,
                error: None,
                not_spoken: None,
                spoken: false,
            },
        });

        let agent = thread.agent.as_ref().filter(|a| a.linked);
        let (session, model) = match (&thread.claude_session, agent) {
            (Some(id), _) => (
                Session::Resume(id.clone()),
                agent.is_none().then(|| settings.model.clone()),
            ),
            (None, Some(agent)) => (Session::Fork(agent.session_id.clone()), None),
            (None, None) => (Session::New(new_uuid()), Some(settings.model.clone())),
        };
        if let Some(last) = thread.exchanges.last_mut() {
            last.answer.model = model.clone();
        }
        self.store.save(&thread)?;
        self.changed(&thread);
        self.answering.store(true, Ordering::Release);
        crate::speech::pill::ask_started(&self.app, &thread.id);
        if settings.open_panel {
            panel::open(&self.app, Some(thread.id.clone()), false);
        }

        let mut add_dirs: Vec<PathBuf> = Vec::new();
        if let Some(dir) = &thread.project_dir {
            add_dirs.push(PathBuf::from(dir));
        }
        if let Some(agent) = agent {
            add_dirs.push(PathBuf::from(&agent.cwd));
        }
        if let Some(dir) = question.screen.screenshot.as_ref().and_then(|p| p.parent()) {
            add_dirs.push(dir.to_path_buf());
        }
        add_dirs.dedup();
        let request = claude::Request {
            prompt,
            session,
            model,
            cwd: self.workspace.clone(),
            add_dirs,
        };

        let manager = Arc::clone(self);
        let thread_id = thread.id.clone();
        let screenshot = question.screen.screenshot.clone();
        let read_aloud = settings.read_aloud;
        std::thread::Builder::new()
            .name("ask".into())
            .spawn(move || {
                manager.answer(&thread_id, &answer_id, request, generation, read_aloud);
                if let Some(path) = screenshot {
                    let _ = std::fs::remove_file(path);
                }
            })?;
        Ok(thread.id)
    }

    fn answer(
        &self,
        thread_id: &str,
        answer_id: &str,
        request: claude::Request,
        generation: u64,
        read_aloud: bool,
    ) {
        let started = Instant::now();
        let mut text = String::new();
        let mut first_words: Option<u32> = None;
        let mut speaker = Speaker::new(&self.app, read_aloud);
        let is_current = || self.generation.load(Ordering::Acquire) == generation;
        info!("Asking Claude Code ({:?})", request.session);
        let outcome = claude::run(
            &request,
            || !is_current(),
            |line| {
                let (reset, more) = match line {
                    Line::MessageStart if !text.is_empty() => (true, ""),
                    Line::Text(more) => (false, more.as_str()),
                    _ => return,
                };
                if reset {
                    text.clear();
                    speaker.reset();
                }
                text.push_str(more);
                if is_current() {
                    speaker.feed(&text);
                }
                if first_words.is_none() && !more.is_empty() {
                    first_words = Some(started.elapsed().as_millis() as u32);
                    let ms = first_words;
                    if let Ok(Some(thread)) = self.store.update(thread_id, |t| {
                        if let Some(answer) = find_answer(t, answer_id) {
                            answer.status = AnswerStatus::Streaming;
                            answer.first_words_ms = ms;
                        }
                    }) {
                        self.changed(&thread);
                    }
                }
                let _ = AskDeltaEvent {
                    thread_id: thread_id.to_string(),
                    answer_id: answer_id.to_string(),
                    reset,
                    text: more.to_string(),
                }
                .emit_to(&self.app, PANEL_LABEL);
            },
        );
        let current = is_current();
        let not_spoken = match &outcome {
            Ok(finished) if current => speaker.finish(&finished.text),
            _ => None,
        };
        if let Some(refusal) = not_spoken {
            crate::speech::refused(&self.app, refusal);
        }
        let updated = self.store.update(thread_id, |t| {
            if let Ok(finished) = &outcome {
                if let Some(id) = &finished.session_id {
                    t.claude_session = Some(id.clone());
                    if !t.sessions.contains(id) {
                        t.sessions.push(id.clone());
                    }
                }
            }
            let Some(answer) = find_answer(t, answer_id) else {
                return;
            };
            answer.first_words_ms = first_words;
            answer.not_spoken = not_spoken;
            answer.spoken = speaker.spoke();
            match &outcome {
                Ok(finished) => {
                    answer.text = finished.text.clone();
                    answer.status = AnswerStatus::Done;
                }
                Err(e) => {
                    answer.text = text.clone();
                    answer.status = if *e == RunError::Cancelled {
                        AnswerStatus::Cancelled
                    } else {
                        AnswerStatus::Failed
                    };
                    answer.error = match e {
                        RunError::Cancelled => None,
                        RunError::NotFound => Some(AskError {
                            code: AskErrorCode::ClaudeNotFound,
                            detail: None,
                        }),
                        RunError::TimedOut => Some(AskError {
                            code: AskErrorCode::TimedOut,
                            detail: None,
                        }),
                        RunError::Failed(detail) => Some(AskError {
                            code: AskErrorCode::Failed,
                            detail: Some(detail.clone()),
                        }),
                    };
                }
            }
        });
        match updated {
            Ok(Some(thread)) => self.changed(&thread),
            Ok(None) => {}
            Err(e) => error!("Failed to save the answer: {e}"),
        }
        match &outcome {
            Ok(_) => info!("Answered in {:?}", started.elapsed()),
            Err(e) => info!("No answer: {e:?}"),
        }
        self.last_activity.store(now_ms(), Ordering::Release);
        self.untidy.store(true, Ordering::Release);
        if current {
            self.answering.store(false, Ordering::Release);
            crate::speech::pill::ask_finished(&self.app, thread_id, outcome.is_err());
        }
    }
}

fn find_answer<'a>(thread: &'a mut Thread, answer_id: &str) -> Option<&'a mut Answer> {
    thread
        .exchanges
        .iter_mut()
        .map(|e| &mut e.answer)
        .find(|a| a.id == answer_id)
}

/// The Ask manager, once Handy has set it up.
pub fn manager(app: &AppHandle) -> Option<Arc<AskManager>> {
    app.try_state::<Arc<AskManager>>()
        .map(|m| Arc::clone(m.inner()))
}

/// Asks `question`, logging rather than failing.
pub fn ask(app: &AppHandle, question: NewQuestion) {
    let Some(manager) = manager(app) else {
        warn!("Ask isn't set up; question dropped");
        return;
    };
    if question.text.trim().is_empty() {
        return;
    }
    if let Err(e) = manager.ask(question) {
        error!("Failed to ask: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_look_right_and_differ() {
        let a = new_uuid();
        let b = new_uuid();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        let parts: Vec<_> = a.split('-').map(str::len).collect();
        assert_eq!(parts, [8, 4, 4, 4, 12]);
        assert_eq!(&a[14..15], "4");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn titles_cut_at_a_word() {
        assert_eq!(suggested_title("  Short one?  "), "Short one?");
        let long =
            "What does process transcription output do, and where is it called from in the code?";
        let title = suggested_title(long);
        assert!(title.ends_with('…'));
        assert!(title.chars().count() <= TITLE_CHARS + 1);
        assert!(!title.contains("  "));
    }
}
