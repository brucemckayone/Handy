//! Transcribes a recorded meeting: each track with timestamps, leaving out
//! what the user dictated into other apps, then the call audio's speakers
//! told apart. Once the transcript is saved the audio is deleted and the
//! summary is started.

use anyhow::{anyhow, Result};
use log::{info, warn};
use std::cell::Cell;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

use super::capture::{track_path, TrackKind};
use super::store::{DictationSpan, MeetingStatus, Speaker, TrackTiming, TranscriptSegment};
use super::{MeetingManager, DICTATION_MARGIN_MS};
use crate::managers::transcription::{ChunkControl, TimedText, TranscriptionManager};
use crate::managers::transcription_jobs::wait_for_dictation;
use crate::settings::get_settings;

/// Samples read from a track at a time.
const FEED_SAMPLES: usize = 60 * 16_000;
/// Progress is saved at least this often (in track samples), so a resumed
/// transcription redoes little and long meetings aren't rewritten per chunk.
const SAVE_EVERY_SAMPLES: usize = 2 * 60 * 16_000;
const SAMPLES_PER_MS: f64 = 16.0;

/// One recorded track and how it lines up with the meeting.
struct Track {
    kind: TrackKind,
    path: PathBuf,
    timing: TrackTiming,
    samples: usize,
}

impl Track {
    fn to_meeting_ms(&self, track_ms: i64) -> i64 {
        self.timing.start_offset_ms + (track_ms as f64 * self.timing.scale).round() as i64
    }

    fn to_track_sample(&self, meeting_ms: i64) -> usize {
        let track_ms = (meeting_ms - self.timing.start_offset_ms) as f64 / self.timing.scale;
        (track_ms.max(0.0) * SAMPLES_PER_MS) as usize
    }
}

