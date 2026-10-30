//! Records a meeting's two tracks side by side: the mic ("me") and the call
//! audio ("them", WASAPI loopback of an output device). Each track is
//! resampled to 16 kHz mono and streamed to its own WAV while recording, so
//! memory stays flat and a crash keeps everything up to the last flush.
//!
//! Loopback only delivers packets while something plays, so a silent output
//! stream runs on the captured device for the whole meeting (spike
//! handy-1xt.2). Holes and device restarts are filled with silence, so each
//! track's sample index stays on the meeting's clock.
//!
//! This stays outside handy-recorder on purpose: its watchdog ends streams
//! that go quiet, and a meeting's call audio is often quiet.

use anyhow::{anyhow, Result};
use log::{debug, error, info, warn};
use rodio::cpal::{
    self,
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, ErrorKind, FromSample, SampleFormat, SizedSample, Stream, StreamConfig,
};
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::audio_toolkit::audio::Resampled;
use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;

const RATE: u64 = WHISPER_SAMPLE_RATE as u64;
/// A capture-time jump this much longer than the previous packet is a hole.
const HOLE_THRESHOLD_NS: u128 = 50_000_000;
/// Rewrite the WAV header this often, so a crash loses at most this much.
const FLUSH_EVERY_SAMPLES: u64 = 5 * RATE;
/// How often the capture thread checks its streams.
const CHECK_EVERY: Duration = Duration::from_millis(500);
/// No packet for this long means a stream has stalled. Both streams always
/// deliver: the mic hears the room, and loopback hears the silent stream.
const STALL_AFTER_MS: u64 = 5_000;
/// Wait between attempts to reopen a lost device.
const RETRY_EVERY: Duration = Duration::from_secs(3);
/// Drift beyond this is a measuring error, not a clock: leave the track as is.
const MAX_DRIFT: f64 = 0.005;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    /// The user's microphone.
    Mic,
    /// What the output device plays: the other people on the call.
    Loopback,
}

impl TrackKind {
    pub fn name(self) -> &'static str {
        match self {
            TrackKind::Mic => "me",
            TrackKind::Loopback => "them",
        }
    }
}

/// A problem while recording, for the meeting's record. `kind` is a code the
/// UI translates.
#[derive(Clone, Debug)]
pub struct Problem {
    pub at_ms: i64,
    pub track: TrackKind,
    pub kind: &'static str,
}

/// One finished track.
#[derive(Debug)]
pub struct TrackReport {
    pub kind: TrackKind,
    /// The finished WAV, if any audio was written.
    pub path: Option<PathBuf>,
    /// 16 kHz samples written.
    pub samples: u64,
    pub start_offset_ms: i64,
    /// Meeting time per track time.
    pub scale: f64,
}

#[derive(Debug)]
pub struct CaptureReport {
    pub tracks: Vec<TrackReport>,
    pub problems: Vec<Problem>,
}

/// Peak level of each track since it was last read, for the recording pill.
#[derive(Default)]
pub struct Levels {
    me: AtomicU32,
    them: AtomicU32,
}

impl Levels {
    fn slot(&self, kind: TrackKind) -> &AtomicU32 {
        match kind {
            TrackKind::Mic => &self.me,
            TrackKind::Loopback => &self.them,
        }
    }

