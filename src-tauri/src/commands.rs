//! Commands the webview invokes. Thin wrappers over `herdr_status_core`.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

use herdr_status_core::{actions, ActionError, AgentList, Client, TransportSource};
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::state::{lock, AppState};

/// Upper bound for connecting plus one request. Longer than the bridge's own
/// 20 s ready timeout so its more specific error wins.
const ACTION_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Serialize)]
pub struct PeekResult {
    pub text: String,
    pub truncated: bool,
}

#[tauri::command]
pub fn get_agents(state: State<'_, Arc<AppState>>) -> AgentList {
    lock(&state.latest).clone()
}

/// Resolve a connected client and the server's chip label. The hub lock is
/// held only to look up the source, never across `transport()`.
async fn client_for(
    state: &AppState,
    server: &str,
    deadline: Instant,
) -> Result<(Client, String), ActionError> {
    let source: Arc<dyn TransportSource> = {
        let hub = state.hub.lock().await;
        hub.source_for(server)
    }
    .ok_or_else(|| ActionError::unknown_server(server))?;
    let label = state.label_for(server);
    match tokio::time::timeout_at(deadline, source.transport()).await {
        Err(_) => {
            log::warn!("quick action: connecting to {server} timed out");
            Err(ActionError::timeout(&label))
        }
        Ok(Err(reason)) => {
            log::warn!("quick action: cannot reach {server}: {reason}");
            Err(ActionError::unreachable(&label))
        }
        Ok(Ok(transport)) => Ok((Client::new(transport), label)),
    }
}

/// Bound one action call by the shared deadline.
async fn bounded<T>(
    server: &str,
    label: &str,
    deadline: Instant,
    call: impl Future<Output = Result<T, ActionError>>,
) -> Result<T, ActionError> {
    match tokio::time::timeout_at(deadline, call).await {
        Ok(result) => result,
        Err(_) => {
            log::warn!("quick action: request to {server} timed out");
            Err(ActionError::timeout(label))
        }
    }
}

#[tauri::command]
pub async fn peek(
    state: State<'_, Arc<AppState>>,
    server: String,
    pane: String,
) -> Result<PeekResult, ActionError> {
    log::info!("quick action: peek server={server} pane={pane}");
    let deadline = Instant::now() + ACTION_TIMEOUT;
    let (client, label) = client_for(&state, &server, deadline).await?;
    let lines = state.peek_lines();
    let read = bounded(
        &server,
        &label,
        deadline,
        actions::peek(&client, &pane, lines, &label),
    )
    .await?;
    Ok(PeekResult {
        text: read.text,
        truncated: read.truncated,
    })
}

#[tauri::command]
pub async fn send_key(
    state: State<'_, Arc<AppState>>,
    server: String,
    pane: String,
    key: String,
) -> Result<(), ActionError> {
    log::info!("quick action: send_key server={server} pane={pane}");
    let deadline = Instant::now() + ACTION_TIMEOUT;
    let (client, label) = client_for(&state, &server, deadline).await?;
    bounded(
        &server,
        &label,
        deadline,
        actions::send_key(&client, &pane, &key, &label),
    )
    .await
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, Arc<AppState>>,
    server: String,
    pane: String,
    text: String,
) -> Result<(), ActionError> {
    log::info!("quick action: send_message server={server} pane={pane}");
    let deadline = Instant::now() + ACTION_TIMEOUT;
    let (client, label) = client_for(&state, &server, deadline).await?;
    bounded(
        &server,
        &label,
        deadline,
        actions::send_message(&client, &pane, &text, &label),
    )
    .await
}

#[tauri::command]
pub fn get_config_error(state: State<'_, Arc<AppState>>) -> Option<String> {
    lock(&state.config_error).clone()
}

#[tauri::command]
pub async fn reconnect_all(state: State<'_, Arc<AppState>>) -> Result<(), ()> {
    state.hub.lock().await.reconnect_all();
    Ok(())
}

#[tauri::command]
pub fn open_config(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    if !state.config_path.exists() {
        if let Some(parent) = state.config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&state.config_path, herdr_status_core::Config::DEFAULT_TOML)
            .map_err(|e| e.to_string())?;
    }
    app.opener()
        .open_path(state.config_path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_logs(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    if let Err(e) = std::fs::create_dir_all(&state.log_dir) {
        log::warn!("could not create {}: {e}", state.log_dir.display());
    }
    app.opener()
        .open_path(state.log_dir.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}
