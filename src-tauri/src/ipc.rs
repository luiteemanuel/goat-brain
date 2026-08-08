use std::io::BufRead;
use std::io::BufReader;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;

use tauri::AppHandle;

use crate::commands;

pub const SOCKET_PATH: &str = "/tmp/goat-reuniao.sock";

pub fn start(app: AppHandle) {
    let _ = std::fs::remove_file(SOCKET_PATH);
    let listener = match UnixListener::bind(SOCKET_PATH) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[goat] ipc socket: {e}");
            return;
        }
    };
    eprintln!("[goat] ipc ouvindo em {SOCKET_PATH}");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    let app = app.clone();
                    std::thread::spawn(move || handle(s, app));
                }
                Err(_) => break,
            }
        }
    });
}

fn handle(stream: UnixStream, app: AppHandle) {
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(cmd) = line else { break };
        match cmd.trim() {
            "ptt_press" => commands::ptt_press(&app),
            "ptt_release" => commands::ptt_release(&app),
            "meeting_toggle" => commands::toggle_meeting(&app),
            "show" => commands::show_window(&app),
            "quit" => app.exit(0),
            other => eprintln!("[goat] ipc comando desconhecido: {other}"),
        }
    }
}