    fn raise(&self, kind: TrackKind, peak: f32) {
        let slot = self.slot(kind);
        let mut current = slot.load(Ordering::Relaxed);
        while f32::from_bits(current) < peak {
            match slot.compare_exchange_weak(
                current,
                peak.to_bits(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// `(me, them)` peaks since the last call, 0..1.
    pub fn take(&self) -> (f32, f32) {
        (
            f32::from_bits(self.me.swap(0, Ordering::Relaxed)),
            f32::from_bits(self.them.swap(0, Ordering::Relaxed)),
        )
    }
}

pub struct CaptureOptions {
    /// Mic by name; `None` for the default.
    pub mic: Option<String>,
    /// Output device by name; `None` for the default.
    pub output: Option<String>,
    /// Folder the tracks are written to.
    pub dir: PathBuf,
}

/// A recording in progress. Dropping it without `stop` still finishes the
/// files.
pub struct Capture {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<CaptureReport>>,
    levels: Arc<Levels>,
}

/// The in-progress file for a track.
pub fn partial_path(dir: &Path, kind: TrackKind) -> PathBuf {
    dir.join(format!("{}.wav.partial", kind.name()))
}

/// The finished file for a track.
pub fn track_path(dir: &Path, kind: TrackKind) -> PathBuf {
    dir.join(format!("{}.wav", kind.name()))
}

/// Where a track's timing is kept while it records, for recovery.
pub fn timing_path(dir: &Path, kind: TrackKind) -> PathBuf {
    dir.join(format!("{}.timing.json", kind.name()))
}

/// A recording track's timing so far, rewritten at every flush.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TrackClock {
    /// Meeting time the track's first sample was captured.
    pub first_ms: f64,
    /// Meeting time its latest audio ended.
    pub last_end_ms: f64,
    /// 16 kHz samples written.
    pub samples: u64,
}

impl TrackClock {
    /// `(start offset, scale)`: where the track starts on the meeting's
    /// clock, and meeting time per track time.
    pub fn timing(&self) -> (i64, f64) {
        let written_ms = self.samples as f64 * 1000.0 / RATE as f64;
        let scale = if written_ms > 0.0 {
            (self.last_end_ms - self.first_ms) / written_ms
        } else {
            1.0
        };
        let scale = if (scale - 1.0).abs() <= MAX_DRIFT {
            scale
        } else {
            1.0
        };
        (self.first_ms.round() as i64, scale)
    }

    /// The timing a track saved before Handy stopped, if any.
    pub fn read(dir: &Path, kind: TrackKind) -> Option<Self> {
        let text = std::fs::read_to_string(timing_path(dir, kind)).ok()?;
        serde_json::from_str(&text).ok()
    }
}

impl Capture {
    /// Opens both tracks. Fails only if neither device can be opened; a
    /// track that fails later, or at first, keeps being retried.
    pub fn start(options: CaptureOptions, start: Instant) -> Result<Self> {
        std::fs::create_dir_all(&options.dir)?;
        let levels = Arc::new(Levels::default());
        let (stop_tx, stop_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread_levels = Arc::clone(&levels);
        let thread = thread::Builder::new()
            .name("meeting-capture".into())
            .spawn(move || run(options, start, thread_levels, stop_rx, ready_tx))
            .map_err(|e| anyhow!("Failed to start the meeting recorder: {}", e))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop: Some(stop_tx),
                thread: Some(thread),
                levels,
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(anyhow!("The meeting recorder stopped before it started"))
            }
        }
    }

    pub fn levels(&self) -> &Arc<Levels> {
        &self.levels
    }

    /// Stops both tracks and finishes their files.
    pub fn stop(mut self) -> CaptureReport {
        self.finish()
    }

    fn finish(&mut self) -> CaptureReport {
        drop(self.stop.take());
        self.thread
            .take()
            .and_then(|t| t.join().ok())
            .unwrap_or(CaptureReport {
                tracks: Vec::new(),
                problems: Vec::new(),
            })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.finish();
        }
    }
}

fn run(
    options: CaptureOptions,
    start: Instant,
    levels: Arc<Levels>,
    stop: Receiver<()>,
    ready: Sender<Result<()>>,
) -> CaptureReport {
    let host = crate::audio_toolkit::get_cpal_host();
    let mut problems = Vec::new();
    let mut tracks = Vec::new();
    for (kind, name) in [
        (TrackKind::Mic, options.mic.clone()),
        (TrackKind::Loopback, options.output.clone()),
    ] {
        match Track::new(kind, name, &options.dir, start, Arc::clone(&levels)) {
            Ok(track) => tracks.push(track),
            Err(e) => {
                let _ = ready.send(Err(e));
                return CaptureReport {
                    tracks: Vec::new(),
                    problems,
                };
            }
        }
    }
    for track in &mut tracks {
        track.open(&host, &mut problems);
        if track.streams.is_empty() {
            track.problem(&mut problems, "device_unavailable");
        }
    }
    if tracks.iter().all(|t| t.streams.is_empty()) {
        let _ = ready.send(Err(anyhow!(
            "Couldn't open the microphone or the call audio"
        )));
    } else {
        let _ = ready.send(Ok(()));
        loop {
            match stop.recv_timeout(CHECK_EVERY) {
                Err(RecvTimeoutError::Timeout) => {}
                // A stop, or the handle went away.
                _ => break,
            }
            for track in &mut tracks {
                track.check(&host, &mut problems);
            }
        }
    }
    let tracks = tracks.into_iter().filter_map(Track::close).collect();
    CaptureReport { tracks, problems }
}

/// What the callback shares with the capture thread.
#[derive(Default)]
struct Health {
    /// The stream reported an error it can't recover from.
    failed: AtomicBool,
    /// Milliseconds from the meeting start of the latest packet.
    last_packet_ms: AtomicU64,
}

/// One packet of mono audio at its device's rate.
struct Packet {
    samples: Vec<f32>,
    rate: u32,
    /// Capture time of the first frame, on the stream's clock.
    capture_ns: u128,
    /// Arrival, in milliseconds from the meeting start.
    arrived_ms: u64,
    /// Bumped each time the device is (re)opened.
    epoch: u32,
}

struct Track {
    kind: TrackKind,
    device_name: Option<String>,
    start: Instant,
    levels: Arc<Levels>,
    /// The capture stream, plus the silent stream for loopback.
    streams: Vec<Stream>,
    health: Arc<Health>,
    epoch: u32,
    packets: Option<Sender<Packet>>,
    writer: Option<JoinHandle<Result<WriterReport>>>,
    next_open: Instant,
    /// The device was open before, so the next failure to open is reported.
    was_open: bool,
    final_path: PathBuf,
}

impl Track {
    fn new(
        kind: TrackKind,
        device_name: Option<String>,
        dir: &Path,
        start: Instant,
        levels: Arc<Levels>,
    ) -> Result<Self> {
        let partial = partial_path(dir, kind);
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: WHISPER_SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let wav = hound::WavWriter::create(&partial, spec)
            .map_err(|e| anyhow!("Failed to create {}: {}", partial.display(), e))?;
        let clock_path = timing_path(dir, kind);
        let (tx, rx) = mpsc::channel();
        let writer = thread::Builder::new()
            .name(format!("meeting-{}", kind.name()))
            .spawn(move || Writer::new(wav, Some(clock_path)).run(rx))
            .map_err(|e| anyhow!("Failed to start the {} writer: {}", kind.name(), e))?;
        Ok(Self {
            kind,
            device_name,
            start,
            levels,
            streams: Vec::new(),
            health: Arc::new(Health::default()),
            epoch: 0,
            packets: Some(tx),
            writer: Some(writer),
            next_open: Instant::now(),
            was_open: false,
            final_path: track_path(dir, kind),
        })
    }

