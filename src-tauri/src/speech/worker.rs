//! The voice model runs in a child process (`--speak-worker <model-dir>`), as
//! transcription does: a native crash ends only the worker, and unloading the
//! model is just ending the process.
//!
//! Protocol: the worker prints one JSON line when ready. Each request is a
//! JSON line on its stdin; each reply is a JSON line on its stdout, followed
//! for audio by that many little-endian f32 samples.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const WORKER_FLAG: &str = "--speak-worker";

#[derive(Serialize, Deserialize)]
struct Request {
    text: String,
    voice: u32,
    speed: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
enum Reply {
    Ready { rate: u32, voices: u32 },
    Audio { samples: usize },
    Error { message: String },
}

/// The parent's handle on a running worker. Dropping it ends the worker.
pub struct Engine {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    pub rate: u32,
    pub voices: u32,
}

impl Engine {
    /// Starts a worker and waits for it to load the model.
    pub fn start(model_dir: &Path) -> Result<Self> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg(WORKER_FLAG)
            .arg(model_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        crate::meeting::hide_console(&mut command);
        let mut child = command.spawn()?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(anyhow!("The speech worker has no pipes"));
        };
        let mut engine = Engine {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            rate: 0,
            voices: 0,
        };
        match engine.read_reply()? {
            Reply::Ready { rate, voices } => {
                engine.rate = rate;
                engine.voices = voices;
                Ok(engine)
            }
            Reply::Error { message } => Err(anyhow!(message)),
            Reply::Audio { .. } => Err(anyhow!("The speech worker didn't start properly")),
        }
    }

    /// Renders `text` as mono samples at [`Engine::rate`].
    pub fn render(&mut self, text: &str, voice: u32, speed: f32) -> Result<Vec<f32>> {
        let request = Request {
            text: text.to_string(),
            voice,
            speed,
        };
        writeln!(self.stdin, "{}", serde_json::to_string(&request)?)?;
        self.stdin.flush()?;
        match self.read_reply()? {
            Reply::Audio { samples } => {
                let mut bytes = vec![0u8; samples * 4];
                self.stdout.read_exact(&mut bytes)?;
                Ok(bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect())
            }
            Reply::Error { message } => Err(anyhow!(message)),
            Reply::Ready { .. } => Err(anyhow!("The speech worker replied out of turn")),
        }
    }

    fn read_reply(&mut self) -> Result<Reply> {
        let mut line = String::new();
        if self.stdout.read_line(&mut line)? == 0 {
            return Err(anyhow!("The speech worker stopped"));
        }
        Ok(serde_json::from_str(line.trim())?)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Whether this process was started as the speech worker.
pub fn is_worker_invocation() -> bool {
    std::env::args().nth(1).as_deref() == Some(WORKER_FLAG)
}

/// The worker's main loop. Returns the exit code.
pub fn run_worker() -> i32 {
    let Some(dir) = std::env::args().nth(2) else {
        eprintln!("usage: handy {WORKER_FLAG} <model-dir>");
        return 2;
    };
    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let tts = match load(Path::new(&dir)) {
        Ok(tts) => tts,
        Err(e) => {
            let _ = send(
                &mut out,
                &Reply::Error {
                    message: e.to_string(),
                },
            );
            return 1;
        }
    };
    let ready = Reply::Ready {
        rate: tts.sample_rate().max(0) as u32,
        voices: tts.num_speakers().max(0) as u32,
    };
    if send(&mut out, &ready).is_err() {
        return 1;
    }
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let result = serde_json::from_str::<Request>(&line)
            .map_err(|e| anyhow!("Bad request: {e}"))
            .and_then(|request| render(&tts, &request));
        let sent = match result {
            Ok(samples) => send(
                &mut out,
                &Reply::Audio {
                    samples: samples.len(),
                },
            )
            .and_then(|_| {
                for sample in &samples {
                    out.write_all(&sample.to_le_bytes())?;
                }
                out.flush()
            }),
            Err(e) => send(
                &mut out,
                &Reply::Error {
                    message: e.to_string(),
                },
            ),
        };
        if sent.is_err() {
            break;
        }
    }
    0
}

fn send(out: &mut impl Write, reply: &Reply) -> std::io::Result<()> {
    writeln!(out, "{}", serde_json::to_string(reply)?)?;
    out.flush()
}

fn load(dir: &Path) -> Result<sherpa_onnx::OfflineTts> {
    use sherpa_onnx::{OfflineTtsConfig, OfflineTtsModelConfig, OfflineTtsSupertonicModelConfig};

    let file = |name: &str| Some(dir.join(name).to_string_lossy().into_owned());
    // More threads than this doesn't help this model on a laptop CPU.
    let threads = std::thread::available_parallelism()
        .map(|n| (n.get() / 2).clamp(1, 4))
        .unwrap_or(2);
    let config = OfflineTtsConfig {
        model: OfflineTtsModelConfig {
            supertonic: OfflineTtsSupertonicModelConfig {
                duration_predictor: file("duration_predictor.int8.onnx"),
                text_encoder: file("text_encoder.int8.onnx"),
                vector_estimator: file("vector_estimator.int8.onnx"),
                vocoder: file("vocoder.int8.onnx"),
                tts_json: file("tts.json"),
                unicode_indexer: file("unicode_indexer.bin"),
                voice_style: file("voice.bin"),
            },
            num_threads: threads as i32,
            provider: Some("cpu".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    sherpa_onnx::OfflineTts::create(&config)
        .ok_or_else(|| anyhow!("The voice model failed to load"))
}

fn render(tts: &sherpa_onnx::OfflineTts, request: &Request) -> Result<Vec<f32>> {
    let mut extra = std::collections::HashMap::new();
    extra.insert("lang".to_string(), serde_json::Value::String("en".into()));
    let config = sherpa_onnx::GenerationConfig {
        sid: request.voice as i32,
        speed: request.speed,
        extra: Some(extra),
        ..Default::default()
    };
    let audio = tts
        .generate_with_config::<fn(&[f32], f32) -> bool>(&request.text, &config, None)
        .ok_or_else(|| anyhow!("The voice model couldn't read that"))?;
    Ok(audio.samples().to_vec())
}
