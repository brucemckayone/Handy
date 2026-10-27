//! Tells the people on the call audio apart, within one meeting: Speaker 1-4
//! from Sortformer (English, up to four speakers). Nothing about a voice is
//! kept: the model's turns become labels on the transcript and are dropped.
//!
//! The model runs in a child process (`--diarize-worker`), so a native crash
//! fails only the labelling, and its memory is freed as soon as it is done.
//! It is downloaded only when the user asks for it.

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use specta::Type;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::AppHandle;
use tauri_specta::Event;

use super::store::TranscriptSegment;

const REPOSITORY: &str = "handy-computer/diar_streaming_sortformer_4spk-v2.1-gguf";
const REVISION: &str = "ae4afbb5c3d33b71cf2dbf600022b655ee706dd0";
const FILE_NAME: &str = "diar_streaming_sortformer_4spk-v2.1-Q8_0.gguf";
const SHA256: &str = "a5dacdc650790266c7a362e54e6bf51952015487edaa606c4e11632bc32442a9";
pub const MODEL_BYTES: u64 = 139_310_336;

const WORKER_FLAG: &str = "--diarize-worker";
/// How often a running labelling checks for a cancel.
const POLL: Duration = Duration::from_millis(250);

/// One speaker's turn, in milliseconds of the call-audio track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub start_ms: i64,
    pub end_ms: i64,
    /// 1-based.
    pub speaker: i32,
}

/// Progress of the speaker model download.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct SpeakerModelEvent {
    pub downloaded: u64,
    pub total: u64,
    pub done: bool,
    pub error: Option<String>,
}

pub fn model_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(crate::portable::app_data_dir(app)?
        .join("meeting-models")
        .join(FILE_NAME))
}

pub fn model_installed(app: &AppHandle) -> bool {
    model_path(app).is_ok_and(|p| p.exists())
}

/// Downloads the speaker model from Hugging Face, checking its hash.
pub async fn download_model(app: &AppHandle) -> Result<()> {
    let path = model_path(app)?;
    if path.exists() {
        return Ok(());
    }
    let result = download_to(app, &path).await;
    let _ = SpeakerModelEvent {
        downloaded: if result.is_ok() { MODEL_BYTES } else { 0 },
        total: MODEL_BYTES,
        done: true,
        error: result.as_ref().err().map(|e| e.to_string()),
    }
    .emit(app);
    result
}

async fn download_to(app: &AppHandle, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let url = format!("https://huggingface.co/{REPOSITORY}/resolve/{REVISION}/{FILE_NAME}");
    info!("Downloading the speaker model from {}", url);
    let response = reqwest::get(&url).await?.error_for_status()?;
    let total = response.content_length().unwrap_or(MODEL_BYTES);
    let partial = path.with_extension("gguf.partial");
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
            let _ = SpeakerModelEvent {
                downloaded,
                total,
                done: false,
                error: None,
            }
            .emit(app);
        }
    }
    file.flush()?;
    drop(file);
    let digest = format!("{:x}", hasher.finalize());
    if digest != SHA256 {
        let _ = std::fs::remove_file(&partial);
        return Err(anyhow!("The speaker model download is corrupt"));
    }
    std::fs::rename(&partial, path)?;
    info!("Speaker model installed at {}", path.display());
    Ok(())
}

/// Who spoke when on the call-audio track at `wav`. `None` if the model
/// isn't installed or the labelling was cancelled.
pub fn label(app: &AppHandle, wav: &Path, cancelled: &AtomicBool) -> Result<Option<Vec<Turn>>> {
    let model = model_path(app)?;
    if !model.exists() {
        info!("The speaker model isn't installed; leaving the call audio as one speaker");
        return Ok(None);
    }
    let exe = std::env::current_exe()?;
    let mut command = Command::new(exe);
    command
        .arg(WORKER_FLAG)
        .arg(&model)
        .arg(wav)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    super::hide_console(&mut command);
    let mut child = command.spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("No worker output"))?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        std::io::Read::read_to_string(&mut stdout, &mut out).map(|_| out)
    });
    loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        if let Some(status) = child.try_wait()? {
            let out = reader
                .join()
                .map_err(|_| anyhow!("The speaker worker's output was lost"))??;
            if !status.success() {
                return Err(anyhow!("The speaker worker failed ({})", status));
            }
            let turns: Vec<Turn> = serde_json::from_str(out.trim())?;
            info!("Found {} speaker turns in the call audio", turns.len());
            return Ok(Some(turns));
        }
        std::thread::sleep(POLL);
    }
}

