//! Boot: paths, config (with first-launch prefill), hub, and the loop that
//! forwards every `AgentList` to the webview and the tray.

use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use herdr_status_core::paths;
use herdr_status_core::prefill::{gather_inputs, prefill_config, render_prefilled};
use herdr_status_core::{
    default_source_factory, Config, Env, Hub, Platform, ProcessRunner, TokioRunner, WatcherConfig,
};
use tauri::{AppHandle, Emitter};

use crate::state::{lock, AppState};
use crate::tray;

/// Read `config.toml`, creating it on first launch. Returns the config and an
/// optional banner message when the file is invalid (defaults are used then).
pub async fn load_or_create(
    path: &Path,
    platform: Platform,
    runner: &dyn ProcessRunner,
    env: &Env,
) -> (Config, Option<String>) {
    if !path.exists() {
        let inputs = gather_inputs(runner, platform, env).await;
        let cfg = prefill_config(platform, &inputs);
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                log::warn!("could not create {}: {e}", parent.display());
            }
        }
        if let Err(e) = std::fs::write(path, render_prefilled(&cfg)) {
            log::warn!("could not write initial config {}: {e}", path.display());
        }
        return (cfg, None);
    }
    match std::fs::read_to_string(path) {
        Ok(text) => match Config::parse(&text) {
            Ok(cfg) => (cfg, None),
            Err(e) => (Config::default(), Some(format!("{}: {e}", path.display()))),
        },
        Err(e) => (Config::default(), Some(format!("{}: {e}", path.display()))),
    }
}

pub async fn boot(
    app: &AppHandle,
    env: Env,
    platform: Platform,
    log_dir: PathBuf,
) -> Result<Arc<AppState>, String> {
    let config_path = paths::app_config_file(&env, platform);
    let data_dir = paths::app_data_dir(&env, platform);
    log::info!("config file: {}", config_path.display());
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        log::warn!("could not create {}: {e}", data_dir.display());
    }
    let runner: Arc<dyn ProcessRunner> = Arc::new(TokioRunner);

    let (config, config_error) =
        load_or_create(&config_path, platform, runner.as_ref(), &env).await;
    // Runs under `block_on` in `setup`, which enters Tauri's tokio runtime, so
    // the `tokio::spawn` inside `Hub::new` works.
    let factory = default_source_factory(env, platform, runner, data_dir);
    let mut hub = Hub::new(factory, WatcherConfig::default());
    hub.apply_config(&config, platform).await;
    let mut list_rx = hub.subscribe();

    let state = Arc::new(AppState {
        platform,
        config_path,
        log_dir,
        hub: tokio::sync::Mutex::new(Some(hub)),
        config: Mutex::new(config),
        latest: Mutex::new(Default::default()),
        config_error: Mutex::new(config_error),
        reload_task: Mutex::new(None),
        config_watcher: Mutex::new(None),
    });

    let forward_app = app.clone();
    let forward_state = state.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let list = list_rx.borrow_and_update().clone();
            *lock(&forward_state.latest) = list.clone();
            if let Err(e) = forward_app.emit("agents", &list) {
                log::warn!("emit agents failed: {e}");
            }
            tray::refresh_tray(&forward_app);
            if list_rx.changed().await.is_err() {
                break;
            }
        }
    });

    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use herdr_client::transport::bridge::ChildCommand;
    use herdr_status_core::remote_connect::RunOutput;
    use std::time::Duration;

    struct NoHerdr;
    #[async_trait::async_trait]
    impl ProcessRunner for NoHerdr {
        async fn run(&self, _c: &ChildCommand, _t: Duration) -> std::io::Result<RunOutput> {
            Ok(RunOutput {
                exit_code: Some(127),
                stdout: String::new(),
                stderr: "not found".into(),
            })
        }
    }

    #[tokio::test]
    async fn missing_config_is_created_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        let (cfg, error) = load_or_create(&path, Platform::Mac, &NoHerdr, &Env::default()).await;
        assert_eq!(cfg, Config::default());
        assert!(error.is_none());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            Config::DEFAULT_TOML
        );
    }

    #[tokio::test]
    async fn invalid_config_falls_back_to_defaults_with_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\npeek_lines = \"many\"\n").unwrap();
        let (cfg, error) = load_or_create(&path, Platform::Mac, &NoHerdr, &Env::default()).await;
        assert_eq!(cfg, Config::default());
        assert!(error.unwrap().contains("line 2"));
    }

    #[tokio::test]
    async fn valid_config_is_loaded_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\npeek_lines = 12\n").unwrap();
        let (cfg, error) = load_or_create(&path, Platform::Mac, &NoHerdr, &Env::default()).await;
        assert_eq!(cfg.ui.peek_lines, 12);
        assert!(error.is_none());
    }
}
