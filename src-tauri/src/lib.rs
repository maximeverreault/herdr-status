//! herdr-status desktop app: Tauri glue around `herdr_status_core`.
//!
//! On exit the hub is taken out of the app state and shut down (bounded by a
//! 2 s timeout). Bridge children are `kill_on_drop`, so even if the timeout
//! fires, dropping the hub cannot leave bridge processes behind.

mod commands;
mod config_watch;
mod startup;
mod state;
mod tray;
mod tray_icon;
mod window;

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use herdr_status_core::{paths, Env, Platform};
use tauri::Manager;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};
use tauri_plugin_window_state::StateFlags;

/// `HERDR_STATUS_LOG=debug|trace` raises the log level (spec section 15).
fn log_level() -> log::LevelFilter {
    match std::env::var("HERDR_STATUS_LOG").ok().as_deref() {
        Some("debug") => log::LevelFilter::Debug,
        Some("trace") => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    }
}

/// Set in `run()`; lets code that must not block the main thread check for it.
pub(crate) static MAIN_THREAD: OnceLock<std::thread::ThreadId> = OnceLock::new();

pub fn run() {
    let _ = MAIN_THREAD.set(std::thread::current().id());
    let env = Env::from_process();
    let platform = Platform::current();
    let log_dir = paths::app_log_dir(&env, platform);
    let app = tauri::Builder::default()
        // Must be first so a second launch never runs `boot`.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            log::info!("second instance launched; showing the main window");
            window::show(app);
        }))
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
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .plugin(
            tauri_plugin_window_state::Builder::default()
                // Visibility is owned by `--hidden` and the tray only.
                .with_state_flags(StateFlags::all() & !StateFlags::VISIBLE)
                .build(),
        )
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
            tray::build(&handle)?;
            window::install_close_to_hide(&handle);
            let window = app.get_webview_window("main").expect("main window exists");
            if std::env::args().any(|a| a == "--hidden") {
                let _ = window.hide();
            } else {
                let _ = window.show();
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building herdr-status");

    app.run(|app, event| match event {
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => window::show(app),
        tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit => shutdown_hub(app),
        _ => {}
    });
}

/// Stop every watcher and bridge. Idempotent: the hub is taken on first call.
fn shutdown_hub(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<Arc<state::AppState>>() else {
        return;
    };
    let state = state.inner().clone();
    // A long `apply_config` must not hold the hub lock past the exit timeout.
    if let Some(task) = state::lock(&state.reload_task).take() {
        task.abort();
    }
    // The timeout also covers waiting for the hub lock (a reload may hold it).
    let result =
        tauri::async_runtime::block_on(tokio::time::timeout(Duration::from_secs(2), async move {
            let hub = state.hub.lock().await.take();
            if let Some(hub) = hub {
                log::info!("shutting down hub");
                hub.shutdown().await;
                log::info!("hub shut down");
            }
        }));
    if result.is_err() {
        log::warn!("hub could not be shut down in time; bridges may be left behind");
    }
}
