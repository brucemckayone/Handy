use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone, Default)]
#[command(name = "handy", about = "Handy - Speech to Text")]
pub struct CliArgs {
    /// Start with the main window hidden
    #[arg(long)]
    pub start_hidden: bool,

    /// Disable the system tray icon
    #[arg(long)]
    pub no_tray: bool,

    /// Toggle transcription on/off (sent to running instance)
    #[arg(long)]
    pub toggle_transcription: bool,

    /// Toggle transcription with post-processing on/off (sent to running instance)
    #[arg(long)]
    pub toggle_post_process: bool,

    /// Cancel the current operation (sent to running instance)
    #[arg(long)]
    pub cancel: bool,

    /// Read this text aloud (sent to running instance; queued after anything
    /// already being read)
    #[arg(long, value_name = "TEXT")]
    pub speak: Option<String>,

    /// Read this file aloud, e.g. markdown (sent to running instance)
    #[arg(long, value_name = "FILE")]
    pub speak_file: Option<PathBuf>,

    /// Read what is piped in aloud (sent to running instance)
    #[arg(long)]
    pub speak_stdin: bool,

    /// For Claude Code's Stop and Notification hooks: reads the hook input on
    /// stdin and reads Claude's last reply, or the notice, aloud (sent to
    /// running instance)
    #[arg(long)]
    pub speak_claude_hook: bool,

    /// Stop reading aloud and clear what is queued (sent to running instance)
    #[arg(long)]
    pub stop_speaking: bool,

    /// Ask Claude Code this question; the answer is spoken and shown in the
    /// Ask panel (sent to running instance)
    #[arg(long, value_name = "QUESTION")]
    pub ask: Option<String>,

    /// Ask what is piped in, e.g. a diff with a question (sent to running
    /// instance)
    #[arg(long)]
    pub ask_stdin: bool,

    /// Enable debug mode with verbose logging
    #[arg(long)]
    pub debug: bool,

    /// Transcribe this WAV (16 kHz mono) headlessly and exit. Runs the same
    /// batch transcription path as the app — no mic, no silence removal, no
    /// download (the model must already be installed). Long audio is still
    /// chunked at pauses, as in the app.
    #[arg(short = 'f', long, value_name = "WAV")]
    pub transcribe_file: Option<PathBuf>,

    /// Model id to load for --transcribe-file (default: the selected model).
    #[arg(long)]
    pub model: Option<String>,

    /// Hard-select the compute device for --transcribe-file by its registry
    /// index (see --list-devices). Omit to use the persisted accelerator
    /// setting. transcribe-cpp (whisper-family) models only.
    #[arg(long, value_name = "N")]
    pub device_index: Option<usize>,

    /// List the transcribe-cpp compute devices (with indices) and exit.
    #[arg(long)]
    pub list_devices: bool,

    /// List the available models (with ids) and exit. Pass an id to --model.
    /// Honors --json for machine-readable output.
    #[arg(long)]
    pub list_models: bool,

    /// Repeat the transcription N times (best_ms reports the fastest run).
    #[arg(long, value_name = "N")]
    pub repeat: Option<usize>,

    /// Emit --transcribe-file results as JSON.
    #[arg(long)]
    pub json: bool,
}
