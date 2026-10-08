//! herdr-status desktop app: Tauri glue around `herdr_status_core`.
//!
//! There is no explicit hub shutdown on exit: process exit tears everything
//! down (bridge children are `kill_on_drop`).

mod commands;
mod config_watch;
mod startup;
mod state;
// `pub` only to avoid dead-code warnings until Task 5 wires the tray.
pub mod tray_icon;

// `tray` and `window` arrive in Tasks 5 and 6; until then provide no-op stubs.
mod tray {
    pub fn update_tray(_app: &tauri::AppHandle, _list: &herdr_status_core::AgentList) {}
}

use herdr_status_core::{paths, Env, Platform};
use tauri::Manager;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

/// `HERDR_STATUS_LOG=debug|trace` raises the log level (spec section 15).
fn log_level() -> log::LevelFilter {
    match std::env::var("HERDR_STATUS_LOG").ok().as_deref() {
        Some("debug") => log::LevelFilter::Debug,
        Some("trace") => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    }
}

pub fn run() {
    let env = Env::from_process();
    let platform = Platform::current();
    let log_dir = paths::app_log_dir(&env, platform);
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log_level())
                .max_file_size(2_000_000)
                .rotation_strategy(RotationStrategy::KeepSome(3))
                .targets([
                    Target::new(TargetKind::Folder {
                        path: log_dir.clone(),
                        file_name: None,
                    }),
                    Target::new(TargetKind::Stdout),
                ])
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_agents,
            commands::peek,
            commands::send_key,
            commands::send_message,
            commands::reconnect_all,
            commands::open_config,
            commands::open_logs,
            commands::get_config_error,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let state =
                tauri::async_runtime::block_on(startup::boot(&handle, env, platform, log_dir))?;
            app.manage(state);
            if let Err(e) = config_watch::start(&handle) {
                log::warn!("config watcher not started: {e}");
            }
            let window = app.get_webview_window("main").expect("main window exists");
            if std::env::args().any(|a| a == "--hidden") {
                let _ = window.hide();
            } else {
                let _ = window.show();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running herdr-status");
}
