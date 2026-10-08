//! Everything commands and background tasks share.

use std::path::PathBuf;
use std::sync::Mutex;

use herdr_status_core::{AgentList, Config, Env, Hub, Platform};

// Some fields are first read by the tray/config-watch tasks that follow.
#[allow(dead_code)]
pub struct AppState {
    pub platform: Platform,
    pub env: Env,
    pub config_path: PathBuf,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub hub: tokio::sync::Mutex<Hub>,
    pub config: Mutex<Config>,
    pub latest: Mutex<AgentList>,
    /// Keeps the config file watcher alive for the life of the app.
    pub config_watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

impl AppState {
    pub fn peek_lines(&self) -> u32 {
        self.config.lock().unwrap().ui.peek_lines
    }

    /// Chip label for a server id (falls back to the id).
    pub fn label_for(&self, server_id: &str) -> String {
        self.latest
            .lock()
            .unwrap()
            .servers
            .iter()
            .find(|s| s.id == server_id)
            .map(|s| s.label.clone())
            .unwrap_or_else(|| server_id.to_string())
    }
}