    fn now_ms(&self) -> i64 {
        self.start.elapsed().as_millis() as i64
    }

    fn problem(&self, problems: &mut Vec<Problem>, kind: &'static str) {
        problems.push(Problem {
            at_ms: self.now_ms(),
            track: self.kind,
            kind,
        });
    }

    fn open(&mut self, host: &cpal::Host, problems: &mut Vec<Problem>) {
        self.next_open = Instant::now() + RETRY_EVERY;
        let (device, fell_back) = match find_device(host, self.kind, self.device_name.as_deref()) {
            Some(found) => found,
            None => {
                warn!("No device for the meeting's {} track", self.kind.name());
                if std::mem::take(&mut self.was_open) {
                    self.problem(problems, "device_lost");
                }
                return;
            }
        };
        if fell_back {
            self.problem(problems, "device_missing_used_default");
            // Keep following the default from here on.
            self.device_name = None;
        }
        self.epoch += 1;
        self.health = Arc::new(Health::default());
        self.health
            .last_packet_ms
            .store(self.now_ms() as u64, Ordering::Relaxed);
        match self.open_streams(&device) {
            Ok(streams) => {
                info!(
                    "Meeting {} track recording from {}",
                    self.kind.name(),
                    device_label(&device)
                );
                if self.was_open {
                    self.problem(problems, "device_reopened");
                }
                self.streams = streams;
                self.was_open = true;
            }
            Err(e) => {
                error!(
                    "Failed to open {} for the meeting's {} track: {}",
                    device_label(&device),
                    self.kind.name(),
                    e
                );
                if std::mem::take(&mut self.was_open) {
                    self.problem(problems, "device_lost");
                }
            }
        }
    }

