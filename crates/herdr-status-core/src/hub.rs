//! Owns one watcher per configured server, merges their states, and publishes
//! the sorted [`AgentList`] through a `watch` channel.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use herdr_client::transport::bridge::BridgeTransport;
use herdr_client::{Client, Transport};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::config::{Config, LocalConfig, RemoteConfig, WslConfig, LOCAL_SERVER_ID};
use crate::launchers::{self, SshOptions};
use crate::model::{assemble, AgentList, ServerKind, ServerState};
use crate::paths::{self, Env, Platform};
use crate::remote_connect::{ProcessRunner, RemoteCache};
use crate::transport_source::{
    FallbackTransport, FixedSource, RemoteSource, TransportSource, WslSource,
};
use crate::watcher::{spawn_watcher, WatcherConfig, WatcherHandle};

/// One configured server, as the hub sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerSpec {
    Local(LocalConfig),
    Wsl(WslConfig),
    Remote(RemoteConfig),
}

impl ServerSpec {
    pub fn id(&self) -> &str {
        match self {
            ServerSpec::Local(_) => LOCAL_SERVER_ID,
            ServerSpec::Wsl(w) => &w.label,
            ServerSpec::Remote(r) => &r.label,
        }
    }

    pub fn kind(&self) -> ServerKind {
        match self {
            ServerSpec::Local(_) => ServerKind::Local,
            ServerSpec::Wsl(_) => ServerKind::Wsl,
            ServerSpec::Remote(_) => ServerKind::Remote,
        }
    }
}

/// Display order: local, WSL entries (Windows only), remotes.
pub fn specs_from_config(cfg: &Config, platform: Platform) -> Vec<ServerSpec> {
    let mut specs = Vec::new();
    if cfg.local.enabled {
        specs.push(ServerSpec::Local(cfg.local.clone()));
    }
    if platform == Platform::Windows {
        specs.extend(cfg.wsl.iter().cloned().map(ServerSpec::Wsl));
    }
    specs.extend(cfg.remote.iter().cloned().map(ServerSpec::Remote));
    specs
}

pub type SourceFactory = Arc<dyn Fn(&ServerSpec) -> Arc<dyn TransportSource> + Send + Sync>;

/// The production factory: Unix socket or named pipe (with `herdr.exe`
/// fallback) locally, `wsl.exe` for WSL, SSH bridge for remotes.
pub fn default_source_factory(
    env: Env,
    platform: Platform,
    runner: Arc<dyn ProcessRunner>,
    data_dir: PathBuf,
) -> SourceFactory {
    let cache_path = data_dir.join("cache.json");
    let cache = Arc::new(Mutex::new(RemoteCache::load(&cache_path)));
    Arc::new(move |spec: &ServerSpec| -> Arc<dyn TransportSource> {
        match spec {
            ServerSpec::Local(local) => {
                let path = local
                    .socket_path
                    .clone()
                    .unwrap_or_else(|| paths::herdr_socket_path(&env, platform, &local.session));
                Arc::new(FixedSource(local_transport(
                    platform,
                    &path,
                    &local.session,
                )))
            }
            ServerSpec::Wsl(wsl) => Arc::new(WslSource::new(
                wsl.distro.clone(),
                wsl.session.clone(),
                runner.clone(),
            )),
            ServerSpec::Remote(remote) => {
                // Re-created at every watcher start: macOS periodically cleans /tmp.
                let control_dir =
                    paths::ssh_control_dir(&env, platform).and_then(|dir| secure_control_dir(&dir));
                let ssh = SshOptions {
                    client: platform,
                    control_dir,
                };
                Arc::new(RemoteSource::new(
                    remote.clone(),
                    runner.clone(),
                    ssh,
                    cache.clone(),
                    cache_path.clone(),
                ))
            }
        }
    })
}

/// Create `dir` (mode 0700) and return it only if it is a private directory we
/// own. The directory lives in world-writable /tmp, so a pre-existing one could
/// belong to someone else. Ownership check: we create a marker file inside (this
/// only succeeds if the directory is writable by us) and compare its uid, which
/// is our effective uid, with the directory's uid; the mode must be exactly 0700.
#[cfg(unix)]
fn secure_control_dir(dir: &std::path::Path) -> Option<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let check = || -> std::io::Result<bool> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let dir_meta = std::fs::symlink_metadata(dir)?;
        let marker = dir.join(format!(".owner-check-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)?;
        let marker_meta = std::fs::symlink_metadata(&marker);
        let _ = std::fs::remove_file(&marker);
        let my_uid = marker_meta?.uid();
        Ok(dir_meta.is_dir() && dir_meta.uid() == my_uid && dir_meta.mode() & 0o777 == 0o700)
    };
    match check() {
        Ok(true) => Some(dir.to_path_buf()),
        Ok(false) => {
            tracing::warn!(dir = %dir.display(), "ssh control dir is not a private directory we own; connection sharing disabled");
            None
        }
        Err(error) => {
            tracing::warn!(dir = %dir.display(), %error, "cannot prepare ssh control dir; connection sharing disabled");
            None
        }
    }
}

#[cfg(not(unix))]
fn secure_control_dir(_dir: &std::path::Path) -> Option<PathBuf> {
    None
}

