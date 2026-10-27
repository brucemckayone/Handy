//! Meeting capture: records the user's mic and the call audio as two tracks,
//! transcribes them after the meeting with timestamps and speaker labels,
//! deletes the audio once the transcript is saved, then summarises it.
//!
//! Nothing is recorded without the user starting it (tray, Meetings page,
//! shortcut, or answering the call prompt), and no voice is ever learned:
//! speakers are told apart within one meeting only.

pub mod capture;
pub mod commands;
pub mod detect;
pub mod speakers;
pub mod store;
pub mod summary;
pub mod transcribe;
pub mod widgets;

use anyhow::{anyhow, Result};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
use crate::managers::transcription_jobs::TranscriptionJobManager;
use crate::settings::get_settings;
use capture::{Capture, CaptureOptions, TrackKind};
use store::{
    Bookmark, CaptureProblem, DictationSpan, Meeting, MeetingListItem, MeetingStatus, MeetingStore,
    TrackTiming,
};

/// How often the pill's level meters are refreshed.
const LEVELS_EVERY: Duration = Duration::from_millis(100);
/// Dictation audio is left out with this much margin either side.
pub const DICTATION_MARGIN_MS: i64 = 300;

/// The meeting being recorded, for the tray, the pill and the Meetings page.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct RecordingInfo {
    pub id: String,
    /// Unix milliseconds.
    pub started_at: i64,
    pub bookmarks: u32,
    /// A dictation is under way, so the pill shows it too.
    pub dictating: bool,
}

/// Sent when a meeting starts or stops recording, or gains a bookmark.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingRecordingEvent {
    pub recording: Option<RecordingInfo>,
}

/// Sent when a meeting changes; `meeting` is `None` once it is deleted.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingChangedEvent {
    pub id: String,
    pub meeting: Option<MeetingListItem>,
}

/// Peak level of each track over the last tenth of a second, 0..1.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingLevelsEvent {
    pub me: f32,
    pub them: f32,
}

struct Active {
    id: String,
    started_at: i64,
    start: Instant,
    capture: Capture,
    bookmarks: u32,
    /// When the dictation under way began, from the meeting start.
    dictation_from_ms: Option<i64>,
    levels_done: Arc<AtomicBool>,
}

