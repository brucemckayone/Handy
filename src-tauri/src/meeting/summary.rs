//! A meeting's summary, decisions, action items and a suggested title.
//!
//! Written by headless Claude Code (`claude -p`, on the user's own account)
//! or by the Custom post-processing provider when it runs on this computer
//! (Ollama by default). Only the transcript text is sent; audio never leaves
//! the computer, and nothing is sent until a transcript exists.

use anyhow::{anyhow, Result};
use log::{info, warn};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashSet;
use std::io::Write;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::AppHandle;

use super::store::{ActionItem, BookmarkNote, Decision, Meeting, MeetingStatus, MeetingSummary};
use super::MeetingManager;
use crate::claude_cli;
use crate::settings::{get_settings, AppSettings, SummaryEngine};

/// Long meetings take a while; past this the summary is given up.
const TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// The Custom provider's model when none is set: small enough for a laptop.
const DEFAULT_LOCAL_MODEL: &str = "llama3.2:3b";
/// A small local model's context holds a few thousand tokens (Ollama's
/// default), so a long transcript is summarised this many words at a time,
/// then from those notes.
const LOCAL_PART_WORDS: usize = 1_200;

/// Meetings being summarised, so one isn't summarised twice at once.
static RUNNING: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

/// Summarises meeting `id` in the background, with `engine` or the one in
/// settings.
pub fn spawn(app: AppHandle, id: String, engine: Option<SummaryEngine>) {
    let Some(manager) = super::manager(&app) else {
        return;
    };
    if !RUNNING
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(id.clone())
    {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("meeting-summary".into())
        .spawn(move || {
            let settings = get_settings(&app);
            let engine = engine.unwrap_or(settings.meeting.summary_engine);
            // Only a transcribed meeting is summarised, and its status is
            // left alone otherwise.
            let mut ready = false;
            let _ = manager.update(&id, |m| {
                ready = !m.segments.is_empty()
                    && matches!(m.status, MeetingStatus::Ready | MeetingStatus::Summarising);
                if ready {
                    m.status = MeetingStatus::Summarising;
                    m.summary_error = None;
                }
            });
            if !ready {
                RUNNING
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&id);
                return;
            }
            let result = summarise(&manager, &id, engine, &settings);
            let _ = manager.update(&id, |m| {
                if m.status == MeetingStatus::Summarising {
                    m.status = MeetingStatus::Ready;
                }
                match result {
                    Ok((summary, title)) => {
                        if m.title_is_suggested && !title.is_empty() {
                            m.title = title;
                        }
                        m.summary = Some(summary);
                        m.summary_error = None;
                    }
                    Err(e) => {
                        warn!("Summary of meeting {} failed: {}", m.id, e);
                        m.summary_error = Some(e.to_string());
                    }
                }
            });
            RUNNING
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
        });
    if let Err(e) = spawned {
        warn!("Failed to start the meeting summary: {}", e);
    }
}

fn summarise(
    manager: &Arc<MeetingManager>,
    id: &str,
    engine: SummaryEngine,
    settings: &AppSettings,
) -> Result<(MeetingSummary, String)> {
    let meeting = manager
        .store()
        .get(id)?
        .ok_or_else(|| anyhow!("The meeting no longer exists"))?;
    if meeting.segments.is_empty() {
        return Err(anyhow!("The meeting has no transcript to summarise"));
    }
    let instructions = &settings.meeting.summary_prompt;
    let started = Instant::now();
    let reply = match engine {
        SummaryEngine::ClaudeCode => run_claude_code(&build_prompt(
            &meeting,
            instructions,
            "Transcript",
            &transcript_text(&meeting),
        ))?,
        SummaryEngine::Local => {
            tauri::async_runtime::block_on(summarise_locally(&meeting, instructions, settings))?
        }
    };
    info!(
        "Summarised meeting {} with {:?} in {:.0}s",
        id,
        engine,
        started.elapsed().as_secs_f64()
    );
    parse_reply(&reply, &meeting, engine)
}

