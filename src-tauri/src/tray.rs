use std::io::Write;
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

/// Copia o texto pro clipboard e injeta Ctrl+V no campo focado (wtype,
/// teclado virtual Wayland). Retorna false se wtype não estiver disponível.
pub fn type_text(text: &str) -> bool {
    let mut copy = match Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let mut stdin = match copy.stdin.take() {
        Some(s) => s,
        None => return false,
    };
    let _ = stdin.write_all(text.as_bytes());
    drop(stdin);
    if copy.wait().map(|s| !s.success()).unwrap_or(true) {
        return false;
    }
    match Command::new("wtype")
        .args(["-M", "ctrl", "v"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(s) => s.success(),
        Err(_) => false,
    }
}