pub struct MeetingManager {
    app: AppHandle,
    store: MeetingStore,
    root: PathBuf,
    active: Mutex<Option<Active>>,
    /// A meeting is opening its devices; `active` is set once they are open.
    starting: AtomicBool,
    /// The meeting the latest dictation was spoken in, so its text can be
    /// attached once it is transcribed.
    last_dictation_meeting: Mutex<Option<String>>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl MeetingManager {
    pub fn new(app: &AppHandle) -> Result<Self> {
        let app_data = crate::portable::app_data_dir(app)?;
        let root = app_data.join("meetings");
        std::fs::create_dir_all(&root)?;
        let store = MeetingStore::open(&app_data.join("meetings.db"))?;
        let manager = Self {
            app: app.clone(),
            store,
            root,
            active: Mutex::new(None),
            starting: AtomicBool::new(false),
            last_dictation_meeting: Mutex::new(None),
        };
        manager.recover_interrupted();
        manager.remove_stray_audio();
        Ok(manager)
    }

    /// Deletes audio left behind by a meeting that no longer needs it: one
    /// deleted, or transcribed when Handy stopped before removing it.
    fn remove_stray_audio(&self) {
        let Ok(dirs) = std::fs::read_dir(&self.root) else {
            return;
        };
        for dir in dirs.flatten() {
            let id = dir.file_name().to_string_lossy().into_owned();
            // A meeting that can't be read keeps its audio.
            let stray = match self.store.get(&id) {
                Ok(None) => true,
                Ok(Some(m)) => !m.has_audio,
                Err(_) => false,
            };
            if stray {
                info!("Deleting leftover audio of meeting {}", id);
                self.remove_audio(&id);
            }
        }
    }

    pub fn store(&self) -> &MeetingStore {
        &self.store
    }

    /// The folder a meeting's audio is kept in while it is processed.
    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    fn lock(&self) -> MutexGuard<'_, Option<Active>> {
        self.active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn is_recording(&self) -> bool {
        self.lock().is_some()
    }

    pub fn recording(&self) -> Option<RecordingInfo> {
        self.lock().as_ref().map(Active::info)
    }

    /// Milliseconds since the meeting being recorded started.
    pub fn elapsed_ms(&self) -> Option<i64> {
        self.lock()
            .as_ref()
            .map(|a| a.start.elapsed().as_millis() as i64)
    }

    /// Tells everyone that a meeting changed.
    pub fn changed(&self, meeting: &Meeting) {
        let event = MeetingChangedEvent {
            id: meeting.id.clone(),
            meeting: Some(MeetingListItem::from(meeting)),
        };
        if let Err(e) = event.emit(&self.app) {
            error!("Failed to emit meeting change: {}", e);
        }
    }

    /// Changes a stored meeting and tells everyone. `None` if it is gone.
    pub fn update(&self, id: &str, change: impl FnOnce(&mut Meeting)) -> Result<Option<Meeting>> {
        let meeting = self.store.update(id, change)?;
        if let Some(meeting) = &meeting {
            self.changed(meeting);
        }
        Ok(meeting)
    }

    fn recording_changed(&self) {
        let recording = self.recording();
        if let Err(e) = (MeetingRecordingEvent {
            recording: recording.clone(),
        })
        .emit(&self.app)
        {
            error!("Failed to emit meeting recording state: {}", e);
        }
        crate::tray::update_tray_menu(&self.app);
        widgets::sync_pill(&self.app, recording.as_ref());
    }

    /// Starts recording a meeting.
    pub fn start(self: &Arc<Self>) -> Result<RecordingInfo> {
        if self.is_recording() || self.starting.swap(true, Ordering::AcqRel) {
            return Err(anyhow!("A meeting is already being recorded"));
        }
        // Opening devices takes a moment; nothing waits on `active` for it.
        let opened = self.open();
        self.starting.store(false, Ordering::Release);
        let (info, meeting) = opened?;
        info!("Started recording meeting {}", info.id);
        self.changed(&meeting);
        self.recording_changed();
        detect::meeting_started(&self.app);
        Ok(info)
    }

    fn open(self: &Arc<Self>) -> Result<(RecordingInfo, Meeting)> {
        let settings = get_settings(&self.app);
        let started_at = now_ms();
        let id = started_at.to_string();
        let dir = self.dir(&id);
        let start = Instant::now();
        // "default" (any case) is the Windows default device.
        let named = |name: Option<String>| name.filter(|n| !n.eq_ignore_ascii_case("default"));
        let capture = Capture::start(
            CaptureOptions {
                mic: named(
                    settings
                        .meeting
                        .microphone
                        .clone()
                        .or(settings.selected_microphone.clone()),
                ),
                output: named(settings.meeting.output_device.clone()),
                dir: dir.clone(),
            },
            start,
        )
        .inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&dir);
        })?;
        let meeting = Meeting::new(id.clone(), String::new(), started_at);
        if let Err(e) = self.store.save(&meeting) {
            let _ = capture.stop();
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        let levels_done = Arc::new(AtomicBool::new(false));
        spawn_levels(
            self.app.clone(),
            Arc::clone(capture.levels()),
            Arc::clone(&levels_done),
        );
        let recording = Active {
            id,
            started_at,
            start,
            capture,
            bookmarks: 0,
            dictation_from_ms: None,
            levels_done,
        };
        let info = recording.info();
        *self.lock() = Some(recording);
        Ok((info, meeting))
    }

    /// Stops the meeting being recorded and queues it to be transcribed.
    pub fn stop(&self) -> Result<String> {
        let Some(mut active) = self.lock().take() else {
            return Err(anyhow!("No meeting is being recorded"));
        };
        active.levels_done.store(true, Ordering::Relaxed);
        let duration_ms = active.start.elapsed().as_millis() as i64;
        let open_dictation = active.dictation_from_ms.take();
        let id = active.id.clone();
        let report = active.capture.stop();
        info!(
            "Stopped recording meeting {} after {} s",
            id,
            duration_ms / 1000
        );

        let has_audio = !report.tracks.is_empty();
        let saved = self.update(&id, |m| {
            m.duration_ms = duration_ms;
            if let Some(from) = open_dictation {
                m.dictations.push(DictationSpan {
                    start_ms: from,
                    end_ms: duration_ms,
                    text: None,
                });
            }
            m.tracks = report
                .tracks
                .iter()
                .map(|t| TrackTiming {
                    track: t.kind.name().to_string(),
                    start_offset_ms: t.start_offset_ms,
                    scale: t.scale,
                })
                .collect();
            m.problems = report
                .problems
                .iter()
                .map(|p| CaptureProblem {
                    at_ms: p.at_ms,
                    track: p.track.name().to_string(),
                    kind: p.kind.to_string(),
                })
                .collect();
            m.has_audio = has_audio;
            if has_audio {
                m.status = MeetingStatus::Waiting;
            } else {
                m.status = MeetingStatus::Failed;
                m.error = Some("no_audio".into());
            }
        });
        // The recording has ended whether or not it could be saved.
        self.recording_changed();
        detect::meeting_stopped(&self.app);
        if saved?.is_some() && has_audio {
            self.app
                .state::<Arc<TranscriptionJobManager>>()
                .queue_meeting(&id);
        }
        Ok(id)
    }

    /// Marks the last few seconds of the meeting being recorded.
    pub fn bookmark(&self) -> Result<u32> {
        let seconds = get_settings(&self.app).meeting.bookmark_seconds.max(5) as i64;
        let (id, end_ms, count) = {
            let mut active = self.lock();
            let active = active
                .as_mut()
                .ok_or_else(|| anyhow!("No meeting is being recorded"))?;
            active.bookmarks += 1;
            (
                active.id.clone(),
                active.start.elapsed().as_millis() as i64,
                active.bookmarks,
            )
        };
        self.update(&id, |m| {
            m.bookmarks.push(Bookmark {
                start_ms: (end_ms - seconds * 1000).max(0),
                end_ms,
            })
        })?;
        self.recording_changed();
        widgets::pill_message(&self.app, "bookmarked");
        Ok(count)
    }

    fn set_last_dictation(&self, id: Option<String>) {
        *self
            .last_dictation_meeting
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = id;
    }

    /// A dictation started; while a meeting records, its time is noted so
    /// the mic audio can be left out.
    pub fn dictation_started(&self) {
        // Text still owed by an earlier dictation never comes now.
        self.set_last_dictation(None);
        let mut active = self.lock();
        if let Some(active) = active.as_mut() {
            if active.dictation_from_ms.is_none() {
                active.dictation_from_ms = Some(active.start.elapsed().as_millis() as i64);
            }
        } else {
            return;
        }
        drop(active);
        self.recording_changed();
    }

    /// The dictation's recording ended.
    pub fn dictation_stopped(&self) {
        let closed = {
            let mut active = self.lock();
            active.as_mut().and_then(|active| {
                let from = active.dictation_from_ms.take()?;
                Some((
                    active.id.clone(),
                    DictationSpan {
                        start_ms: from,
                        end_ms: active.start.elapsed().as_millis() as i64,
                        text: None,
                    },
                ))
            })
        };
        let Some((id, span)) = closed else {
            // Spoken outside a meeting: its text belongs to none.
            self.set_last_dictation(None);
            return;
        };
        self.set_last_dictation(Some(id.clone()));
        if let Err(e) = self.update(&id, |m| m.dictations.push(span)) {
            error!("Failed to note a dictation in meeting {}: {}", id, e);
        }
        self.recording_changed();
    }

    /// The dictation was cancelled: its time is still left out, but no text
    /// will follow.
    pub fn dictation_cancelled(&self) {
        self.dictation_stopped();
        self.set_last_dictation(None);
    }

    /// The text of the latest dictation, shown with the meeting it was
    /// left out of.
    pub fn dictation_transcribed(&self, text: &str) {
        let Some(id) = self
            .last_dictation_meeting
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        else {
            return;
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Err(e) = self.update(&id, |m| {
            if let Some(span) = m.dictations.iter_mut().rev().find(|s| s.text.is_none()) {
                span.text = Some(text);
            }
        }) {
            error!("Failed to keep a dictation's text in meeting {}: {}", id, e);
        }
    }

    /// Deletes a meeting, its transcript and any audio still kept.
    pub fn delete(&self, id: &str) -> Result<()> {
        if self.lock().as_ref().is_some_and(|a| a.id == id) {
            return Err(anyhow!("Stop the meeting before deleting it"));
        }
        self.app
            .state::<Arc<TranscriptionJobManager>>()
            .cancel_meeting(id);
        self.remove_audio(id);
        self.store.delete(id)?;
        let _ = (MeetingChangedEvent {
            id: id.to_string(),
            meeting: None,
        })
        .emit(&self.app);
        Ok(())
    }

    /// Transcribes a meeting again from its audio: an interrupted one, or
    /// one whose transcription failed.
    pub fn retry(&self, id: &str) -> Result<()> {
        let meeting = self
            .store
            .get(id)?
            .ok_or_else(|| anyhow!("Meeting {} not found", id))?;
        if !meeting.has_audio {
            return Err(anyhow!("The meeting's audio is already deleted"));
        }
        if meeting.status == MeetingStatus::Recording {
            return Err(anyhow!("The meeting is still being recorded"));
        }
        self.update(id, |m| {
            m.status = MeetingStatus::Waiting;
            m.error = None;
            m.progress = None;
        })?;
        self.app
            .state::<Arc<TranscriptionJobManager>>()
            .queue_meeting(id);
        Ok(())
    }

    /// Deletes the meeting's recorded audio, keeping its transcript.
    pub fn remove_audio(&self, id: &str) {
        let dir = self.dir(id);
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                error!("Failed to delete {}: {}", dir.display(), e);
            }
        }
    }

    /// A meeting that was recording when Handy last stopped: make its
    /// tracks readable and let the user decide what to do with it.
    fn recover_interrupted(&self) {
        let ids = match self.store.ids_with_status(&[MeetingStatus::Recording]) {
            Ok(ids) => ids,
            Err(e) => {
                error!("Failed to look for unfinished meetings: {}", e);
                return;
            }
        };
        for id in ids {
            let dir = self.dir(&id);
            let mut samples = 0u64;
            let mut tracks = Vec::new();
            for kind in [TrackKind::Mic, TrackKind::Loopback] {
                let partial = capture::partial_path(&dir, kind);
                if !partial.exists() {
                    continue;
                }
                match capture::repair_partial(&partial).and_then(|n| {
                    std::fs::rename(&partial, capture::track_path(&dir, kind))?;
                    Ok(n)
                }) {
                    Ok(0) => {}
                    Ok(n) => {
                        samples = samples.max(n);
                        // The timing saved at the last flush.
                        let (start_offset_ms, scale) = capture::TrackClock::read(&dir, kind)
                            .map(|c| c.timing())
                            .unwrap_or((0, 1.0));
                        tracks.push(TrackTiming {
                            track: kind.name().to_string(),
                            start_offset_ms,
                            scale,
                        });
                    }
                    Err(e) => warn!("Couldn't recover {}: {}", partial.display(), e),
                }
            }
            let duration_ms = (samples * 1000 / WHISPER_SAMPLE_RATE as u64) as i64;
            warn!(
                "Recovered meeting {} interrupted after {} s",
                id,
                duration_ms / 1000
            );
            let result = self.store.update(&id, |m| {
                m.duration_ms = duration_ms;
                m.tracks = tracks.clone();
                m.has_audio = !tracks.is_empty();
                if m.has_audio {
                    m.status = MeetingStatus::Interrupted;
                } else {
                    m.status = MeetingStatus::Failed;
                    m.error = Some("no_audio".into());
                }
            });
            if let Err(e) = result {
                error!("Failed to recover meeting {}: {}", id, e);
            }
        }
    }

    /// Requeues meetings whose transcription Handy was in the middle of when
    /// it last stopped. Called once the job manager exists.
    pub fn resume_pending(&self) {
        let pending = [
            MeetingStatus::Waiting,
            MeetingStatus::Transcribing,
            MeetingStatus::Labelling,
        ];
        let jobs = self.app.state::<Arc<TranscriptionJobManager>>();
        for id in self.store.ids_with_status(&pending).unwrap_or_default() {
            info!("Resuming the transcription of meeting {}", id);
            let _ = self.store.update(&id, |m| {
                m.status = MeetingStatus::Waiting;
                m.progress = None;
            });
            jobs.queue_meeting(&id);
        }
        // A summary that was being written: its transcript is saved, so it is
        // ready, just without a summary.
        for id in self
            .store
            .ids_with_status(&[MeetingStatus::Summarising])
            .unwrap_or_default()
        {
            let _ = self.store.update(&id, |m| {
                m.status = MeetingStatus::Ready;
                m.summary_error = Some("interrupted".into());
            });
        }
    }
}