    fn open_streams(&self, device: &Device) -> Result<Vec<Stream>> {
        let Some(packets) = self.packets.clone() else {
            return Err(anyhow!("The track is closed"));
        };
        let mut streams = Vec::new();
        // Started first, so loopback has something to follow from its first
        // packet. Silence plays nothing the user can hear.
        if self.kind == TrackKind::Loopback {
            let silent = silent_stream(device, Arc::clone(&self.health))?;
            silent.play()?;
            streams.push(silent);
        }
        // Render endpoints have no input format: loopback uses the mix format.
        let config = match self.kind {
            TrackKind::Mic => device.default_input_config()?,
            TrackKind::Loopback => device.default_output_config()?,
        };
        let context = CallbackContext {
            kind: self.kind,
            start: self.start,
            rate: config.sample_rate(),
            channels: config.channels().max(1) as usize,
            epoch: self.epoch,
            packets,
            health: Arc::clone(&self.health),
            levels: Arc::clone(&self.levels),
        };
        let stream_config = config.config();
        let stream = match config.sample_format() {
            SampleFormat::F32 => capture_stream::<f32>(device, stream_config, context),
            SampleFormat::I16 => capture_stream::<i16>(device, stream_config, context),
            SampleFormat::I32 => capture_stream::<i32>(device, stream_config, context),
            SampleFormat::U16 => capture_stream::<u16>(device, stream_config, context),
            other => Err(anyhow!("Unsupported sample format {}", other)),
        }?;
        stream.play()?;
        streams.push(stream);
        Ok(streams)
    }

    /// Reopens a stream that failed or stalled, and retries a lost device.
    fn check(&mut self, host: &cpal::Host, problems: &mut Vec<Problem>) {
        if !self.streams.is_empty() {
            let silent_for = (self.now_ms() as u64)
                .saturating_sub(self.health.last_packet_ms.load(Ordering::Relaxed));
            let failed = self.health.failed.load(Ordering::Relaxed);
            if !failed && silent_for < STALL_AFTER_MS {
                return;
            }
            warn!(
                "Meeting {} track {}; reopening",
                self.kind.name(),
                if failed { "failed" } else { "stalled" }
            );
            self.streams.clear();
            self.next_open = Instant::now();
        }
        if Instant::now() >= self.next_open {
            self.open(host, problems);
        }
    }

    /// Stops the streams and finishes the file. `None` if nothing usable
    /// was written.
    fn close(mut self) -> Option<TrackReport> {
        self.streams.clear();
        drop(self.packets.take());
        let report = match self.writer.take()?.join() {
            Ok(Ok(report)) => report,
            Ok(Err(e)) => {
                error!("The meeting's {} track failed: {}", self.kind.name(), e);
                return None;
            }
            Err(_) => {
                error!("The meeting's {} track writer panicked", self.kind.name());
                return None;
            }
        };
        let partial = partial_path(
            self.final_path.parent().unwrap_or(Path::new(".")),
            self.kind,
        );
        if report.samples == 0 {
            let _ = std::fs::remove_file(&partial);
            return None;
        }
        if let Err(e) = std::fs::rename(&partial, &self.final_path) {
            error!("Failed to finish {}: {}", self.final_path.display(), e);
            return None;
        }
        Some(TrackReport {
            kind: self.kind,
            path: Some(self.final_path),
            samples: report.samples,
            start_offset_ms: report.start_offset_ms,
            scale: report.scale,
        })
    }
}

fn device_label(device: &Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "an unnamed device".into())
}

/// The named device, else the default (`true` when it had to fall back).
pub(crate) fn find_device(
    host: &cpal::Host,
    kind: TrackKind,
    name: Option<&str>,
) -> Option<(Device, bool)> {
    if let Some(name) = name {
        let devices = match kind {
            TrackKind::Mic => host.input_devices().map(|d| d.collect::<Vec<_>>()),
            TrackKind::Loopback => host.output_devices().map(|d| d.collect::<Vec<_>>()),
        };
        if let Some(device) = devices
            .ok()
            .into_iter()
            .flatten()
            .find(|d| d.description().map(|d| d.name() == name).unwrap_or(false))
        {
            return Some((device, false));
        }
        warn!("'{}' is not connected; using the default device", name);
    }
    let default = match kind {
        TrackKind::Mic => host.default_input_device(),
        TrackKind::Loopback => host.default_output_device(),
    }?;
    Some((default, name.is_some()))
}

