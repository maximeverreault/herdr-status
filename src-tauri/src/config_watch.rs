//! Live reload of `config.toml`.

use std::path::Path;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use herdr_status_core::Config;
use notify::{EventKind, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter, Manager};

use crate::state::{lock, AppState};

const DEBOUNCE: Duration = Duration::from_millis(500);

pub fn read_config(path: &Path) -> Result<Config, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Config::parse(&text).map_err(|e| e.to_string())
}

/// Call `on_quiet` once after each burst of signals, when no new signal has
/// arrived for `quiet`. Returns when the sender is dropped.
fn debounce(rx: mpsc::Receiver<()>, quiet: Duration, mut on_quiet: impl FnMut()) {
    while rx.recv().is_ok() {
        while rx.recv_timeout(quiet).is_ok() {}
        on_quiet();
    }
}

/// Watch the config file's directory; on changes to the file, reload after a
/// quiet period and apply or report. Reloads are processed one at a time by a
/// single long-lived task.
pub fn start(app: &AppHandle) -> Result<(), String> {
    let state: Arc<AppState> = app.state::<Arc<AppState>>().inner().clone();
    let config_path = state.config_path.clone();
    let dir = config_path
        .parent()
        .ok_or("config path has no parent")?
        .to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let (event_tx, event_rx) = mpsc::channel::<()>();
    let (reload_tx, mut reload_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let file_name = config_path.file_name().map(|n| n.to_os_string());
    let mut watcher =
        notify::recommended_watcher(move |result: notify::Result<notify::Event>| match result {
            Ok(event) => {
                if matches!(event.kind, EventKind::Access(_)) {
                    return;
                }
                if event
                    .paths
                    .iter()
                    .any(|p| p.file_name() == file_name.as_deref())
                {
                    let _ = event_tx.send(());
                }
            }
            Err(e) => log::debug!("config watcher error: {e}"),
        })
        .map_err(|e| e.to_string())?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .map_err(|e| e.to_string())?;
    *lock(&state.config_watcher) = Some(watcher);

    std::thread::Builder::new()
        .name("config-watch".into())
        .spawn(move || {
            debounce(event_rx, DEBOUNCE, || {
                let _ = reload_tx.send(());
            })
        })
        .map_err(|e| e.to_string())?;

    let app = app.clone();
    let state_for_task = state.clone();
    let task = tauri::async_runtime::spawn(async move {
        while reload_rx.recv().await.is_some() {
            // Stacked signals collapse into one reload.
            while reload_rx.try_recv().is_ok() {}
            match read_config(&config_path) {
                Ok(cfg) => {
                    log::info!("config reloaded from {}", config_path.display());
                    *lock(&state.config) = cfg.clone();
                    if let Some(hub) = state.hub.lock().await.as_mut() {
                        hub.apply_config(&cfg, state.platform).await;
                    }
                    *lock(&state.config_error) = None;
                    let _ = app.emit("config_error", Option::<String>::None);
                }
                Err(message) => {
                    log::warn!("config invalid, keeping previous: {message}");
                    *lock(&state.config_error) = Some(message.clone());
                    let _ = app.emit("config_error", Some(message));
                }
            }
        }
    });
    *lock(&state_for_task.reload_task) = Some(task);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn debounce_collapses_bursts_and_ends_when_sender_drops() {
        let quiet = Duration::from_millis(20);
        let (tx, rx) = mpsc::channel::<()>();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let handle = std::thread::spawn(move || {
            debounce(rx, quiet, || {
                counter.fetch_add(1, Ordering::SeqCst);
            })
        });
        for _ in 0..5 {
            tx.send(()).unwrap();
        }
        std::thread::sleep(quiet * 6);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tx.send(()).unwrap();
        tx.send(()).unwrap();
        std::thread::sleep(quiet * 6);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        drop(tx);
        handle.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn reload_outcome_distinguishes_valid_and_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\npeek_lines = 7\n").unwrap();
        match read_config(&path) {
            Ok(cfg) => assert_eq!(cfg.ui.peek_lines, 7),
            Err(e) => panic!("unexpected {e}"),
        }
        std::fs::write(&path, "[ui\n").unwrap();
        let err = read_config(&path).unwrap_err();
        assert!(err.contains("config.toml line 1"), "{err}");
        std::fs::remove_file(&path).unwrap();
        assert!(read_config(&path).is_err());
    }
}
