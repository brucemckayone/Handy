//! Notices when a call app starts using the mic, so Handy can offer to
//! record (it never records by itself), and when the app lets go of the mic,
//! so a meeting recorded for that call can stop after a countdown.
//!
//! Windows keeps, per app, when it last started and stopped using the mic
//! (the privacy settings' "recent activity"); an app is using the mic while
//! its stop time is zero. Reading it needs no admin rights.

use log::{debug, error, info};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashSet;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};
use tauri::AppHandle;

use super::widgets::{self, PromptState};
use crate::settings::{get_settings, write_settings};

const POLL: Duration = Duration::from_secs(2);
/// How long a meeting keeps recording after its call ends, in case it didn't.
pub const STOP_COUNTDOWN: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CallApp {
    Teams,
    Zoom,
    Browser,
}

/// Which call app, if any, a microphone consent entry belongs to.
fn classify(entry: &str) -> Option<CallApp> {
    // Packaged apps are keyed by package family name.
    if entry.starts_with("MSTeams_") {
        return Some(CallApp::Teams);
    }
    // Desktop apps by their path, with `#` for `\`.
    let exe = entry.rsplit(['#', '\\']).next()?.to_ascii_lowercase();
    match exe.as_str() {
        "ms-teams.exe" | "teams.exe" => Some(CallApp::Teams),
        "zoom.exe" => Some(CallApp::Zoom),
        "chrome.exe" | "msedge.exe" | "firefox.exe" | "brave.exe" | "opera.exe" | "vivaldi.exe" => {
            Some(CallApp::Browser)
        }
        _ => None,
    }
}

/// Call apps using the mic right now.
#[cfg(windows)]
fn apps_using_mic() -> HashSet<CallApp> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    const BASE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";
    let mut apps = HashSet::new();
    let Ok(base) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(BASE) else {
        return apps;
    };
    let mut check = |parent: &RegKey| {
        for name in parent.enum_keys().flatten() {
            let Some(app) = classify(&name) else {
                continue;
            };
            let Ok(key) = parent.open_subkey(&name) else {
                continue;
            };
            let start: u64 = key.get_value("LastUsedTimeStart").unwrap_or(0);
            let stop: u64 = key.get_value("LastUsedTimeStop").unwrap_or(1);
            if start != 0 && stop == 0 {
                apps.insert(app);
            }
        }
    };
    check(&base);
    if let Ok(desktop) = base.open_subkey("NonPackaged") {
        check(&desktop);
    }
    apps
}

#[cfg(not(windows))]
fn apps_using_mic() -> HashSet<CallApp> {
    HashSet::new()
}

#[derive(Default)]
struct State {
    /// The app the start prompt is showing for.
    prompted: Option<CallApp>,
    /// Apps the user said "Not now" to, until their call ends.
    dismissed: HashSet<CallApp>,
    /// The call the meeting being recorded belongs to.
    meeting_app: Option<CallApp>,
    /// When the meeting stops, its call having ended.
    stop_at: Option<Instant>,
    /// "Keep recording": wait for the call to use the mic again before
    /// counting down again.
    kept: bool,
    /// Apps using the mic at the last poll.
    in_use: HashSet<CallApp>,
}

static STATE: Lazy<Mutex<State>> = Lazy::new(|| Mutex::new(State::default()));

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

fn enabled(app: CallApp, settings: &crate::settings::MeetingSettings) -> bool {
    match app {
        CallApp::Teams => settings.detect_teams,
        CallApp::Zoom => settings.detect_zoom,
        CallApp::Browser => settings.detect_browser,
    }
}

/// Starts watching for calls.
pub fn start(app: &AppHandle) {
    if !cfg!(windows) {
        return;
    }
    let app = app.clone();
    let spawned = thread::Builder::new()
        .name("meeting-call-detect".into())
        .spawn(move || loop {
            thread::sleep(POLL);
            poll(&app);
        });
    if let Err(e) = spawned {
        error!("Failed to start call detection: {}", e);
    }
}

