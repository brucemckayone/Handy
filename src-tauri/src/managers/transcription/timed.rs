//! Background transcription that keeps when each line was said, for a
//! meeting track: the audio is read in feeds, so an hour-long track is never
//! held whole, and chunks with no sound are skipped.

use anyhow::Result;
use log::{debug, info, warn};
use std::time::Instant;

use super::ChunkControl;
use super::{
    engine_error, post_process_transcription_text, real_time_factor, BatchContext,
    TranscriptionManager,
};
use crate::audio_toolkit::chunking::Chunker;
use crate::engine_supervisor::EngineError;
use crate::managers::audio::create_detector;

/// Tries per chunk when a dictation's cancel interrupts it.
const MAX_CHUNK_ATTEMPTS: usize = 3;

/// A chunk whose loudest sample is quieter than this (about -46 dBFS) has
/// no speech: call audio is digital silence between talkers, and a quiet
/// room stays well under it.
const SILENT_PEAK: f32 = 0.005;

const SAMPLES_PER_MS: i64 = 16;

/// A line of transcript and when it was said, in milliseconds from the
/// start of the audio.
#[derive(Clone, Debug, PartialEq)]
pub struct TimedText {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

impl TranscriptionManager {
    /// Transcribes 16 kHz audio arriving as `feeds`, chunk by chunk, with
    /// the selected model. The first `base` samples of the track were done
    /// before (a resumed transcription); `total` is the whole track.
    /// `on_chunk` gets each chunk's lines, timed from the track's start, and
    /// the samples done so far, so a transcription can resume from there.
    /// `false` if `control` stopped it.
    pub fn transcribe_timed(
        &self,
        feeds: &mut dyn Iterator<Item = Result<Vec<f32>>>,
        base: usize,
        total: usize,
        control: &dyn ChunkControl,
        on_chunk: &mut dyn FnMut(Vec<TimedText>, usize) -> Result<()>,
    ) -> Result<bool> {
        let started = Instant::now();
        if base >= total {
            return Ok(true);
        }

        self.touch_activity();
        self.initiate_model_load();
        // Language detection hears the first feed, which then goes back in
        // front of the rest: the track streams, so there's no whole clip.
        let first = feeds.next().transpose()?;
        let BatchContext {
            settings,
            active_model,
            setup,
            ..
        } = self.batch_context(first.as_deref().unwrap_or_default())?;
        let mut feeds = first.map(Ok).into_iter().chain(feeds);

        let backend = settings.vad_backend;
        let app_handle = self.app_handle.clone();
        let new_chunker = || {
            let app_handle = app_handle.clone();
            Chunker::new(Box::new(move || create_detector(backend, &app_handle)))
        };
        let mut chunker = new_chunker();

        let mut line_count = 0usize;
        let mut offset = base;
        let mut skipped = 0usize;
        control.progress(offset, total);
        loop {
            if !control.proceed() {
                return Ok(false);
            }
            // A silent feed is skipped whole: the chunker would hold it,
            // waiting for speech, and call audio can be silent for an hour.
            let (chunks, last, silence) = match feeds.next() {
                Some(feed) => {
                    let feed = feed?;
                    if feed.iter().all(|s| s.abs() < SILENT_PEAK) {
                        let mut before = std::mem::replace(&mut chunker, new_chunker());
                        (before.finish(), false, feed.len())
                    } else {
                        (chunker.push(feed), false, 0)
                    }
                }
                None => (chunker.finish(), true, 0),
            };
            for chunk in chunks {
                let len = chunk.len();
                let chunk_start_ms = offset as i64 / SAMPLES_PER_MS;
                offset += len;
                if chunk.iter().all(|s| s.abs() < SILENT_PEAK) {
                    skipped += 1;
                    on_chunk(Vec::new(), offset)?;
                    control.progress(offset, total);
                    continue;
                }
                let mut attempts = 0;
                let transcript = loop {
                    if !control.proceed() {
                        return Ok(false);
                    }
                    attempts += 1;
                    let mark = self.engine.cancel_mark();
                    match self.run_batch(mark, chunk.clone(), &setup.run_options, &active_model) {
                        Ok(transcript) => break transcript,
                        Err(EngineError::Cancelled) if attempts < MAX_CHUNK_ATTEMPTS => {
                            warn!("A meeting chunk was cancelled; trying it again");
                        }
                        Err(e) => return Err(engine_error(e)),
                    }
                };
                let output_language = setup.output_language(&settings, transcript.language.clone());
                let chunk_end_ms = chunk_start_ms + len as i64 / SAMPLES_PER_MS;
                let tidy = |text: String| {
                    post_process_transcription_text(
                        text,
                        &settings,
                        setup.custom_words_prompted(),
                        &output_language,
                        &setup.languages,
                    )
                };
                let mut lines = Vec::new();
                let timed: Vec<_> = transcript
                    .segments
                    .into_iter()
                    .filter(|s| s.t1_ms > s.t0_ms)
                    .collect();
                if timed.is_empty() {
                    let text = tidy(transcript.text);
                    if !text.trim().is_empty() {
                        lines.push(TimedText {
                            start_ms: chunk_start_ms,
                            end_ms: chunk_end_ms,
                            text: text.trim().to_string(),
                        });
                    }
                } else {
                    for segment in timed {
                        let text = tidy(segment.text);
                        if text.trim().is_empty() {
                            continue;
                        }
                        lines.push(TimedText {
                            start_ms: chunk_start_ms + segment.t0_ms,
                            end_ms: (chunk_start_ms + segment.t1_ms).min(chunk_end_ms),
                            text: text.trim().to_string(),
                        });
                    }
                }
                line_count += lines.len();
                on_chunk(lines, offset)?;
                control.progress(offset, total);
            }
            if silence > 0 {
                offset += silence;
                skipped += 1;
                on_chunk(Vec::new(), offset)?;
                control.progress(offset, total);
            }
            if last {
                break;
            }
        }

        let elapsed_secs = started.elapsed().as_secs_f64();
        let audio_secs = (total - base) as f64 / 16_000.0;
        debug!("Skipped {} silent stretches", skipped);
        info!(
            "Timed transcription of {:.1}s of audio into {} lines took {:.1}s ({:.1}x real time)",
            audio_secs,
            line_count,
            elapsed_secs,
            real_time_factor(audio_secs, elapsed_secs)
        );
        Ok(true)
    }
}
