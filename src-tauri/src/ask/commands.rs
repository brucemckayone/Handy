//! Commands for the Ask panel and the Ask settings page.

use tauri::{AppHandle, Manager};

use super::store::{Thread, ThreadListItem, Via};
use super::{manager, NewQuestion};
use super::{panel, AskBusyEvent};
use crate::settings::{get_settings, write_settings, AskSettings};

fn ask_manager(app: &AppHandle) -> Result<std::sync::Arc<super::AskManager>, String> {
    manager(app).ok_or_else(|| "Ask isn't set up".to_string())
}

/// Opens the panel on a thread (`None`: the latest), ready for typing.
#[tauri::command]
#[specta::specta]
pub fn open_ask_panel(app: AppHandle, thread_id: Option<String>) {
    crate::speech::pill::ask_dismissed(&app);
    panel::open(&app, thread_id, true);
}

#[tauri::command]
#[specta::specta]
pub fn close_ask_panel(app: AppHandle) {
    panel::close(&app);
}

/// The thread the panel was last opened on, for a page that loads late.
#[tauri::command]
#[specta::specta]
pub fn get_ask_panel_thread() -> Option<String> {
    panel::opened_thread()
}

/// The panel switched thread: questions asked with it in front go there.
#[tauri::command]
#[specta::specta]
pub fn set_ask_panel_thread(thread_id: Option<String>) {
    panel::set_opened_thread(thread_id);
}

/// What Ask shows in the speaking pill, for a pill page that loads late.
#[tauri::command]
#[specta::specta]
pub fn get_ask_pill_state() -> AskBusyEvent {
    crate::speech::pill::ask_state()
}

/// The pill's Send: the question being recorded is done.
#[tauri::command]
#[specta::specta]
pub fn send_ask(app: AppHandle) {
    if let Some(coordinator) = app.try_state::<crate::TranscriptionCoordinator>() {
        coordinator.finish("ask", "pill");
    }
}

/// What the grab overlay shows, for a page that loads late.
#[tauri::command]
#[specta::specta]
pub fn get_ask_grab() -> Option<super::grab::GrabFrame> {
    super::grab::frame()
}

/// The grab overlay is done: its marked-up picture joins the question.
#[tauri::command]
#[specta::specta]
pub fn finish_ask_grab(app: AppHandle, result: super::grab::GrabResult) -> Result<(), String> {
    super::grab::finish(&app, result)
}

/// The grab was dropped: back to talking.
#[tauri::command]
#[specta::specta]
pub fn close_ask_grab(app: AppHandle) {
    super::grab::close(&app);
}

/// A thumbnail on the pill was clicked: its picture opens to mark up again.
#[tauri::command]
#[specta::specta]
pub fn open_ask_capture(app: AppHandle, n: u32) {
    std::thread::spawn(move || super::grab::open_capture(&app, n));
}

/// The panel's grab button: a grab for the question being typed.
#[tauri::command]
#[specta::specta]
pub fn grab_for_ask_draft(app: AppHandle) {
    std::thread::spawn(move || super::grab::open_for_draft(&app));
}

/// The grabs waiting with the panel's draft.
#[tauri::command]
#[specta::specta]
pub fn get_ask_draft() -> Vec<super::capture::CaptureView> {
    super::draft::captures()
}

/// A grab is taken off the panel's draft.
#[tauri::command]
#[specta::specta]
pub fn remove_ask_draft_grab(app: AppHandle, n: u32) {
    super::draft::remove(&app, n);
}

/// A picture is taken out of the question being asked.
#[tauri::command]
#[specta::specta]
pub fn remove_ask_capture(app: AppHandle, n: u32) {
    super::grab::take_out(&app, n);
}

/// Recent threads, newest first.
#[tauri::command]
#[specta::specta]
pub fn list_ask_threads(app: AppHandle) -> Result<Vec<ThreadListItem>, String> {
    ask_manager(&app)?.threads().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn get_ask_thread(app: AppHandle, id: String) -> Result<Option<Thread>, String> {
    ask_manager(&app)?.thread(&id).map_err(|e| e.to_string())
}

/// A question typed in the panel. Returns the thread it went into.
#[tauri::command]
#[specta::specta]
pub fn ask_typed(
    app: AppHandle,
    thread_id: Option<String>,
    text: String,
) -> Result<String, String> {
    if text.trim().is_empty() {
        return Err("empty".to_string());
    }
    let mut question = NewQuestion::text(text, Via::Typed, thread_id);
    // Grabs waiting with the draft go with it.
    let captures = super::draft::take(&app);
    if !captures.is_empty() {
        let events = super::voice::capture_events(&captures);
        question.screen.pictures = captures
            .into_iter()
            .map(super::capture::Capture::into_picture)
            .collect();
        question.screen.timeline = Some(super::timeline::Timeline {
            words: Vec::new(),
            events,
        });
    }
    ask_manager(&app)?.ask(question).map_err(|e| e.to_string())
}

/// Stops the answer being worked on.
#[tauri::command]
#[specta::specta]
pub fn cancel_ask(app: AppHandle) -> Result<(), String> {
    ask_manager(&app)?.cancel();
    Ok(())
}

/// Reads an answer's spoken part aloud again.
#[tauri::command]
#[specta::specta]
pub fn replay_ask_answer(
    app: AppHandle,
    thread_id: String,
    answer_id: String,
) -> Result<(), String> {
    let thread = ask_manager(&app)?
        .thread(&thread_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No such thread".to_string())?;
    let answer = thread
        .exchanges
        .iter()
        .map(|e| &e.answer)
        .find(|a| a.id == answer_id)
        .ok_or_else(|| "No such answer".to_string())?;
    crate::speech::stop(&app);
    crate::speech::speak(&app, super::answer::split(&answer.text).0);
    Ok(())
}

/// Stops answering a thread from a running agent's conversation.
#[tauri::command]
#[specta::specta]
pub fn unlink_ask_agent(app: AppHandle, thread_id: String) -> Result<(), String> {
    ask_manager(&app)?
        .unlink_agent(&thread_id)
        .map_err(|e| e.to_string())
}

/// Asks a question again in another thread (`None`: a new one).
#[tauri::command]
#[specta::specta]
pub fn move_ask_question(
    app: AppHandle,
    from_thread_id: String,
    question_id: String,
    to_thread_id: Option<String>,
) -> Result<String, String> {
    ask_manager(&app)?
        .move_question(&from_thread_id, &question_id, to_thread_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn delete_ask_thread(app: AppHandle, id: String) -> Result<(), String> {
    ask_manager(&app)?.delete(&id).map_err(|e| e.to_string())
}

/// Merges the thread the tidy-up suggested into this one.
#[tauri::command]
#[specta::specta]
pub fn merge_ask_threads(app: AppHandle, thread_id: String) -> Result<(), String> {
    ask_manager(&app)?
        .merge(&thread_id)
        .map_err(|e| e.to_string())
}

/// Declines a merge suggestion.
#[tauri::command]
#[specta::specta]
pub fn keep_ask_threads_apart(app: AppHandle, thread_id: String) -> Result<(), String> {
    ask_manager(&app)?
        .keep_apart(&thread_id)
        .map_err(|e| e.to_string())
}

/// Deletes every thread, and the Claude Code sessions behind them.
#[tauri::command]
#[specta::specta]
pub fn delete_all_ask_threads(app: AppHandle) -> Result<(), String> {
    ask_manager(&app)?.delete_all().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn change_ask_settings(app: AppHandle, settings: AskSettings) -> Result<(), String> {
    let mut all = get_settings(&app);
    all.ask = settings;
    write_settings(&app, all);
    Ok(())
}
