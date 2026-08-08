use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::meeting::ErrorEvent;
use crate::prompt::PromptRecorder;
use crate::AppState;

pub use crate::meeting::TtResultEvent;

#[derive(Serialize)]
pub struct OkPayload {
    pub ok: bool,
}

#[tauri::command]
pub fn meeting_start(state: State<AppState>) -> Result<OkPayload, String> {
    let mut m = state.meeting.lock().unwrap();
    if !m.is_running() {
        m.start().map_err(|e| e.to_string())?;
        crate::tray::update_toggle_label(true);
    }
    Ok(OkPayload { ok: true })
}

#[tauri::command]
pub fn meeting_stop(state: State<AppState>) -> Result<OkPayload, String> {
    state.meeting.lock().unwrap().stop();
    crate::tray::update_toggle_label(false);
    Ok(OkPayload { ok: true })
}

#[tauri::command]
pub fn tt_start(state: State<AppState>) -> Result<OkPayload, String> {
    state
        .prompt
        .lock()
        .unwrap()
        .start()
        .map_err(|e| e.to_string())?;
    Ok(OkPayload { ok: true })
}

#[tauri::command]
pub async fn tt_stop(
    state: State<'_, AppState>,
    language: Option<String>,
) -> Result<serde_json::Value, String> {
    let lang = language.unwrap_or_else(|| "pt".to_string());
    let prompt: std::sync::Arc<std::sync::Mutex<PromptRecorder>> = state.prompt.clone();
    let result = tauri::async_runtime::spawn_blocking(move || prompt.lock().unwrap().stop(&lang))
        .await
        .map_err(|e| e.to_string())?;
    match result {
        Ok(Some(text)) => Ok(serde_json::json!({"ok": true, "text": text})),
        Ok(None) => Ok(serde_json::json!({"ok": true, "text": "", "active": false})),
        Err(e) => Ok(serde_json::json!({"ok": false, "text": "", "error": e.to_string()})),
    }
}

#[tauri::command]
pub fn get_status(state: State<AppState>) -> serde_json::Value {
    let whisper = state.backend.is_online();
    let meeting = state.meeting.lock().unwrap().is_running();
    serde_json::json!({
        "whisper": whisper,
        "meeting": meeting,
        "model": crate::whisper::MODEL_NAME,
    })
}

#[tauri::command]
pub fn video_transcribe(
    app: AppHandle,
    state: State<AppState>,
    path: String,
    language: Option<String>,
) -> Result<OkPayload, String> {
    let lang = language.unwrap_or_else(|| "pt".to_string());
    if !crate::video::start(app, state.backend.clone(), state.video.clone(), path, lang) {
        return Err("já existe uma transcrição de vídeo em andamento".into());
    }
    Ok(OkPayload { ok: true })
}

#[tauri::command]
pub fn video_cancel(state: State<AppState>) -> Result<OkPayload, String> {
    state.video.cancel();
    Ok(OkPayload { ok: true })
}

pub fn toggle_meeting(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut m = state.meeting.lock().unwrap();
    if m.is_running() {
        m.stop();
        crate::tray::update_toggle_label(false);
        crate::tray::notify("Goat Reunião", "Gravação pausada");
    } else {
        match m.start() {
            Ok(()) => {
                crate::tray::update_toggle_label(true);
                crate::tray::notify("Goat Reunião", "Gravação iniciada");
            }
            Err(e) => {
                let msg = format!("falha ao iniciar: {e}");
                let _ = app.emit(
                    "error",
                    ErrorEvent {
                        message: msg.clone(),
                    },
                );
                crate::tray::notify("Goat Reunião", &msg);
            }
        }
    }
}

pub fn ptt_press(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Err(e) = state.prompt.lock().unwrap().start() {
        crate::tray::notify("Prompt", &format!("erro: {e}"));
        return;
    }
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.show();
    }
}

pub fn ptt_release(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.hide();
    }
    let state = app.state::<AppState>();
    let prompt = state.prompt.clone();
    let handle = app.clone();
    std::thread::spawn(move || match prompt.lock().unwrap().stop("pt") {
        Ok(Some(text)) if !text.is_empty() => {
            let _ = handle.emit("tt_result", TtResultEvent { text: text.clone() });
            if crate::tray::type_text(&text) {
                crate::tray::notify("Goat Reunião", "Prompt transcrito e digitado ✓");
            } else {
                crate::tray::notify("Goat Reunião", "Transcrito — copiado (wtype ausente)");
            }
        }
        Ok(Some(_)) => {
            let _ = handle.emit(
                "error",
                ErrorEvent {
                    message: "nenhum áudio detectado".into(),
                },
            );
        }
        Ok(None) => {}
        Err(e) => {
            let _ = handle.emit(
                "error",
                ErrorEvent {
                    message: format!("falha ao parar PTT: {e}"),
                },
            );
        }
    });
}

pub fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