fn poll(app: &AppHandle) {
    let Some(manager) = super::manager(app) else {
        return;
    };
    let meeting = get_settings(app).meeting;
    let in_use = apps_using_mic();
    let recording = manager.is_recording();
    let mut show: Option<PromptState> = None;
    let mut hide = false;
    let mut stop = false;
    {
        let mut s = state();
        if in_use != s.in_use {
            debug!("Call apps using the mic: {:?}", in_use);
        }
        s.in_use = in_use.clone();
        s.dismissed.retain(|a| in_use.contains(a));
        if !recording {
            s.stop_at = None;
            s.meeting_app = None;
            if let Some(prompted) = s.prompted {
                if !in_use.contains(&prompted) || !enabled(prompted, &meeting) {
                    s.prompted = None;
                    hide = true;
                }
            }
            if s.prompted.is_none() && meeting.ask_when_call_starts {
                let offer = [CallApp::Teams, CallApp::Zoom, CallApp::Browser]
                    .into_iter()
                    .find(|a| {
                        in_use.contains(a) && enabled(*a, &meeting) && !s.dismissed.contains(a)
                    });
                if let Some(offer) = offer {
                    info!("{:?} started using the mic; offering to record", offer);
                    s.prompted = Some(offer);
                    hide = false;
                    show = Some(PromptState::AskStart { app: offer });
                }
            }
        } else if let Some(call) = s.meeting_app {
            let ended = !in_use.contains(&call);
            if !ended {
                s.kept = false;
                if s.stop_at.take().is_some() {
                    hide = true;
                }
            } else if meeting.stop_when_call_ends && !s.kept {
                match s.stop_at {
                    None => {
                        info!("{:?} stopped using the mic; counting down to stop", call);
                        s.stop_at = Some(Instant::now() + STOP_COUNTDOWN);
                        show = Some(PromptState::AskEnd {
                            app: call,
                            seconds: STOP_COUNTDOWN.as_secs() as u32,
                        });
                    }
                    Some(at) if Instant::now() >= at => {
                        s.stop_at = None;
                        stop = true;
                    }
                    Some(_) => {}
                }
            }
        }
    }
    if hide {
        widgets::hide_prompt(app);
    }
    if let Some(state) = show {
        widgets::show_prompt(app, state);
    }
    if stop {
        info!("The call ended; stopping the meeting");
        stop_meeting(app);
    }
}

/// Stops the meeting and says so in the prompt.
fn stop_meeting(app: &AppHandle) {
    let Some(manager) = super::manager(app) else {
        return;
    };
    let started = manager.elapsed_ms();
    match manager.stop() {
        Ok(_) => widgets::show_prompt(
            app,
            PromptState::Saved {
                minutes: (started.unwrap_or(0) / 60_000) as u32,
            },
        ),
        Err(e) => error!("Failed to stop the meeting: {}", e),
    }
}

/// A meeting started: tie it to the call using the mic, if any.
pub fn meeting_started(_app: &AppHandle) {
    let mut s = state();
    s.meeting_app = s.prompted.take().or_else(|| {
        [CallApp::Teams, CallApp::Zoom, CallApp::Browser]
            .into_iter()
            .find(|a| s.in_use.contains(a))
    });
    s.stop_at = None;
    s.kept = false;
}

pub fn meeting_stopped(app: &AppHandle) {
    let had_countdown = {
        let mut s = state();
        // Stopped while the call goes on: don't offer to record it again.
        if let Some(call) = s.meeting_app.take() {
            s.dismissed.insert(call);
        }
        s.kept = false;
        s.stop_at.take().is_some()
    };
    if had_countdown {
        widgets::hide_prompt(app);
    }
}

/// What the user chose in the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PromptChoice {
    Record,
    NotNow,
    /// Stop asking for this app (it can be turned back on in settings).
    Never,
    /// Undo "Never".
    Undo,
    KeepRecording,
    StopNow,
    Close,
}

pub fn respond(app: &AppHandle, choice: PromptChoice, call: Option<CallApp>) {
    match choice {
        PromptChoice::Record => {
            let Some(manager) = super::manager(app) else {
                return;
            };
            match manager.start() {
                Ok(_) => widgets::show_prompt(app, PromptState::Recording),
                Err(e) => {
                    error!("Failed to start the meeting: {}", e);
                    widgets::hide_prompt(app);
                }
            }
        }
        PromptChoice::NotNow | PromptChoice::Close => {
            let mut s = state();
            if let Some(prompted) = s.prompted.take() {
                s.dismissed.insert(prompted);
            }
            drop(s);
            widgets::hide_prompt(app);
        }
        PromptChoice::Never | PromptChoice::Undo => {
            let Some(call) = call.or_else(|| state().prompted) else {
                return;
            };
            let on = choice == PromptChoice::Undo;
            let mut settings = get_settings(app);
            match call {
                CallApp::Teams => settings.meeting.detect_teams = on,
                CallApp::Zoom => settings.meeting.detect_zoom = on,
                CallApp::Browser => settings.meeting.detect_browser = on,
            }
            write_settings(app, settings);
            if on {
                widgets::show_prompt(app, PromptState::AskStart { app: call });
            } else {
                state().prompted = None;
                widgets::show_prompt(app, PromptState::Never { app: call });
            }
        }
        PromptChoice::KeepRecording => {
            {
                let mut s = state();
                s.stop_at = None;
                s.kept = true;
            }
            widgets::show_prompt(app, PromptState::Kept);
        }
        PromptChoice::StopNow => {
            state().stop_at = None;
            stop_meeting(app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_apps_are_recognised_from_their_consent_entries() {
        assert_eq!(classify("MSTeams_8wekyb3d8bbwe"), Some(CallApp::Teams));
        assert_eq!(
            classify(r"C:#Program Files#Zoom#bin#Zoom.exe"),
            Some(CallApp::Zoom)
        );
        assert_eq!(
            classify(r"C:#Program Files (x86)#Microsoft#Edge#Application#msedge.exe"),
            Some(CallApp::Browser)
        );
        assert_eq!(
            classify(r"C:#Users#me#dev#Handy#target#debug#handy.exe"),
            None
        );
        assert_eq!(
            classify("Microsoft.WindowsSoundRecorder_8wekyb3d8bbwe"),
            None
        );
    }
}
