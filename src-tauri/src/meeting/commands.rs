//! Tauri commands for meetings.

use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;

use super::detect::{self, CallApp, PromptChoice};
use super::store::{Meeting, MeetingListItem};
use super::widgets::{self, PromptState};
use super::{speakers, summary, MeetingManager, RecordingInfo};
use crate::settings::{get_settings, write_settings, MeetingSettings, SummaryEngine};

fn manager(app: &AppHandle) -> Result<Arc<MeetingManager>, String> {
    super::manager(app).ok_or_else(|| "Meetings aren't set up yet".to_string())
}

/// Runs `work` off the main thread: opening and closing audio devices and
/// finishing files take a moment, and the windows must not freeze for it.
async fn off_main<T: Send + 'static>(
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn start_meeting(app: AppHandle) -> Result<RecordingInfo, String> {
    let manager = manager(&app)?;
    off_main(move || manager.start()).await
}

#[tauri::command]
#[specta::specta]
pub async fn stop_meeting(app: AppHandle) -> Result<String, String> {
    let manager = manager(&app)?;
    off_main(move || manager.stop()).await
}

#[tauri::command]
#[specta::specta]
pub async fn add_meeting_bookmark(app: AppHandle) -> Result<u32, String> {
    let manager = manager(&app)?;
    off_main(move || manager.bookmark()).await
}

#[tauri::command]
#[specta::specta]
pub fn get_meeting_recording(app: AppHandle) -> Option<RecordingInfo> {
    super::manager(&app).and_then(|m| m.recording())
}

#[tauri::command]
#[specta::specta]
pub fn list_meetings(
    app: AppHandle,
    query: Option<String>,
) -> Result<Vec<MeetingListItem>, String> {
    manager(&app)?
        .store()
        .list(query.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn get_meeting(app: AppHandle, id: String) -> Result<Option<Meeting>, String> {
    manager(&app)?.store().get(&id).map_err(|e| e.to_string())
}

fn update(app: &AppHandle, id: &str, change: impl FnOnce(&mut Meeting)) -> Result<(), String> {
    manager(app)?
        .update(id, change)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn rename_meeting(app: AppHandle, id: String, title: String) -> Result<(), String> {
    update(&app, &id, |m| {
        m.title = title.trim().to_string();
        // Clearing the title lets the summary suggest one again.
        m.title_is_suggested = m.title.is_empty();
    })
}

/// Names a speaker for this meeting only; `None` or empty goes back to the
/// default label.
#[tauri::command]
#[specta::specta]
pub fn rename_meeting_speaker(
    app: AppHandle,
    id: String,
    speaker: String,
    name: Option<String>,
) -> Result<(), String> {
    let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    update(&app, &id, |m| {
        if let Some(s) = m.speakers.iter_mut().find(|s| s.id == speaker) {
            s.name = name;
        }
    })
}

#[tauri::command]
#[specta::specta]
pub fn set_meeting_starred(app: AppHandle, id: String, starred: bool) -> Result<(), String> {
    update(&app, &id, |m| m.starred = starred)
}

#[tauri::command]
#[specta::specta]
pub fn set_meeting_action_done(
    app: AppHandle,
    id: String,
    index: u32,
    done: bool,
) -> Result<(), String> {
    update(&app, &id, |m| {
        if let Some(item) = m
            .summary
            .as_mut()
            .and_then(|s| s.action_items.get_mut(index as usize))
        {
            item.done = done;
        }
    })
}

#[tauri::command]
#[specta::specta]
pub fn delete_meeting(app: AppHandle, id: String) -> Result<(), String> {
    manager(&app)?.delete(&id).map_err(|e| e.to_string())
}

/// Transcribes an interrupted or failed meeting from the audio it kept.
#[tauri::command]
#[specta::specta]
pub fn retry_meeting(app: AppHandle, id: String) -> Result<(), String> {
    manager(&app)?.retry(&id).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn summarise_meeting(
    app: AppHandle,
    id: String,
    engine: Option<SummaryEngine>,
) -> Result<(), String> {
    manager(&app)?;
    summary::spawn(app, id, engine);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_meeting_settings(app: AppHandle, settings: MeetingSettings) -> Result<(), String> {
    let mut all = get_settings(&app);
    all.meeting = settings;
    write_settings(&app, all);
    Ok(())
}

/// What the meeting's call audio plays through, for the headphones advice.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct MeetingOutputInfo {
    pub name: Option<String>,
    /// The mic will hear other people too, so they may be transcribed twice.
    pub is_speakers: bool,
}

#[tauri::command]
#[specta::specta]
pub fn get_meeting_output_info(app: AppHandle) -> MeetingOutputInfo {
    use rodio::cpal::traits::DeviceTrait;
    use rodio::cpal::DeviceType;

    let wanted = get_settings(&app).meeting.output_device;
    let host = crate::audio_toolkit::get_cpal_host();
    let description = super::capture::find_device(
        &host,
        super::capture::TrackKind::Loopback,
        wanted.as_deref(),
    )
    .and_then(|(device, _)| device.description().ok());
    MeetingOutputInfo {
        name: description.as_ref().map(|d| d.name().to_string()),
        is_speakers: description.is_some_and(|d| d.device_type() == DeviceType::Speaker),
    }
}

/// Where Claude Code is installed, if Handy can find it.
#[tauri::command]
#[specta::specta]
pub fn find_claude_code() -> Option<String> {
    summary::find_claude().map(|p| p.display().to_string())
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct SpeakerModelStatus {
    pub installed: bool,
    pub size_bytes: u64,
}

#[tauri::command]
#[specta::specta]
pub fn get_speaker_model_status(app: AppHandle) -> SpeakerModelStatus {
    SpeakerModelStatus {
        installed: speakers::model_installed(&app),
        size_bytes: speakers::MODEL_BYTES,
    }
}

/// Downloads the model that tells speakers apart. Only when the user asks.
#[tauri::command]
#[specta::specta]
pub async fn download_speaker_model(app: AppHandle) -> Result<(), String> {
    speakers::clear_partial(&app);
    speakers::download_model(&app)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn respond_to_meeting_prompt(
    app: AppHandle,
    choice: PromptChoice,
    call: Option<CallApp>,
) -> Result<(), String> {
    off_main(move || {
        detect::respond(&app, choice, call);
        Ok(())
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub fn get_meeting_prompt_state() -> Option<PromptState> {
    widgets::prompt_state()
}

/// Opens the Meetings page in the main window.
#[tauri::command]
#[specta::specta]
pub fn open_meetings(app: AppHandle) {
    show_meetings(&app);
}

pub fn show_meetings(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    let _ = tauri::Emitter::emit(app, "open-meetings", ());
}

#[tauri::command]
#[specta::specta]
pub fn copy_meeting_text(app: AppHandle, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

/// Writes an exported meeting to the file the user picked.
#[tauri::command]
#[specta::specta]
pub fn save_meeting_markdown(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, contents).map_err(|e| e.to_string())
}
