use std::fs;
use std::thread;

use evdev::{Device, EventType, KeyCode};
use tauri::AppHandle;

use crate::commands;

fn is_ptt_code(code: u16) -> bool {
    matches!(
        code,
        c if c == KeyCode::KEY_F1.0
            || c == KeyCode::BTN_SIDE.0
            || c == KeyCode::BTN_EXTRA.0
    )
}

/// Lê teclado e mouse direto via evdev:
/// - F1 ou Mouse 4 (`BTN_SIDE`/`BTN_EXTRA`): press/release -> push-to-talk
/// - F2: press -> toggle reunião
///   Requer o usuário no grupo `input` (sudo usermod -aG input $USER + relogin).
pub fn start(app: AppHandle) {
    let entries = match fs::read_dir("/dev/input") {
        Ok(e) => e,
        Err(e) => {
            eprintln!(
                "[goat] sem acesso a /dev/input ({e}) — rode: sudo usermod -aG input SEU_USUARIO e relogue"
            );
            return;
        }
    };
    let mut devices = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("event") {
            continue;
        }
        let path = entry.path();
        let Ok(mut dev) = Device::open(&path) else {
            continue;
        };
        let supported = dev.supported_keys();
        let has_keyboard = supported.is_some_and(|k| k.contains(KeyCode::KEY_A));
        let has_mouse4 = supported
            .is_some_and(|k| k.contains(KeyCode::BTN_SIDE) || k.contains(KeyCode::BTN_EXTRA));
        if !has_keyboard && !has_mouse4 {
            continue;
        }
        devices += 1;
        let app = app.clone();
        thread::spawn(move || {
            eprintln!(
                "[goat] dispositivo evdev: {} ({})",
                dev.name().unwrap_or("?"),
                path.display()
            );
            loop {
                match dev.fetch_events() {
                    Ok(events) => {
                        for ev in events {
                            if ev.event_type() != EventType::KEY {
                                continue;
                            }
                            let code = ev.code();
                            let val = ev.value();
                            if is_ptt_code(code) && (val == 1 || val == 0) {
                                if val == 1 {
                                    commands::ptt_press(&app);
                                } else {
                                    commands::ptt_release(&app);
                                }
                            } else if code == KeyCode::KEY_F2.0 && val == 1 {
                                commands::toggle_meeting(&app);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[goat] evdev: {e}");
                        break;
                    }
                }
            }
        });
    }
    if devices == 0 {
        eprintln!("[goat] nenhum teclado/mouse via evdev (permissão do grupo input?)");
    }
}

#[cfg(test)]
mod tests {
    use super::is_ptt_code;
    use evdev::KeyCode;

    #[test]
    fn recognizes_f1_and_mouse4_buttons() {
        assert!(is_ptt_code(KeyCode::KEY_F1.0));
        assert!(is_ptt_code(KeyCode::BTN_SIDE.0));
        assert!(is_ptt_code(KeyCode::BTN_EXTRA.0));
        assert!(!is_ptt_code(KeyCode::KEY_F2.0));
    }
}