fn local_transport(
    platform: Platform,
    socket_path: &std::path::Path,
    session: &str,
) -> Arc<dyn Transport> {
    match platform {
        Platform::Mac => Arc::new(herdr_client::transport::unix::UnixSocketTransport::new(
            socket_path,
        )),
        Platform::Windows => {
            let pipe: Arc<dyn Transport> = windows_pipe_transport(&socket_path.to_string_lossy());
            let fallback: Arc<dyn Transport> = Arc::new(
                BridgeTransport::new(
                    launchers::local_herdr_bridge_command("herdr.exe", session),
                    "local herdr.exe bridge",
                )
                .without_ready_marker(),
            );
            Arc::new(FallbackTransport::new(pipe, fallback))
        }
    }
}

#[cfg(windows)]
fn windows_pipe_transport(socket_path: &str) -> Arc<dyn Transport> {
    Arc::new(herdr_client::transport::named_pipe::NamedPipeTransport::for_socket_path(socket_path))
}

#[cfg(not(windows))]
fn windows_pipe_transport(socket_path: &str) -> Arc<dyn Transport> {
    // Only reachable in tests that ask for Platform::Windows on a Mac.
    Arc::new(herdr_client::transport::unix::UnixSocketTransport::new(
        socket_path,
    ))
}

struct Running {
    spec: ServerSpec,
    source: Arc<dyn TransportSource>,
    handle: WatcherHandle,
}

pub struct Hub {
    factory: SourceFactory,
    watcher_cfg: WatcherConfig,
    running: HashMap<String, Running>,
    order: Arc<Mutex<Vec<String>>>,
    states: Arc<Mutex<HashMap<String, ServerState>>>,
    list_tx: watch::Sender<AgentList>,
    updates_tx: mpsc::Sender<ServerState>,
    merge_task: JoinHandle<()>,
}

impl Hub {
    pub fn new(factory: SourceFactory, watcher_cfg: WatcherConfig) -> Self {
        let (list_tx, _) = watch::channel(AgentList::default());
        let (updates_tx, mut updates_rx) = mpsc::channel::<ServerState>(256);
        let order = Arc::new(Mutex::new(Vec::new()));
        let states = Arc::new(Mutex::new(HashMap::new()));
        let merge_task = {
            let order = order.clone();
            let states = states.clone();
            let list_tx = list_tx.clone();
            tokio::spawn(async move {
                while let Some(state) = updates_rx.recv().await {
                    states.lock().unwrap().insert(state.id.clone(), state);
                    publish(&order, &states, &list_tx);
                }
            })
        };
        Self {
            factory,
            watcher_cfg,
            running: HashMap::new(),
            order,
            states,
            list_tx,
            updates_tx,
            merge_task,
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<AgentList> {
        self.list_tx.subscribe()
    }

    pub fn current(&self) -> AgentList {
        self.list_tx.borrow().clone()
    }

    /// Start new servers, stop removed or changed ones, keep the rest running.
    pub async fn apply_config(&mut self, cfg: &Config, platform: Platform) {
        let specs = specs_from_config(cfg, platform);
        let wanted: HashMap<&str, &ServerSpec> = specs.iter().map(|s| (s.id(), s)).collect();
        let watcher_cfg = self
            .watcher_cfg
            .clone()
            .with_reconcile_seconds(cfg.ui.reconcile_seconds);
        self.watcher_cfg = watcher_cfg.clone();

        let stale: Vec<String> = self
            .running
            .iter()
            .filter(|(id, r)| wanted.get(id.as_str()).is_none_or(|spec| **spec != r.spec))
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            if let Some(r) = self.running.remove(&id) {
                r.handle.shutdown().await;
            }
            self.states.lock().unwrap().remove(&id);
        }

        for spec in &specs {
            if self.running.contains_key(spec.id()) {
                continue;
            }
            let source = (self.factory)(spec);
            let state = ServerState::new(spec.id(), spec.id(), spec.kind());
            let handle = spawn_watcher(
                state,
                source.clone(),
                watcher_cfg.clone(),
                self.updates_tx.clone(),
            );
            self.running.insert(
                spec.id().to_string(),
                Running {
                    spec: spec.clone(),
                    source,
                    handle,
                },
            );
        }

        *self.order.lock().unwrap() = specs.iter().map(|s| s.id().to_string()).collect();
        publish(&self.order, &self.states, &self.list_tx);
    }

    pub fn reconnect_all(&self) {
        for r in self.running.values() {
            r.handle.reconnect();
        }
    }

    /// A one-shot client for quick actions, or `None` for an unknown server.
    pub async fn client_for(&self, server_id: &str) -> Option<Client> {
        let source = self.running.get(server_id)?.source.clone();
        match source.transport().await {
            Ok(transport) => Some(Client::new(transport)),
            Err(reason) => {
                tracing::warn!(server_id, %reason, "no transport for quick action");
                None
            }
        }
    }

    pub async fn shutdown(mut self) {
        for (_, r) in self.running.drain() {
            r.handle.shutdown().await;
        }
        self.merge_task.abort();
    }
}

fn publish(
    order: &Mutex<Vec<String>>,
    states: &Mutex<HashMap<String, ServerState>>,
    list_tx: &watch::Sender<AgentList>,
) {
    let order = order.lock().unwrap();
    let states = states.lock().unwrap();
    let ordered: Vec<ServerState> = order
        .iter()
        .filter_map(|id| states.get(id).cloned())
        .collect();
    let _ = list_tx.send(assemble(&ordered));
}