struct CallbackContext {
    kind: TrackKind,
    start: Instant,
    rate: u32,
    channels: usize,
    epoch: u32,
    packets: Sender<Packet>,
    health: Arc<Health>,
    levels: Arc<Levels>,
}

fn on_stream_error(kind: TrackKind, health: &Health, err: cpal::Error) {
    match err.kind() {
        // Glitches and priority: the stream carries on.
        ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged => {
            debug!("Meeting {} stream: {}", kind.name(), err);
        }
        _ => {
            warn!("Meeting {} stream error: {}", kind.name(), err);
            health.failed.store(true, Ordering::Relaxed);
        }
    }
}

fn capture_stream<T>(device: &Device, config: StreamConfig, ctx: CallbackContext) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let error_health = Arc::clone(&ctx.health);
    let kind = ctx.kind;
    let stream = device.build_input_stream(
        config,
        move |data: &[T], info: &cpal::InputCallbackInfo| {
            let channels = ctx.channels;
            let samples: Vec<f32> = data
                .chunks_exact(channels)
                .map(|frame| {
                    frame.iter().map(|&s| f32::from_sample_(s)).sum::<f32>() / channels as f32
                })
                .collect();
            let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
            ctx.levels.raise(ctx.kind, peak);
            let arrived_ms = ctx.start.elapsed().as_millis() as u64;
            ctx.health
                .last_packet_ms
                .store(arrived_ms, Ordering::Relaxed);
            // The writer only goes away at the end of the meeting.
            let _ = ctx.packets.send(Packet {
                samples,
                rate: ctx.rate,
                capture_ns: info.timestamp().capture.as_nanos(),
                arrived_ms,
                epoch: ctx.epoch,
            });
        },
        move |err| on_stream_error(kind, &error_health, err),
        None,
    )?;
    Ok(stream)
}

/// Plays silence on `device` so its loopback keeps delivering packets.
fn silent_stream(device: &Device, health: Arc<Health>) -> Result<Stream> {
    let config = device.default_output_config()?;
    let stream_config = config.config();
    match config.sample_format() {
        SampleFormat::F32 => silence::<f32>(device, stream_config, health),
        SampleFormat::I16 => silence::<i16>(device, stream_config, health),
        SampleFormat::I32 => silence::<i32>(device, stream_config, health),
        SampleFormat::U16 => silence::<u16>(device, stream_config, health),
        other => Err(anyhow!("Unsupported sample format {}", other)),
    }
}

fn silence<T: SizedSample>(
    device: &Device,
    config: StreamConfig,
    health: Arc<Health>,
) -> Result<Stream> {
    let stream = device.build_output_stream(
        config,
        |data: &mut [T], _: &cpal::OutputCallbackInfo| data.fill(T::EQUILIBRIUM),
        move |err| on_stream_error(TrackKind::Loopback, &health, err),
        None,
    )?;
    Ok(stream)
}

struct WriterReport {
    samples: u64,
    start_offset_ms: i64,
    scale: f64,
}

/// Turns one track's packets into a continuous 16 kHz file on the meeting's
/// clock.
struct Writer {
    wav: hound::WavWriter<BufWriter<File>>,
    /// Where the timing so far is saved at each flush.
    clock_path: Option<PathBuf>,
    resampler: Option<Resampled>,
    epoch: u32,
    samples: u64,
    since_flush: u64,
    /// Meeting time the track's first sample was captured.
    first_ms: Option<f64>,
    /// Meeting time the latest packet's audio ended.
    last_end_ms: f64,
    last_capture_ns: u128,
    last_packet_ns: u128,
}

impl Writer {
    fn new(wav: hound::WavWriter<BufWriter<File>>, clock_path: Option<PathBuf>) -> Self {
        Self {
            wav,
            clock_path,
            resampler: None,
            epoch: 0,
            samples: 0,
            since_flush: 0,
            first_ms: None,
            last_end_ms: 0.0,
            last_capture_ns: 0,
            last_packet_ns: 0,
        }
    }

    fn run(mut self, packets: Receiver<Packet>) -> Result<WriterReport> {
        for packet in packets {
            self.push(packet)?;
        }
        self.flush_resampler()?;
        let clock = self.clock();
        self.wav.finalize()?;
        if let Some(path) = &self.clock_path {
            let _ = std::fs::remove_file(path);
        }
        let (start_offset_ms, scale) = clock.timing();
        Ok(WriterReport {
            samples: clock.samples,
            start_offset_ms,
            scale,
        })
    }

