//! Commands the webview invokes. Thin wrappers over `herdr_status_core`.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use herdr_status_core::{actions, ActionError, AgentList, Client, TransportSource};
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::state::AppState;

/// Upper bound for connecting plus one request.
const ACTION_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Serialize)]
pub struct PeekResult {
    pub text: String,
    pub truncated: bool,
}

#[tauri::command]
pub fn get_agents(state: State<'_, Arc<AppState>>) -> AgentList {
    state.latest.lock().unwrap().clone()
}

/// Resolve a connected client and the server's chip label. The hub lock is
/// held only to look up the source, never across `transport()`.
async fn client_for(state: &AppState, server: &str) -> Result<(Client, String), ActionError> {
    let source: Arc<dyn TransportSource> = {
        let hub = state.hub.lock().await;
        hub.source_for(server)
    }
    .ok_or_else(|| ActionError::unknown_server(server))?;
    let label = state.label_for(server);
    match tokio::time::timeout(ACTION_TIMEOUT, source.transport()).await {
        Err(_) => Err(ActionError::timeout(&label)),
        Ok(Err(_)) => Err(ActionError::unreachable(&label)),
        Ok(Ok(transport)) => Ok((Client::new(transport), label)),
    }
}

/// Bound one action call by [`ACTION_TIMEOUT`].
async fn bounded<T>(
    label: &str,
    call: impl Future<Output = Result<T, ActionError>>,
) -> Result<T, ActionError> {
    tokio::time::timeout(ACTION_TIMEOUT, call)
        .await
        .unwrap_or_else(|_| Err(ActionError::timeout(label)))
}

#[tauri::command]
pub async fn peek(
    state: State<'_, Arc<AppState>>,
    server: String,
    pane: String,
) -> Result<PeekResult, ActionError> {
    let (client, label) = client_for(&state, &server).await?;
    let read = bounded(
        &label,
        actions::peek(&client, &pane, state.peek_lines(), &label),
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
    let (client, label) = client_for(&state, &server).await?;
    bounded(&label, actions::send_key(&client, &pane, &key, &label)).await
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, Arc<AppState>>,
    server: String,
    pane: String,
    text: String,
) -> Result<(), ActionError> {
    let (client, label) = client_for(&state, &server).await?;
    bounded(&label, actions::send_message(&client, &pane, &text, &label)).await
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
    let _ = std::fs::create_dir_all(&state.log_dir);
    app.opener()
        .open_path(state.log_dir.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}