/// Labels each call-audio segment with the speaker who talks most during it.
/// `to_meeting_ms` maps the track's time onto the meeting's.
pub fn assign(
    segments: &mut [TranscriptSegment],
    turns: &[Turn],
    to_meeting_ms: impl Fn(i64) -> i64,
) {
    if turns.is_empty() {
        return;
    }
    let turns: Vec<(i64, i64, i32)> = turns
        .iter()
        .map(|t| {
            (
                to_meeting_ms(t.start_ms),
                to_meeting_ms(t.end_ms),
                t.speaker,
            )
        })
        .collect();
    for segment in segments.iter_mut().filter(|s| s.speaker == "them") {
        let mut talk = [0i64; 5];
        for &(start, end, speaker) in &turns {
            let overlap = end.min(segment.end_ms) - start.max(segment.start_ms);
            if overlap > 0 && (1..=4).contains(&speaker) {
                talk[speaker as usize] += overlap;
            }
        }
        let best = (1..=4).max_by_key(|&s| talk[s]).unwrap_or(1);
        let speaker = if talk[best] > 0 {
            best as i32
        } else {
            // Said between turns: the nearest one.
            let middle = (segment.start_ms + segment.end_ms) / 2;
            turns
                .iter()
                .min_by_key(|&&(start, end, _)| {
                    if middle < start {
                        start - middle
                    } else {
                        (middle - end).max(0)
                    }
                })
                .map(|t| t.2)
                .unwrap_or(1)
        };
        segment.speaker = format!("s{}", speaker.clamp(1, 4));
    }
}

/// Whether this process was started as the speaker worker.
pub fn is_worker_invocation() -> bool {
    std::env::args().nth(1).as_deref() == Some(WORKER_FLAG)
}

/// The speaker worker: `handy --diarize-worker <model> <wav>` prints the
/// turns as JSON. Returns the exit code.
pub fn run_worker() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let (Some(model), Some(wav)) = (args.get(2), args.get(3)) else {
        eprintln!("usage: handy {WORKER_FLAG} <model> <wav>");
        return 2;
    };
    match diarize(Path::new(model), Path::new(wav)) {
        Ok(turns) => match serde_json::to_string(&turns) {
            Ok(json) => {
                println!("{json}");
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn diarize(model_path: &Path, wav: &Path) -> Result<Vec<Turn>> {
    use transcribe_cpp::{Backend, BackendMask, DiarizeOptions, Model, ModelOptions};

    transcribe_cpp::init_backends_with(None::<&Path>, BackendMask::CPU)
        .map_err(|e| anyhow!("Failed to start the CPU backend: {e}"))?;
    let model = Model::load_with(
        model_path,
        &ModelOptions {
            backend: Backend::Cpu,
            device: None,
        },
    )
    .map_err(|e| anyhow!("Failed to load the speaker model: {e}"))?;
    let mut session = model
        .diarize_session()
        .map_err(|e| anyhow!("The speaker model can't tell speakers apart: {e}"))?;
    let pcm: Vec<f32> = hound::WavReader::open(wav)?
        .into_samples::<i16>()
        .map(|s| s.map(|s| s as f32 / 32768.0))
        .collect::<Result<_, _>>()?;
    let segments = session
        .run(&pcm, &DiarizeOptions::default())
        .map_err(|e| anyhow!("Telling speakers apart failed: {e}"))?;
    Ok(segments
        .into_iter()
        .filter(|s| s.t1_ms > s.t0_ms && s.speaker_id > 0)
        .map(|s| Turn {
            start_ms: s.t0_ms,
            end_ms: s.t1_ms,
            speaker: s.speaker_id,
        })
        .collect())
}

/// Lets a stale download be retried: removes a half-written file.
pub fn clear_partial(app: &AppHandle) {
    if let Ok(path) = model_path(app) {
        let partial = path.with_extension("gguf.partial");
        if partial.exists() {
            if let Err(e) = std::fs::remove_file(&partial) {
                warn!("Failed to remove {}: {}", partial.display(), e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn them(start_ms: i64, end_ms: i64) -> TranscriptSegment {
        TranscriptSegment {
            start_ms,
            end_ms,
            speaker: "them".into(),
            text: "words".into(),
        }
    }

    #[test]
    fn each_segment_takes_the_speaker_who_talks_most_in_it() {
        let mut segments = vec![
            them(0, 1_000),
            them(1_000, 3_000),
            them(5_000, 5_500),
            TranscriptSegment {
                speaker: "me".into(),
                ..them(0, 1_000)
            },
        ];
        let turns = [
            Turn {
                start_ms: 0,
                end_ms: 1_200,
                speaker: 2,
            },
            Turn {
                start_ms: 1_200,
                end_ms: 4_000,
                speaker: 1,
            },
        ];
        assign(&mut segments, &turns, |ms| ms);
        let speakers: Vec<&str> = segments.iter().map(|s| s.speaker.as_str()).collect();
        assert_eq!(speakers, ["s2", "s1", "s1", "me"]);
    }
}
