//! Cue desktop shell (Tauri 2).
//!
//! Components:
//! * `window`   – floating palette window
//! * `hotkey`   – global hotkey manager with conflict detection
//! * `commands` – provider/model management, auth, settings, chat commands
//! * `chat`     – unified streaming chat orchestration
//! * `history`  – conversations (memory-only unless the user opts in)
//! * `state`    – shared state, diagnostics log
//!
//! Provider implementations, auth and secure storage live in `cue-core`.

mod chat;
mod commands;
mod history;
mod hotkey;
mod state;
mod window;

use std::sync::{Arc, Mutex};

use cue_core::secrets::{ChunkedStore, KeyringBackend, MemoryStore, SecretStore};
use cue_core::settings::Settings;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};
use tauri_plugin_global_shortcut::ShortcutState;

use crate::history::History;
use crate::state::AppState;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| window::show(app)))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        window::toggle(app);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            let settings_path = config_dir.join("settings.json");
            let history_path = data_dir.join("history.json");

            let mut startup_notes = Vec::new();
            let settings = match Settings::load(&settings_path) {
                Ok(s) => s,
                Err(e) => {
                    let _ = std::fs::copy(&settings_path, settings_path.with_extension("json.bak"));
                    startup_notes.push(format!("settings: {} {}", e.message, e.detail.unwrap_or_default()));
                    Settings::default()
                }
            };
            let history = if settings.persist_history { History::load(&history_path) } else { History::default() };

            // Secrets live only in the OS credential store. If it is not
            // available, fall back to memory (nothing written to disk).
            let (secrets, secret_store_error): (Arc<dyn SecretStore>, Option<String>) = match KeyringBackend::available() {
                Ok(()) => (Arc::new(ChunkedStore(KeyringBackend::new(state::KEYRING_SERVICE))), None),
                Err(e) => (
                    Arc::new(MemoryStore::default()),
                    Some(format!(
                        "The system credential store is unavailable ({}). Credentials are kept in memory for this session only.",
                        e.detail.unwrap_or(e.message)
                    )),
                ),
            };

            let hotkey = settings.hotkey.clone();
            let first_run = settings.connections.is_empty();
            let st = Arc::new(AppState {
                settings: Mutex::new(settings),
                settings_path,
                history: Mutex::new(history),
                history_path,
                secrets,
                secret_store_error,
                http: cue_core::http_client(),
                inflight: Mutex::new(Default::default()),
                sign_in: Mutex::new(None),
                refresh_lock: tokio::sync::Mutex::new(()),
                diagnostics: Mutex::new(Default::default()),
                session_usage: Mutex::new(Default::default()),
                work_dir: data_dir.join("claude-code-work"),
                hotkey_error: Mutex::new(None),
                shown_at: Mutex::new(None),
            });
            for n in startup_notes {
                st.log("error", n);
            }
            st.save_settings().ok(); // persists a newly generated host id
            app.manage(st.clone());

            // Global hotkey.
            let res = hotkey::apply(app.handle(), None, &hotkey);
            if !res.ok {
                let msg = res.error.unwrap_or_default();
                st.log("error", format!("Hotkey {hotkey}: {msg}"));
                *st.hotkey_error.lock().expect("lock") = Some(msg);
            }

            // Tray / menu bar icon.
            let open = MenuItem::with_id(app, "open", "Open Cue", true, None::<&str>)?;
            let settings_item = MenuItem::with_id(app, "settings", "AI Providers & Settings…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Cue", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &settings_item, &PredefinedMenuItem::separator(app)?, &quit])?;
            let mut tray = TrayIconBuilder::with_id("cue-tray")
                .tooltip("Cue")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| match ev.id().as_ref() {
                    "open" => window::show(app),
                    "settings" => window::open_settings(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        window::toggle(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            #[cfg(target_os = "macos")]
            {
                tray = tray.icon_as_template(true);
            }
            tray.build(app)?;

            // First run: show the window; the UI opens AI Providers itself
            // when no connection exists yet.
            if first_run {
                window::show(app.handle());
            }
            Ok(())
        })
        .on_window_event(|w, ev| match ev {
            // Closing the palette only hides it; quit via the tray menu.
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                window::hide(w.app_handle());
            }
            WindowEvent::Focused(false) => {
                let app = w.app_handle();
                let st = app.state::<Arc<AppState>>();
                let hide = st.settings.lock().map(|s| s.hide_on_blur).unwrap_or(true);
                let signing_in = st.sign_in.lock().map(|s| s.is_some()).unwrap_or(false);
                let just_shown = st
                    .shown_at
                    .lock()
                    .ok()
                    .and_then(|t| *t)
                    .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(700));
                if hide && !signing_in && !just_shown {
                    if let Some(win) = app.get_webview_window(window::MAIN) {
                        let _ = win.hide();
                    }
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::add_connection,
            commands::update_connection,
            commands::remove_connection,
            commands::test_connection,
            commands::claude_code_status,
            commands::chatgpt_sign_in,
            commands::chatgpt_cancel_sign_in,
            commands::set_default_model,
            commands::set_mode_model,
            commands::update_prefs,
            commands::check_hotkey,
            commands::set_hotkey,
            commands::acknowledge_connection,
            commands::set_launch_at_login,
            commands::get_launch_at_login,
            commands::send_message,
            commands::cancel_message,
            commands::list_conversations,
            commands::get_conversation,
            commands::delete_conversation,
            commands::clear_history,
            commands::copy_text,
            commands::hide_window,
            commands::open_external,
            commands::get_diagnostics,
            commands::clear_diagnostics,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Cue");
}
