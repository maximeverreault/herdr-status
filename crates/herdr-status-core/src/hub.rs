//! Owns one watcher per configured server, merges their states, and publishes
//! the sorted [`AgentList`] through a `watch` channel.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use herdr_client::transport::bridge::BridgeTransport;
use herdr_client::{Client, Transport};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::actions::ActionError;
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
///
/// The Windows fallback runs `herdr.exe` from `PATH`.
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
                // Validated (and re-created if /tmp was cleaned) by the source
                // on every connection, not here.
                let ssh = SshOptions {
                    client: platform,
                    control_dir: paths::ssh_control_dir(&env, platform),
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

fn local_transport(
    platform: Platform,
    socket_path: &std::path::Path,
    session: &str,
) -> Arc<dyn Transport> {
    match platform {
        Platform::Mac => unix_socket_transport(socket_path),
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
    unix_socket_transport(std::path::Path::new(socket_path))
}

#[cfg(unix)]
fn unix_socket_transport(path: &std::path::Path) -> Arc<dyn Transport> {
    Arc::new(herdr_client::transport::unix::UnixSocketTransport::new(
        path,
    ))
}

#[cfg(not(unix))]
fn unix_socket_transport(_path: &std::path::Path) -> Arc<dyn Transport> {
    Arc::new(NoUnixSockets)
}

/// Stand-in on targets without Unix sockets; only reachable when a Mac
/// configuration is built on such a target (never in production).
#[cfg(not(unix))]
struct NoUnixSockets;

#[cfg(not(unix))]
#[async_trait::async_trait]
impl Transport for NoUnixSockets {
    async fn connect(
        &self,
    ) -> Result<Box<dyn herdr_client::transport::Connection>, herdr_client::transport::TransportError>
    {
        Err(herdr_client::transport::TransportError::NotRunning(
            "unix sockets are not available on this OS".into(),
        ))
    }

    fn describe(&self) -> String {
        "unix sockets are not available on this OS".into()
    }
}

struct Running {
    spec: ServerSpec,
    source: Arc<dyn TransportSource>,
    handle: WatcherHandle,
    /// Tags the watcher's updates with its generation; aborted before the
    /// server's state is removed so nothing stale can land afterwards.
    forwarder: JoinHandle<()>,
}

/// Latest state per server id, with the generation of the watcher that owns it.
type States = Mutex<HashMap<String, (u64, ServerState)>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Hub {
    factory: SourceFactory,
    watcher_cfg: WatcherConfig,
    running: HashMap<String, Running>,
    next_generation: u64,
    order: Arc<Mutex<Vec<String>>>,
    states: Arc<States>,
    list_tx: watch::Sender<AgentList>,
    updates_tx: mpsc::Sender<(String, u64, ServerState)>,
    merge_task: JoinHandle<()>,
}

impl Hub {
    /// Spawns the merge task, so it must be called inside a tokio runtime.
    ///
    /// # Panics
    /// Panics outside a tokio runtime. The Tauri app must create the hub inside
    /// `tauri::async_runtime`.
    pub fn new(factory: SourceFactory, watcher_cfg: WatcherConfig) -> Self {
        let (list_tx, _) = watch::channel(AgentList::default());
        let (updates_tx, mut updates_rx) = mpsc::channel::<(String, u64, ServerState)>(256);
        let order = Arc::new(Mutex::new(Vec::new()));
        let states: Arc<States> = Arc::new(Mutex::new(HashMap::new()));
        let merge_task = {
            let order = order.clone();
            let states = states.clone();
            let list_tx = list_tx.clone();
            tokio::spawn(async move {
                while let Some((id, generation, state)) = updates_rx.recv().await {
                    {
                        let mut states = lock(&states);
                        match states.get_mut(&id) {
                            Some(slot) if slot.0 == generation => slot.1 = state,
                            _ => continue, // from a watcher that was stopped or replaced
                        }
                    }
                    publish(&order, &states, &list_tx);
                }
            })
        };
        Self {
            factory,
            watcher_cfg,
            running: HashMap::new(),
            next_generation: 0,
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
    ///
    /// Changing only `ui.reconcile_seconds` does not restart running watchers; it
    /// applies to watchers started afterwards.
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
        let mut stopping = Vec::new();
        for id in stale {
            if let Some(r) = self.running.remove(&id) {
                // Cut the update path first, then drop the state.
                r.forwarder.abort();
                lock(&self.states).remove(&id);
                stopping.push(r.handle);
            }
        }
        shutdown_all(stopping).await;

        *lock(&self.order) = specs.iter().map(|s| s.id().to_string()).collect();

        for spec in &specs {
            if self.running.contains_key(spec.id()) {
                continue;
            }
            self.next_generation += 1;
            let generation = self.next_generation;
            let source = (self.factory)(spec);
            let state = ServerState::new(spec.id(), spec.id(), spec.kind());
            lock(&self.states).insert(spec.id().to_string(), (generation, state.clone()));

            let (tx, mut rx) = mpsc::channel::<ServerState>(16);
            let merge_tx = self.updates_tx.clone();
            let id = spec.id().to_string();
            let forwarder = tokio::spawn(async move {
                while let Some(state) = rx.recv().await {
                    if merge_tx
                        .send((id.clone(), generation, state))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
            let handle = spawn_watcher(state, source.clone(), watcher_cfg.clone(), tx);
            self.running.insert(
                spec.id().to_string(),
                Running {
                    spec: spec.clone(),
                    source,
                    handle,
                    forwarder,
                },
            );
        }

        publish(&self.order, &self.states, &self.list_tx);
    }

    pub fn reconnect_all(&self) {
        for r in self.running.values() {
            r.handle.reconnect();
        }
    }

    /// The transport source of a running server, or `None` for an unknown id.
    /// Synchronous and cheap (clones an `Arc`), so the app can look it up under a
    /// lock and then run `transport()` and the action outside that lock.
    pub fn source_for(&self, server_id: &str) -> Option<Arc<dyn TransportSource>> {
        self.running.get(server_id).map(|r| r.source.clone())
    }

    /// A one-shot client for quick actions. The app should prefer [`Hub::source_for`]
    /// and call `transport()` plus the action outside any lock around the hub.
    pub async fn client_for(&self, server_id: &str) -> Result<Client, ActionError> {
        let source = self
            .source_for(server_id)
            .ok_or_else(|| ActionError::unknown_server(server_id))?;
        match source.transport().await {
            Ok(transport) => Ok(Client::new(transport)),
            Err(reason) => {
                tracing::warn!(server_id, %reason, "no transport for quick action");
                Err(ActionError::unreachable(server_id))
            }
        }
    }

    pub async fn shutdown(mut self) {
        let mut stopping = Vec::new();
        for (_, r) in self.running.drain() {
            r.forwarder.abort();
            stopping.push(r.handle);
        }
        shutdown_all(stopping).await;
        self.merge_task.abort();
    }
}

/// Stop watchers concurrently so one slow shutdown does not delay the rest.
async fn shutdown_all(handles: Vec<WatcherHandle>) {
    let mut set = tokio::task::JoinSet::new();
    for handle in handles {
        set.spawn(handle.shutdown());
    }
    while set.join_next().await.is_some() {}
}

fn publish(order: &Mutex<Vec<String>>, states: &States, list_tx: &watch::Sender<AgentList>) {
    let order = lock(order);
    let states = lock(states);
    let ordered: Vec<ServerState> = order
        .iter()
        .filter_map(|id| states.get(id).map(|(_, s)| s.clone()))
        .collect();
    let _ = list_tx.send(assemble(&ordered));
}