impl Active {
    fn info(&self) -> RecordingInfo {
        RecordingInfo {
            id: self.id.clone(),
            started_at: self.started_at,
            bookmarks: self.bookmarks,
            dictating: self.dictation_from_ms.is_some(),
        }
    }
}

fn spawn_levels(app: AppHandle, levels: Arc<capture::Levels>, done: Arc<AtomicBool>) {
    let spawned = thread::Builder::new()
        .name("meeting-levels".into())
        .spawn(move || {
            while !done.load(Ordering::Relaxed) {
                thread::sleep(LEVELS_EVERY);
                let (me, them) = levels.take();
                let _ = MeetingLevelsEvent { me, them }.emit_to(&app, widgets::PILL_LABEL);
            }
        });
    if let Err(e) = spawned {
        error!("Failed to start the meeting level meter: {}", e);
    }
}

/// Keeps a child process from flashing a console window on Windows.
pub(crate) fn hide_console(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// The meeting manager, once Handy has set it up.
pub fn manager(app: &AppHandle) -> Option<Arc<MeetingManager>> {
    app.try_state::<Arc<MeetingManager>>()
        .map(|m| Arc::clone(&m))
}

/// Starts a meeting, or stops the one being recorded (the shortcut and the
/// tray's first item).
pub fn toggle(app: &AppHandle) {
    let Some(manager) = manager(app) else {
        return;
    };
    let result = if manager.is_recording() {
        manager.stop().map(|_| ())
    } else {
        manager.start().map(|_| ())
    };
    if let Err(e) = result {
        error!("Meeting start/stop failed: {}", e);
        let _ = tauri::Emitter::emit(app, "meeting-error", e.to_string());
    }
}

/// Bookmarks the meeting being recorded, if any.
pub fn bookmark(app: &AppHandle) {
    if let Some(manager) = manager(app) {
        if manager.is_recording() {
            if let Err(e) = manager.bookmark() {
                error!("Meeting bookmark failed: {}", e);
            }
        }
    }
}

/// Shortcut actions for the meeting bindings.
pub struct MeetingToggleAction;
pub struct MeetingBookmarkAction;

impl crate::actions::ShortcutAction for MeetingToggleAction {
    fn start(&self, app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        let app = app.clone();
        // Starting opens audio devices; keep it off the shortcut thread.
        thread::spawn(move || toggle(&app));
    }

    fn stop(&self, _app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {}
}

impl crate::actions::ShortcutAction for MeetingBookmarkAction {
    fn start(&self, app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        bookmark(app);
    }

    fn stop(&self, _app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {}
}
