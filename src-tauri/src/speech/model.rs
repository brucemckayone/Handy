//! The voice model: Supertonic 3 (int8) in sherpa-onnx's layout, downloaded
//! from Hugging Face only when the user asks, every file checked by hash.

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use specta::Type;
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::AppHandle;
use tauri_specta::Event;

const REPOSITORY: &str = "csukuangfj2/sherpa-onnx-supertonic-3-tts-int8-2026-05-11";
const REVISION: &str = "cca5a0e6c96e1d2c720986bf7e75fcc81dee3ae4";
const DIR_NAME: &str = "supertonic-3-int8";

/// (file name, size in bytes, sha256)
const FILES: &[(&str, u64, &str)] = &[
    (
        "duration_predictor.int8.onnx",
        3_700_147,
        "c3eb91414d5ff8a7a239b7fe9e34e7e2bf8a8140d8375ffb14718b1c639325db",
    ),
    (
        "text_encoder.int8.onnx",
        36_416_150,
        "c7befd5ea8c3119769e8a6c1486c4edc6a3bc8365c67621c881bbb774b9902ff",
    ),
    (
        "vector_estimator.int8.onnx",
        78_400_833,
        "20cd86fa5c6effedfda0e7cffe5b0569ca401c440a0c3a1d72bf39286c0db3fd",
    ),
    (
        "vocoder.int8.onnx",
        25_991_073,
        "e923d60f53f95eb1ce235f1dc33ec56d9c057823c96fa6f8acf98f32b0da6152",
    ),
    (
        "tts.json",
        8_253,
        "42078d3aef1cd43ab43021f3c54f47d2d75ceb4e75f627f118890128b06a0d09",
    ),
    (
        "unicode_indexer.bin",
        262_144,
        "8402ca48e5189a8950138580b0fff64db6f072f24ac07cd54ba8b2fbb9883b30",
    ),
    (
        "voice.bin",
        517_168,
        "67d5209b0ee8ce6c74105ffbe12fe6a7628aea3b4ba2fcb308a4a67938a93ce8",
    ),
    (
        "LICENSE",
        1_070,
        "0dfe0d0ba84416fe3879d9a34f4909d8d0137c78d1e95834177b0414ac096fa2",
    ),
];

pub fn model_bytes() -> u64 {
    FILES.iter().map(|(_, size, _)| size).sum()
}

/// Progress of the voice model download.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct SpeechModelEvent {
    pub downloaded: u64,
    pub total: u64,
    pub done: bool,
    pub error: Option<String>,
}

pub fn model_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(crate::portable::app_data_dir(app)?
        .join("speech-models")
        .join(DIR_NAME))
}

pub fn model_installed(app: &AppHandle) -> bool {
    model_dir(app).is_ok_and(|dir| FILES.iter().all(|(name, _, _)| dir.join(name).exists()))
}

/// Downloads the files that are missing, checking each one's hash.
pub async fn download_model(app: &AppHandle) -> Result<()> {
    let dir = model_dir(app)?;
    std::fs::create_dir_all(&dir)?;
    let total = model_bytes();
    let mut done_before = 0u64;
    let mut result = Ok(());
    for (name, size, sha256) in FILES {
        let path = dir.join(name);
        if !path.exists() {
            if let Err(e) = download_file(app, &path, name, sha256, done_before, total).await {
                result = Err(e);
                break;
            }
        }
        done_before += size;
    }
    let _ = SpeechModelEvent {
        downloaded: if result.is_ok() { total } else { 0 },
        total,
        done: true,
        error: result.as_ref().err().map(|e| e.to_string()),
    }
    .emit(app);
    result
}

async fn download_file(
    app: &AppHandle,
    path: &Path,
    name: &str,
    sha256: &str,
    done_before: u64,
    total: u64,
) -> Result<()> {
    let url = format!("https://huggingface.co/{REPOSITORY}/resolve/{REVISION}/{name}");
    info!("Downloading {}", url);
    let response = reqwest::get(&url).await?.error_for_status()?;
    let partial = partial_path(path);
    let mut file = std::fs::File::create(&partial)?;
    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;
    let mut last_reported = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk)?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        if downloaded - last_reported >= 1 << 20 {
            last_reported = downloaded;
            let _ = SpeechModelEvent {
                downloaded: done_before + downloaded,
                total,
                done: false,
                error: None,
            }
            .emit(app);
        }
    }
    file.flush()?;
    drop(file);
    if format!("{:x}", hasher.finalize()) != sha256 {
        let _ = std::fs::remove_file(&partial);
        return Err(anyhow!("The voice model download is corrupt"));
    }
    std::fs::rename(&partial, path)?;
    Ok(())
}

fn partial_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    path.with_file_name(name)
}

/// Lets a stale download be retried: removes half-written files.
pub fn clear_partial(app: &AppHandle) {
    let Ok(dir) = model_dir(app) else { return };
    for (name, _, _) in FILES {
        let partial = partial_path(&dir.join(name));
        if partial.exists() {
            if let Err(e) = std::fs::remove_file(&partial) {
                warn!("Failed to remove {}: {}", partial.display(), e);
            }
        }
    }
}
