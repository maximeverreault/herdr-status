//! Everything commands and background tasks share.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use herdr_status_core::{AgentList, Config, Hub, Platform};

/// Lock a std mutex, ignoring poisoning (the guarded data stays usable).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct AppState {
    pub platform: Platform,
    pub config_path: PathBuf,
    pub log_dir: PathBuf,
    /// `None` once the app is shutting down (taken by the exit handler).
    pub hub: tokio::sync::Mutex<Option<Hub>>,
    pub config: Mutex<Config>,
    pub latest: Mutex<AgentList>,
    /// Banner text when `config.toml` is invalid; read by the UI at start.
    pub config_error: Mutex<Option<String>>,
    /// The serialised reload task; aborted on exit.
    pub reload_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    /// Keeps the config file watcher alive for the life of the app.
    pub config_watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

impl AppState {
    pub fn peek_lines(&self) -> u32 {
        lock(&self.config).ui.peek_lines
    }

    /// Chip label for a server id (falls back to the id).
    pub fn label_for(&self, server_id: &str) -> String {
        lock(&self.latest)
            .servers
            .iter()
            .find(|s| s.id == server_id)
            .map(|s| s.label.clone())
            .unwrap_or_else(|| server_id.to_string())
    }
}
