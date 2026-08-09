use std::process::{Command, Stdio};
use std::sync::OnceLock;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::AppHandle;

use crate::commands;

static TOGGLE_ITEM: OnceLock<MenuItem<tauri::Wry>> = OnceLock::new();

pub fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Iniciar Reunião", true, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Mostrar Janela", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &show, &quit])?;
    let _ = TOGGLE_ITEM.set(toggle);

    TrayIconBuilder::with_id("goat-tray")
        .icon(tauri::image::Image::from_bytes(include_bytes!(
            "../icons/32x32.png"
        ))?)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" => {
                commands::toggle_meeting(app);
            }
            "show" => {
                commands::show_window(app);
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

pub fn update_toggle_label(running: bool) {
    if let Some(item) = TOGGLE_ITEM.get() {
        let _ = item.set_text(if running {
            "Parar Reunião"
        } else {
            "Iniciar Reunião"
        });
    }
}

pub fn notify(title: &str, body: &str) {
    let _ = Command::new("notify-send")
        .args(["-u", "low", "-a", "Goat Reunião", title, body])
        .status();
}

/// Copia o texto pro clipboard e injeta Ctrl+V no campo focado.
/// Tenta Wayland (wtype) primeiro, depois X11 (xdotool).
/// Retorna false se nenhum estiver disponível.
pub fn type_text(text: &str) -> bool {
    // Tentar wl-copy (Wayland)
    if let Ok(mut child) = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(mut s) = child.stdin.take() {
            let _ = std::io::Write::write_all(&mut s, text.as_bytes());
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        if ok {
            return paste_wayland() || paste_x11();
        }
    }
    // Fallback: xclip (X11)
    if let Ok(mut child) = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(mut s) = child.stdin.take() {
            let _ = std::io::Write::write_all(&mut s, text.as_bytes());
        }
        if child.wait().map(|s| s.success()).unwrap_or(false) {
            return paste_x11();
        }
    }
    false
}

fn paste_wayland() -> bool {
    Command::new("wtype")
        .args(["-M", "ctrl", "v"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn paste_x11() -> bool {
    Command::new("xdotool")
        .args(["key", "ctrl+v"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
