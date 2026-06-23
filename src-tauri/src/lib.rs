//! Clip — Tauri application entry point.
//!
//! Wires the pure engine (`clip-core`) and storage (`clip-store`) to the OS:
//! a background clipboard monitor, a Spotlight-style palette window summoned by
//! a global hotkey, a tray icon, and the IPC commands the UI calls.

mod blobs;
mod capture;
mod commands;
mod monitor;
mod paste;
mod state;
mod sync;

use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use clip_store::Store;
use state::AppState;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// Unix milliseconds, used for ordering clip captures.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Grace window after showing during which a blur event is ignored (Wayland
/// emits a spurious unfocus right after a window maps).
const SHOW_GRACE_MS: i64 = 400;

/// Toggle the palette: if visible, hide; otherwise center, show, and focus it.
fn toggle_palette(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        match win.is_visible() {
            Ok(true) => {
                let _ = win.hide();
            }
            _ => {
                if let Some(state) = app.try_state::<AppState>() {
                    state.last_shown.store(now_ms(), Ordering::Relaxed);
                }
                let _ = win.center();
                let _ = win.show();
                let _ = win.set_focus();
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Summon hotkey: Ctrl/Cmd + Shift + V.
    let toggle_shortcut = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyV);

    tauri::Builder::default()
        // Must be the FIRST plugin: a second `clip` launch routes here and
        // toggles the palette, then exits — our Wayland-friendly summon.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            toggle_palette(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed && shortcut == &toggle_shortcut {
                        toggle_palette(app);
                    }
                })
                .build(),
        )
        .setup(move |app| {
            // --- Storage ------------------------------------------------------
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let store = Store::open(dir.join("clip.db"))?;
            let blob_dir = dir.join("blobs");
            std::fs::create_dir_all(&blob_dir)?;
            app.manage(AppState::new(store, blob_dir));

            // --- Global hotkey ------------------------------------------------
            app.global_shortcut().register(toggle_shortcut)?;

            // --- Tray ---------------------------------------------------------
            let show = MenuItem::with_id(app, "show", "Show Clip", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Clip — your clipboard, everywhere")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => toggle_palette(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { .. } = event {
                        toggle_palette(tray.app_handle());
                    }
                })
                .build(app)?;

            // --- Palette behaves like Spotlight: hide when it loses focus -----
            // A grace period after showing absorbs the spurious unfocus Wayland
            // emits right after a window maps. Disabled on Linux for now:
            // Hyprland/Wayland windows don't reliably hold focus, so blur-hide
            // makes the palette vanish unpredictably. The palette still hides on
            // Escape and after a pick. Proper Wayland blur-hide (xdg-activation
            // focus tracking / compositor overlay rules) is part of the planned
            // post-phase UX redesign. `SHOW_GRACE_MS` is kept for that.
            let _ = SHOW_GRACE_MS;
            if !cfg!(target_os = "linux") {
                if let Some(win) = app.get_webview_window("main") {
                    let win_for_event = win.clone();
                    let last_shown = app.state::<AppState>().last_shown.clone();
                    win.on_window_event(move |event| {
                        if let WindowEvent::Focused(false) = event {
                            let shown = last_shown.load(Ordering::Relaxed);
                            if now_ms() - shown > SHOW_GRACE_MS {
                                let _ = win_for_event.hide();
                            }
                        }
                    });
                }
            }

            // --- Start capturing ----------------------------------------------
            monitor::spawn(app.handle().clone());

            // --- Cross-device sync (only if configured) -----------------------
            if let Some(cfg) = sync::config_from_env() {
                sync::spawn(app.handle().clone(), cfg);
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_clips,
            commands::search_clips,
            commands::copy_clip,
            commands::set_pin,
            commands::delete_clip,
            commands::clear_history,
            commands::hide_palette,
        ])
        .run(tauri::generate_context!())
        .expect("error while running clip");
}
