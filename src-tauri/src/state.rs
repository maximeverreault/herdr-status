//! Everything commands and background tasks share.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use herdr_status_core::{AgentList, Config, Env, Hub, Platform};

/// Lock a std mutex, ignoring poisoning (the guarded data stays usable).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct AppState {
    // read by the tray/config-watch tasks (Plan 3 Tasks 4-5)
    #[allow(dead_code)]
    pub platform: Platform,
    // read by the tray/config-watch tasks (Plan 3 Tasks 4-5)
    #[allow(dead_code)]
    pub env: Env,
    pub config_path: PathBuf,
    // read by the tray/config-watch tasks (Plan 3 Tasks 4-5)
    #[allow(dead_code)]
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub hub: tokio::sync::Mutex<Hub>,
    pub config: Mutex<Config>,
    pub latest: Mutex<AgentList>,
    /// Banner text when `config.toml` is invalid; read by the UI at start.
    pub config_error: Mutex<Option<String>>,
    /// Keeps the config file watcher alive for the life of the app.
    // read by the tray/config-watch tasks (Plan 3 Tasks 4-5)
    #[allow(dead_code)]
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