/// `m:ss`, or `h:mm:ss` past an hour.
pub fn format_ms(ms: i64) -> String {
    let total = ms.max(0) / 1000;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn parse_time(text: &str) -> Option<i64> {
    let parts: Vec<i64> = text
        .trim()
        .split(':')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some(secs * 1000)
}

/// What a speaker is called in the prompt.
fn speaker_name(meeting: &Meeting, id: &str) -> String {
    if let Some(name) = meeting
        .speakers
        .iter()
        .find(|s| s.id == id)
        .and_then(|s| s.name.clone())
        .filter(|n| !n.trim().is_empty())
    {
        return name;
    }
    match id {
        "me" => "Me".to_string(),
        "them" => "Other people".to_string(),
        other => other
            .strip_prefix('s')
            .map(|n| format!("Speaker {n}"))
            .unwrap_or_else(|| other.to_string()),
    }
}

/// The transcript as `[m:ss] Name: text` lines, one per turn.
fn transcript_text(meeting: &Meeting) -> String {
    let mut out = String::new();
    let mut current: Option<&str> = None;
    for segment in &meeting.segments {
        if current == Some(segment.speaker.as_str()) {
            out.push(' ');
        } else {
            if current.is_some() {
                out.push('\n');
            }
            out.push_str(&format!(
                "[{}] {}: ",
                format_ms(segment.start_ms),
                speaker_name(meeting, &segment.speaker)
            ));
            current = Some(&segment.speaker);
        }
        out.push_str(segment.text.trim());
    }
    out
}

/// The summary request: `body`, under `heading`, is the transcript or notes
/// made from it.
fn build_prompt(meeting: &Meeting, instructions: &str, heading: &str, body: &str) -> String {
    let speakers: Vec<String> = meeting
        .speakers
        .iter()
        .map(|s| speaker_name(meeting, &s.id))
        .collect();
    let bookmarks: Vec<String> = meeting
        .bookmarks
        .iter()
        .map(|b| format!("{}-{}", format_ms(b.start_ms), format_ms(b.end_ms)))
        .collect();
    format!(
        "{instructions}\n\n\
         Reply with only a JSON object and no other text, in this shape:\n\
         {{\"title\": \"a short title for the meeting, at most 8 words\", \
         \"overview\": \"the summary\", \
         \"decisions\": [{{\"text\": \"the decision\", \"at\": \"m:ss\"}}], \
         \"action_items\": [{{\"text\": \"what is to be done\", \"owner\": \"a speaker's name, or null\", \
         \"due\": \"when it is due, as said, or null\", \"at\": \"m:ss\"}}], \
         \"bookmarks\": [{{\"at\": \"m:ss\", \"label\": \"what that moment was about, in a few words\"}}]}}\n\
         \"at\" is the transcript time it was said. Use empty lists when there is nothing to list.\n\
         The speakers are: {}. \"Me\" is the person who recorded the meeting.\n\
         The moments they bookmarked are: {}.\n\n\
         {heading}:\n{body}",
        speakers.join(", "),
        if bookmarks.is_empty() {
            "none".to_string()
        } else {
            bookmarks.join(", ")
        },
    )
}

/// The transcript in parts of about `words` words, split between turns.
fn transcript_parts(transcript: &str, words: usize) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    for line in transcript.lines() {
        let line_words = line.split_whitespace().count();
        if count > 0 && count + line_words > words {
            parts.push(std::mem::take(&mut current));
            count = 0;
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
        count += line_words;
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Summarises on this computer: a short meeting in one go; a long one part
/// by part into notes, then the summary from the notes.
async fn summarise_locally(
    meeting: &Meeting,
    instructions: &str,
    settings: &AppSettings,
) -> Result<String> {
    let transcript = transcript_text(meeting);
    let parts = transcript_parts(&transcript, LOCAL_PART_WORDS);
    if parts.len() <= 1 {
        let prompt = build_prompt(meeting, instructions, "Transcript", &transcript);
        return run_local(&prompt, settings).await;
    }
    let mut notes = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let prompt = format!(
            "This is part {} of {} of a meeting transcript. Write short notes on it as bullet \
             points, each starting with its [m:ss] time from the transcript: what was \
             discussed, any decisions, and any action items with who owns them and when \
             they are due. Only use what was said; never invent names or dates.\n\n{}",
            i + 1,
            parts.len(),
            part
        );
        let note = run_local(&prompt, settings).await?;
        notes.push(format!("Part {}:\n{}", i + 1, note.trim()));
    }
    let prompt = build_prompt(
        meeting,
        instructions,
        "Notes on the meeting, part by part, with transcript times",
        &notes.join("\n\n"),
    );
    run_local(&prompt, settings).await
}

fn run_claude_code(prompt: &str) -> Result<String> {
    let claude = claude_cli::find_claude().ok_or_else(|| anyhow!("claude_not_found"))?;
    // An empty folder, so Claude Code reads no project files or settings.
    let workdir = std::env::temp_dir().join("handy-meeting-summary");
    std::fs::create_dir_all(&workdir)?;
    let mut child = claude_cli::command(&claude)
        .args(["-p", "--output-format", "json", "--max-turns", "1"])
        .current_dir(&workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("No stdin"))?;
    let input = prompt.to_string();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let mut stdout = child.stdout.take().ok_or_else(|| anyhow!("No stdout"))?;
    let mut stderr = child.stderr.take().ok_or_else(|| anyhow!("No stderr"))?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        std::io::Read::read_to_string(&mut stdout, &mut out).map(|_| out)
    });
    let err_reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = std::io::Read::read_to_string(&mut stderr, &mut out);
        out
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > TIMEOUT {
            claude_cli::kill_tree(&mut child);
            return Err(anyhow!("Claude Code took too long"));
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    let _ = writer.join();
    let out = reader
        .join()
        .map_err(|_| anyhow!("Claude Code's output was lost"))??;
    let err = err_reader.join().unwrap_or_default();
    if !status.success() && out.trim().is_empty() {
        return Err(anyhow!(
            "Claude Code failed: {}",
            err.lines().last().unwrap_or("no output")
        ));
    }
    #[derive(Deserialize)]
    struct ClaudeResult {
        result: Option<String>,
        #[serde(default)]
        is_error: bool,
    }
    let parsed: ClaudeResult = serde_json::from_str(out.trim())
        .map_err(|e| anyhow!("Unexpected output from Claude Code: {}", e))?;
    match parsed.result {
        Some(result) if !parsed.is_error => Ok(result),
        Some(result) => Err(anyhow!("Claude Code failed: {}", result)),
        None => Err(anyhow!("Claude Code gave no answer")),
    }
}

/// Whether `base_url` points at this computer.
fn is_local_url(base_url: &str) -> bool {
    let rest = base_url
        .trim()
        .strip_prefix("http://")
        .or_else(|| base_url.trim().strip_prefix("https://"))
        .unwrap_or("");
    let host = rest.split(['/', '?']).next().unwrap_or("");
    let host = if let Some(bracketed) = host.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or("")
    } else {
        host.split(':').next().unwrap_or("")
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

async fn run_local(prompt: &str, settings: &AppSettings) -> Result<String> {
    let provider = settings
        .post_process_provider("custom")
        .cloned()
        .ok_or_else(|| anyhow!("local_not_configured"))?;
    // "On this computer" has to mean it: never a remote endpoint.
    if !is_local_url(&provider.base_url) {
        return Err(anyhow!("local_not_local"));
    }
    let model = settings
        .post_process_models
        .get("custom")
        .filter(|m| !m.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| DEFAULT_LOCAL_MODEL.to_string());
    let api_key = settings
        .post_process_api_keys
        .get("custom")
        .cloned()
        .unwrap_or_default();
    let request = crate::llm_client::send_chat_completion_with_schema(
        &provider,
        api_key,
        &model,
        prompt.to_string(),
        None,
        None,
        true,
    );
    match tokio::time::timeout(TIMEOUT, request).await {
        Ok(Ok(Some(reply))) => Ok(reply),
        Ok(Ok(None)) => Err(anyhow!("The local model gave no answer")),
        Ok(Err(e)) => Err(anyhow!("The local model failed: {}", e)),
        Err(_) => Err(anyhow!("The local model took too long")),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Reply {
    title: String,
    overview: String,
    decisions: Vec<ReplyDecision>,
    action_items: Vec<ReplyAction>,
    bookmarks: Vec<ReplyBookmark>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReplyDecision {
    text: String,
    at: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReplyAction {
    text: String,
    owner: Option<String>,
    due: Option<String>,
    at: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReplyBookmark {
    at: Option<String>,
    label: String,
}

/// The summary in the model's reply, and the title it suggests.
fn parse_reply(
    reply: &str,
    meeting: &Meeting,
    engine: SummaryEngine,
) -> Result<(MeetingSummary, String)> {
    let start = reply.find('{');
    let end = reply.rfind('}');
    let json = match (start, end) {
        (Some(s), Some(e)) if e > s => &reply[s..=e],
        _ => return Err(anyhow!("The summary wasn't in the expected form")),
    };
    let parsed: Reply = serde_json::from_str(json)
        .map_err(|e| anyhow!("The summary wasn't in the expected form: {}", e))?;
    let at = |t: &Option<String>| t.as_deref().and_then(parse_time);
    // An owner named like a speaker becomes that speaker, so renaming them
    // later renames the owner too.
    let owner_id = |owner: Option<String>| {
        let owner = owner.filter(|o| !o.trim().is_empty() && o.trim() != "null")?;
        let found = meeting
            .speakers
            .iter()
            .find(|s| speaker_name(meeting, &s.id).eq_ignore_ascii_case(owner.trim()))
            .map(|s| s.id.clone());
        Some(found.unwrap_or(owner))
    };
    let summary = MeetingSummary {
        overview: parsed.overview.trim().to_string(),
        decisions: parsed
            .decisions
            .into_iter()
            .filter(|d| !d.text.trim().is_empty())
            .map(|d| Decision {
                at_ms: at(&d.at),
                text: d.text.trim().to_string(),
            })
            .collect(),
        action_items: parsed
            .action_items
            .into_iter()
            .filter(|a| !a.text.trim().is_empty())
            .map(|a| ActionItem {
                at_ms: at(&a.at),
                text: a.text.trim().to_string(),
                owner: owner_id(a.owner),
                due: a.due.filter(|d| !d.trim().is_empty() && d.trim() != "null"),
                done: false,
            })
            .collect(),
        bookmarks: parsed
            .bookmarks
            .into_iter()
            .filter_map(|b| {
                Some(BookmarkNote {
                    start_ms: at(&b.at)?,
                    label: b.label.trim().to_string(),
                })
            })
            .collect(),
        engine,
        created_at: chrono::Utc::now().timestamp_millis(),
    };
    Ok((summary, parsed.title.trim().trim_matches('"').to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::store::{Speaker, TranscriptSegment};

    fn meeting() -> Meeting {
        let mut m = Meeting::new("m".into(), String::new(), 0);
        for (start, speaker, text) in [
            (0, "s1", "Shall we start?"),
            (2_000, "s1", "Pricing first."),
            (65_000, "me", "I'll draft it by Friday."),
        ] {
            m.segments.push(TranscriptSegment {
                start_ms: start,
                end_ms: start + 1_500,
                speaker: speaker.into(),
                text: text.into(),
            });
        }
        m.speakers = vec![
            Speaker {
                id: "me".into(),
                name: None,
            },
            Speaker {
                id: "s1".into(),
                name: Some("Priya".into()),
            },
        ];
        m
    }

    #[test]
    fn the_transcript_reads_as_turns() {
        assert_eq!(
            transcript_text(&meeting()),
            "[0:00] Priya: Shall we start? Pricing first.\n[1:05] Me: I'll draft it by Friday."
        );
        assert_eq!(format_ms(3_725_000), "1:02:05");
    }

    #[test]
    fn a_long_transcript_is_split_between_turns() {
        let transcript =
            "[0:00] A: one two three\n[0:05] B: four five\n[0:09] A: six seven eight nine";
        assert_eq!(
            transcript_parts(transcript, 5),
            [
                "[0:00] A: one two three",
                "[0:05] B: four five",
                "[0:09] A: six seven eight nine"
            ]
        );
        assert_eq!(transcript_parts(transcript, 100).len(), 1);
    }

    #[test]
    fn a_reply_becomes_a_summary() {
        let reply = "Here you go:\n```json\n{\"title\": \"Pilot pricing\", \"overview\": \"They talked pricing.\", \
            \"decisions\": [{\"text\": \"Per-seat first\", \"at\": \"0:02\"}], \
            \"action_items\": [{\"text\": \"Draft pricing\", \"owner\": \"me\", \"due\": \"Friday\", \"at\": \"1:05\"}, \
            {\"text\": \"Book a room\", \"owner\": \"Sam\", \"due\": null, \"at\": \"nonsense\"}], \
            \"bookmarks\": []}\n```";
        let (summary, title) = parse_reply(reply, &meeting(), SummaryEngine::ClaudeCode).unwrap();
        assert_eq!(title, "Pilot pricing");
        assert_eq!(summary.decisions[0].at_ms, Some(2_000));
        assert_eq!(summary.action_items[0].owner.as_deref(), Some("me"));
        assert_eq!(summary.action_items[0].at_ms, Some(65_000));
        assert_eq!(summary.action_items[1].owner.as_deref(), Some("Sam"));
        assert_eq!(summary.action_items[1].due, None);
        assert_eq!(summary.action_items[1].at_ms, None);
        assert!(parse_reply("no json here", &meeting(), SummaryEngine::Local).is_err());
    }

    /// Summarises a made-up three-line meeting with the installed Claude
    /// Code: `cargo test -- --ignored claude_code_summarises`.
    #[test]
    #[ignore]
    fn claude_code_summarises_a_made_up_meeting() {
        let m = meeting();
        let prompt = build_prompt(
            &m,
            crate::settings::DEFAULT_MEETING_SUMMARY_PROMPT,
            "Transcript",
            &transcript_text(&m),
        );
        let reply = run_claude_code(&prompt).unwrap();
        println!("{reply}");
        let (summary, title) = parse_reply(&reply, &m, SummaryEngine::ClaudeCode).unwrap();
        println!("{title}: {summary:?}");
        assert!(!summary.overview.is_empty());
        assert!(summary
            .action_items
            .iter()
            .any(|a| a.owner.as_deref() == Some("me")));
    }

    #[test]
    fn only_this_computer_counts_as_local() {
        assert!(is_local_url("http://localhost:11434/v1"));
        assert!(is_local_url("http://127.0.0.1:8080"));
        assert!(is_local_url("http://[::1]:11434/v1"));
        assert!(!is_local_url("https://api.example.com/v1"));
        assert!(!is_local_url("http://localhost.example.com/v1"));
        assert!(!is_local_url("ftp://localhost"));
    }
}
