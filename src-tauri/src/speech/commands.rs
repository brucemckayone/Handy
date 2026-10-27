use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;

use super::{model, SpeakRefusal};
use crate::settings::{get_settings, write_settings, SpeechSettings};

/// The voice model's stock voices, in its own order and with its own names
/// (F for the higher voices, M for the lower).
const VOICES: [&str; 10] = ["F1", "F2", "F3", "F4", "F5", "M1", "M2", "M3", "M4", "M5"];

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct SpeechModelStatus {
    pub installed: bool,
    pub bytes: u64,
    pub voices: Vec<String>,
}

#[tauri::command]
#[specta::specta]
pub fn get_speech_model_status(app: AppHandle) -> SpeechModelStatus {
    SpeechModelStatus {
        installed: model::model_installed(&app),
        bytes: model::model_bytes(),
        voices: VOICES.iter().map(|v| v.to_string()).collect(),
    }
}

#[tauri::command]
#[specta::specta]
pub async fn download_speech_model(app: AppHandle) -> Result<(), String> {
    model::download_model(&app).await.map_err(|e| e.to_string())
}

/// Reads `text` after anything already queued.
#[tauri::command]
#[specta::specta]
pub fn speak_text(app: AppHandle, text: String) -> Result<(), SpeakRefusal> {
    match super::manager(&app) {
        Some(speech) => speech.speak(&text),
        None => Err(SpeakRefusal::NotInstalled),
    }
}

/// Stops reading and clears the queue.
#[tauri::command]
#[specta::specta]
pub fn stop_speaking(app: AppHandle) {
    super::stop(&app);
}

#[tauri::command]
#[specta::specta]
pub fn is_speaking(app: AppHandle) -> bool {
    super::manager(&app).is_some_and(|s| s.is_speaking())
}

#[tauri::command]
#[specta::specta]
pub fn change_speech_settings(app: AppHandle, settings: SpeechSettings) -> Result<(), String> {
    let mut all = get_settings(&app);
    all.speech = settings;
    write_settings(&app, all);
    Ok(())
}