    fn clock(&self) -> TrackClock {
        TrackClock {
            first_ms: self.first_ms.unwrap_or(0.0),
            last_end_ms: self.last_end_ms,
            samples: self.samples,
        }
    }

    fn push(&mut self, packet: Packet) -> Result<()> {
        let packet_ms = packet.samples.len() as f64 * 1000.0 / packet.rate as f64;
        let began_ms = packet.arrived_ms as f64 - packet_ms;
        let reopened = packet.epoch != self.epoch;
        let rate_changed = self
            .resampler
            .as_ref()
            .is_none_or(|r| r.rate() != packet.rate);
        match self.first_ms {
            None => self.first_ms = Some(began_ms.max(0.0)),
            Some(first_ms) if reopened || rate_changed => {
                // A restarted device: silence for the time it was away.
                self.flush_resampler()?;
                let due = ((began_ms - first_ms) * RATE as f64 / 1000.0).round() as i64;
                self.write_silence(due - self.samples as i64)?;
            }
            Some(_) => {
                // The device dropped audio (loopback with nothing playing):
                // silence for the gap its own clock shows.
                let expected = self.last_capture_ns + self.last_packet_ns;
                if packet.capture_ns > expected + HOLE_THRESHOLD_NS {
                    let hole_ns = packet.capture_ns - expected;
                    self.flush_resampler()?;
                    self.write_silence((hole_ns * RATE as u128 / 1_000_000_000) as i64)?;
                }
            }
        }
        self.epoch = packet.epoch;
        self.last_capture_ns = packet.capture_ns;
        self.last_packet_ns = packet.samples.len() as u128 * 1_000_000_000 / packet.rate as u128;
        self.last_end_ms = packet.arrived_ms as f64;

        if self.resampler.is_none() {
            self.resampler = Some(Resampled::new(packet.rate)?);
        }
        let resampler = self.resampler.as_mut().expect("resampler was just made");
        resampler.push(packet.samples.into_iter())?;
        let out = resampler.take_output();
        self.write(&out)
    }

    fn flush_resampler(&mut self) -> Result<()> {
        if let Some(resampler) = self.resampler.take() {
            let rest = resampler.finish()?;
            self.write(&rest)?;
        }
        Ok(())
    }

    fn write_silence(&mut self, samples: i64) -> Result<()> {
        if samples <= 0 {
            return Ok(());
        }
        debug!(
            "Filling {} ms of a meeting track with silence",
            samples * 1000 / RATE as i64
        );
        let zeros = vec![0.0; 4096];
        let mut left = samples as usize;
        while left > 0 {
            let n = left.min(zeros.len());
            self.write(&zeros[..n])?;
            left -= n;
        }
        Ok(())
    }

    fn write(&mut self, samples: &[f32]) -> Result<()> {
        for &s in samples {
            self.wav
                .write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
        }
        self.samples += samples.len() as u64;
        self.since_flush += samples.len() as u64;
        if self.since_flush >= FLUSH_EVERY_SAMPLES {
            self.wav.flush()?;
            self.since_flush = 0;
            if let Some(path) = &self.clock_path {
                if let Ok(json) = serde_json::to_string(&self.clock()) {
                    let _ = std::fs::write(path, json);
                }
            }
        }
        Ok(())
    }
}

