mod audio;
mod commands;
pub(crate) mod hotkeys;
mod ipc;
pub(crate) mod meeting;
mod prompt;
pub(crate) mod tray;
mod video;
mod whisper;

use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager};

pub struct AppState {
    pub backend: Arc<whisper::WhisperBackend>,
    pub meeting: Arc<Mutex<meeting::MeetingRecorder>>,
    pub prompt: Arc<Mutex<prompt::PromptRecorder>>,
    pub video: Arc<video::VideoJob>,
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let backend = Arc::new(whisper::WhisperBackend::new());
            let handle = app.handle().clone();
            let resource_dir = app
                .path()
                .resource_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));

            let backend_start = backend.clone();
            let handle_start = handle.clone();
            std::thread::spawn(move || {
                if let Err(e) = backend_start.start(&resource_dir) {
                    let _ = handle_start.emit(
                        "error",
                        meeting::ErrorEvent {
                            message: format!("motor offline: {e}"),
                        },
                    );
                }
            });

            let meeting = Arc::new(Mutex::new(meeting::MeetingRecorder::new(
                backend.clone(),
                handle.clone(),
            )));
            let prompt = Arc::new(Mutex::new(prompt::PromptRecorder::new(backend.clone())));
            let video = Arc::new(video::VideoJob::new());
            app.manage(AppState {
                backend,
                meeting,
                prompt,
                video,
            });

            if let Err(e) = tray::setup_tray(&handle) {
                eprintln!("[goat] falha no tray: {e}");
            }
            ipc::start(handle.clone());
            hotkeys::start(handle.clone());
            if let Some(ov) = handle.get_webview_window("overlay") {
                if let Ok(Some(m)) = handle.primary_monitor() {
                    let pos = m.position();
                    let size = m.size();
                    let (w, h) = (420i32, 90i32);
                    let x = pos.x + (size.width as i32 - w) / 2;
                    let y = pos.y + size.height as i32 - h - 8;
                    let _ = ov.set_position(tauri::Position::Physical(
                        tauri::PhysicalPosition::new(x, y),
                    ));
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::meeting_start,
            commands::meeting_stop,
            commands::tt_start,
            commands::tt_stop,
            commands::get_status,
            commands::video_transcribe,
            commands::video_cancel
        ])
        .run(tauri::generate_context!())
        .expect("erro ao executar Goat Reunião");
}