/// Transcribes meeting `id`; the job queue runs it.
pub fn run(app: &AppHandle, id: &str, cancelled: &AtomicBool) -> Result<()> {
    let manager = super::manager(app).ok_or_else(|| anyhow!("Meetings aren't set up"))?;
    let Some(meeting) = manager.store().get(id)? else {
        return Ok(());
    };
    if !meeting.has_audio {
        return Err(anyhow!("The meeting's audio is already deleted"));
    }
    let settings = get_settings(app);
    let dir = manager.dir(id);
    let mut tracks = Vec::new();
    for kind in [TrackKind::Mic, TrackKind::Loopback] {
        let path = track_path(&dir, kind);
        if !path.exists() {
            continue;
        }
        let samples = hound::WavReader::open(&path)
            .map_err(|e| anyhow!("Failed to open {}: {}", path.display(), e))?
            .len() as usize;
        let timing = meeting
            .tracks
            .iter()
            .find(|t| t.track == kind.name())
            .cloned()
            .unwrap_or(TrackTiming {
                track: kind.name().to_string(),
                start_offset_ms: 0,
                scale: 1.0,
            });
        tracks.push(Track {
            kind,
            path,
            timing,
            samples,
        });
    }
    if tracks.is_empty() {
        return Err(anyhow!("The meeting's audio is missing"));
    }
    let grand_total = tracks.iter().map(|t| t.samples).sum::<usize>();
    manager.update(id, |m| {
        m.status = MeetingStatus::Transcribing;
        m.progress = Some(0);
        m.error = None;
    })?;

    let tm = Arc::clone(&app.state::<Arc<TranscriptionManager>>());
    // Carries on from where an earlier run stopped.
    let mut partial = meeting.partial.clone().unwrap_or_default();
    let mut done_before = 0;
    for track in &tracks {
        let name = track.kind.name();
        if partial.done_tracks.iter().any(|t| t == name) {
            done_before += track.samples;
            continue;
        }
        let base = if partial.track.as_deref() == Some(name) {
            (partial.samples_done as usize).min(track.samples)
        } else {
            0
        };
        if base > 0 {
            info!(
                "Resuming meeting {}'s {} track at {} s",
                id,
                name,
                base / 16_000
            );
        }
        let leave_out = if track.kind == TrackKind::Mic && settings.meeting.leave_out_dictation {
            dictation_ranges(track, &meeting.dictations)
        } else {
            Vec::new()
        };
        let mut reader = hound::WavReader::open(&track.path)
            .map_err(|e| anyhow!("Failed to open {}: {}", track.path.display(), e))?;
        reader.seek(base as u32)?;
        let mut feeds = Feeds {
            samples: reader.into_samples::<i16>(),
            position: base,
            leave_out,
        };
        let control = Control {
            app,
            manager: &manager,
            id,
            cancelled,
            done_before,
            grand_total,
            percent: Cell::new(0),
        };
        let mut last_saved = base;
        let mut save_chunk = |lines: Vec<TimedText>, samples_done: usize| -> Result<()> {
            partial.track = Some(name.to_string());
            partial.samples_done = samples_done as u64;
            partial
                .segments
                .extend(lines.into_iter().map(|line| TranscriptSegment {
                    start_ms: track.to_meeting_ms(line.start_ms),
                    end_ms: track.to_meeting_ms(line.end_ms),
                    speaker: name.to_string(),
                    text: line.text,
                }));
            if samples_done - last_saved >= SAVE_EVERY_SAMPLES {
                last_saved = samples_done;
                let saved = partial.clone();
                manager.store().update(id, |m| m.partial = Some(saved))?;
            }
            Ok(())
        };
        if !tm.transcribe_timed(&mut feeds, base, track.samples, &control, &mut save_chunk)? {
            return Ok(());
        }
        partial.done_tracks.push(name.to_string());
        partial.track = None;
        partial.samples_done = 0;
        let saved = partial.clone();
        manager.store().update(id, |m| m.partial = Some(saved))?;
        done_before += track.samples;
    }
    let mut segments = std::mem::take(&mut partial.segments);
    segments.sort_by_key(|s| (s.start_ms, s.end_ms));

    if settings.meeting.label_speakers {
        if let Some(them) = tracks.iter().find(|t| t.kind == TrackKind::Loopback) {
            if segments.iter().any(|s| s.speaker == "them") {
                manager.update(id, |m| {
                    m.status = MeetingStatus::Labelling;
                    m.progress = None;
                })?;
                match super::speakers::label(app, &them.path, cancelled) {
                    Ok(Some(turns)) => {
                        super::speakers::assign(&mut segments, &turns, |ms| them.to_meeting_ms(ms))
                    }
                    Ok(None) => {}
                    Err(e) => warn!("Couldn't tell the speakers apart: {}", e),
                }
            }
        }
    }
    if cancelled.load(Ordering::Acquire) {
        return Ok(());
    }

    let summarise = settings.meeting.summarise_automatically && !segments.is_empty();
    let no_speech = segments.is_empty();
    let saved = manager.update(id, |m| {
        let mut speakers: Vec<Speaker> = Vec::new();
        for segment in &segments {
            if speakers.iter().all(|s| s.id != segment.speaker) {
                // A name given before, for a transcription run again.
                let name = m
                    .speakers
                    .iter()
                    .find(|s| s.id == segment.speaker)
                    .and_then(|s| s.name.clone());
                speakers.push(Speaker {
                    id: segment.speaker.clone(),
                    name,
                });
            }
        }
        speakers.sort_by_key(|s| (s.id != "me", s.id.clone()));
        m.segments = segments;
        m.speakers = speakers;
        m.partial = None;
        m.progress = None;
        m.error = no_speech.then(|| "no_speech".to_string());
        m.status = if summarise {
            MeetingStatus::Summarising
        } else {
            MeetingStatus::Ready
        };
        // The transcript is saved: the audio goes now.
        m.has_audio = false;
    })?;
    if saved.is_none() {
        return Ok(());
    }
    manager.remove_audio(id);
    info!("Meeting {} is transcribed; its audio is deleted", id);
    if summarise {
        super::summary::spawn(app.clone(), id.to_string(), None);
    }
    Ok(())
}

/// Marks meeting `id` failed; its audio is kept, so it can be tried again.
pub fn failed(app: &AppHandle, id: &str, error: &anyhow::Error) {
    warn!("Transcription of meeting {} failed: {}", id, error);
    if let Some(manager) = super::manager(app) {
        let _ = manager.update(id, |m| {
            m.status = MeetingStatus::Failed;
            m.progress = None;
            m.error = Some(error.to_string());
        });
    }
}