/// Makes a track file that was being written when Handy stopped readable:
/// its header still holds the length at the last flush, so it is set from
/// the file's size. Returns the 16 kHz samples it holds.
pub fn repair_partial(path: &Path) -> Result<u64> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut header = [0u8; 44];
    file.read_exact(&mut header)?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" || &header[36..40] != b"data" {
        return Err(anyhow!("{} is not a track Handy wrote", path.display()));
    }
    let len = file.metadata()?.len();
    // Whole 16-bit samples only.
    let data = (len.saturating_sub(44)) & !1;
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&((36 + data) as u32).to_le_bytes())?;
    file.seek(SeekFrom::Start(40))?;
    file.write_all(&(data as u32).to_le_bytes())?;
    file.set_len(44 + data)?;
    Ok(data / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn writer_in(dir: &Path) -> (Writer, PathBuf) {
        let path = dir.join("track.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: WHISPER_SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        (
            Writer::new(hound::WavWriter::create(&path, spec).unwrap(), None),
            path,
        )
    }

    fn packet(ms: u64, capture_ms: u64, epoch: u32) -> Packet {
        // 10 ms at 48 kHz.
        Packet {
            samples: vec![0.25; 480],
            rate: 48_000,
            capture_ns: capture_ms as u128 * 1_000_000,
            arrived_ms: ms,
            epoch,
        }
    }

    #[test]
    fn a_steady_track_keeps_its_length() {
        let dir = tempfile::tempdir().unwrap();
        let (mut writer, _) = writer_in(dir.path());
        for i in 0..100 {
            writer.push(packet(10 + i * 10, i * 10, 1)).unwrap();
        }
        let report = {
            let (tx, rx) = mpsc::channel();
            drop(tx);
            writer.run(rx).unwrap()
        };
        // One second at 16 kHz, within the resampler's rounding.
        assert!(
            (15_990..=16_010).contains(&report.samples),
            "{}",
            report.samples
        );
        assert_eq!(report.start_offset_ms, 0);
        assert!((report.scale - 1.0).abs() < 0.002, "{}", report.scale);
    }

    #[test]
    fn holes_and_restarts_are_filled_with_silence() {
        let dir = tempfile::tempdir().unwrap();
        let (mut writer, path) = writer_in(dir.path());
        writer.push(packet(10, 0, 1)).unwrap();
        // Loopback skipped half a second.
        writer.push(packet(520, 510, 1)).unwrap();
        // The device came back a second later, on a new clock.
        writer.push(packet(1_530, 99, 2)).unwrap();
        let (tx, rx) = mpsc::channel();
        drop(tx);
        let report = writer.run(rx).unwrap();
        let expected = 1_530 * RATE / 1000;
        assert!(
            report.samples.abs_diff(expected) < 200,
            "{} vs {}",
            report.samples,
            expected
        );
        let samples: Vec<i16> = hound::WavReader::open(&path)
            .unwrap()
            .samples::<i16>()
            .map(|s| s.unwrap())
            .collect();
        assert_eq!(samples.len() as u64, report.samples);
        // Silence in the middle of the hole.
        assert_eq!(samples[(260 * RATE / 1000) as usize], 0);
    }

    #[test]
    fn a_partial_track_is_repaired_from_its_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("me.wav.partial");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: WHISPER_SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut wav = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..1_500 {
            wav.write_sample(1000i16).unwrap();
        }
        wav.finalize().unwrap();
        // As if Handy stopped 500 samples after its last flush: the header
        // still says 1000.
        {
            use std::io::{Seek, SeekFrom, Write};
            let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.seek(SeekFrom::Start(40)).unwrap();
            file.write_all(&2_000u32.to_le_bytes()).unwrap();
        }
        assert_eq!(hound::WavReader::open(&path).unwrap().len(), 1_000);
        assert_eq!(repair_partial(&path).unwrap(), 1_500);
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.len(), 1_500);
    }

    /// Records five seconds from this machine's default mic and output
    /// device (the silent stream plays nothing audible) and checks both
    /// tracks come out the same length. Needs real audio devices:
    /// `cargo test -- --ignored records_both_tracks`.
    #[test]
    #[ignore]
    fn records_both_tracks_from_real_devices() {
        let dir = tempfile::tempdir().unwrap();
        let capture = Capture::start(
            CaptureOptions {
                mic: None,
                output: None,
                dir: dir.path().to_path_buf(),
            },
            Instant::now(),
        )
        .unwrap();
        std::thread::sleep(Duration::from_secs(5));
        let report = capture.stop();
        println!("{:?}", report);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        let lengths: Vec<u64> = report.tracks.iter().map(|t| t.samples).collect();
        assert_eq!(lengths.len(), 2, "both tracks recorded");
        for track in &report.tracks {
            let path = track.path.as_ref().unwrap();
            assert_eq!(
                hound::WavReader::open(path).unwrap().len() as u64,
                track.samples
            );
            // About five seconds at 16 kHz.
            assert!((70_000..90_000).contains(&track.samples), "{:?}", track);
        }
        assert!(lengths[0].abs_diff(lengths[1]) < 1_600, "{:?}", lengths);
        assert!(!partial_path(dir.path(), TrackKind::Mic).exists());
    }
}