/// The track samples each dictation covers, with a margin either side.
fn dictation_ranges(track: &Track, dictations: &[DictationSpan]) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = dictations
        .iter()
        .map(|d| {
            (
                track.to_track_sample(d.start_ms - DICTATION_MARGIN_MS),
                track.to_track_sample(d.end_ms + DICTATION_MARGIN_MS),
            )
        })
        .collect();
    ranges.sort_unstable();
    ranges
}

/// A track read a minute at a time, silenced where the user was dictating.
struct Feeds {
    samples: hound::WavIntoSamples<BufReader<File>, i16>,
    position: usize,
    leave_out: Vec<(usize, usize)>,
}

impl Iterator for Feeds {
    type Item = Result<Vec<f32>>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut feed = Vec::with_capacity(FEED_SAMPLES);
        for sample in self.samples.by_ref().take(FEED_SAMPLES) {
            match sample {
                Ok(s) => feed.push(s as f32 / 32768.0),
                Err(e) => return Some(Err(anyhow!("Failed to read the meeting audio: {}", e))),
            }
        }
        if feed.is_empty() {
            return None;
        }
        let start = self.position;
        self.position += feed.len();
        for &(from, to) in &self.leave_out {
            let from = from.max(start);
            let to = to.min(self.position);
            if from < to {
                feed[from - start..to - start].fill(0.0);
            }
        }
        Some(Ok(feed))
    }
}

struct Control<'a> {
    app: &'a AppHandle,
    manager: &'a MeetingManager,
    id: &'a str,
    cancelled: &'a AtomicBool,
    /// Samples of earlier tracks, for progress across both.
    done_before: usize,
    grand_total: usize,
    percent: Cell<u8>,
}

impl ChunkControl for Control<'_> {
    fn proceed(&self) -> bool {
        wait_for_dictation(self.app, self.cancelled)
    }

    fn progress(&self, done: usize, _total: usize) {
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        let percent = ((self.done_before + done) * 100 / self.grand_total.max(1)).min(100) as u8;
        if self.percent.replace(percent) == percent {
            return;
        }
        let _ = self.manager.update(self.id, |m| m.progress = Some(percent));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(offset: i64, scale: f64) -> Track {
        Track {
            kind: TrackKind::Mic,
            path: PathBuf::new(),
            timing: TrackTiming {
                track: "me".into(),
                start_offset_ms: offset,
                scale,
            },
            samples: 0,
        }
    }

    #[test]
    fn track_time_maps_onto_the_meeting_clock() {
        let t = track(40, 1.0001);
        assert_eq!(t.to_meeting_ms(0), 40);
        assert_eq!(t.to_meeting_ms(3_600_000), 40 + 3_600_360);
        assert_eq!(t.to_track_sample(t.to_meeting_ms(60_000)), 60_000 * 16);
        assert_eq!(t.to_track_sample(0), 0);
    }

    #[test]
    fn dictation_is_silenced_across_feed_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("me.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut wav = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..(FEED_SAMPLES + 32_000) {
            wav.write_sample(16_384i16).unwrap();
        }
        wav.finalize().unwrap();

        let t = track(0, 1.0);
        let span = DictationSpan {
            // Straddles the first feed's end.
            start_ms: 59_000,
            end_ms: 61_000,
            text: None,
        };
        let ranges = dictation_ranges(&t, &[span]);
        let feeds = Feeds {
            samples: hound::WavReader::open(&path).unwrap().into_samples::<i16>(),
            position: 0,
            leave_out: ranges,
        };
        let audio: Vec<f32> = feeds.flat_map(|f| f.unwrap()).collect();
        assert_eq!(audio.len(), FEED_SAMPLES + 32_000);
        assert_eq!(audio[58_000 * 16], 0.5);
        assert_eq!(audio[58_800 * 16], 0.0);
        assert_eq!(audio[60_000 * 16], 0.0);
        assert_eq!(audio[61_200 * 16], 0.0);
        assert_eq!(audio[61_400 * 16], 0.5);
    }
}
