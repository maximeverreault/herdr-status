//! How each server kind obtains its [`Transport`]. Remote and WSL sources run
//! first contact through a [`ProcessRunner`] and cache what they learn;
//! `invalidate` forgets it after a failure so the next attempt re-probes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use herdr_client::transport::bridge::BridgeTransport;
use herdr_client::transport::{Connection, Transport, TransportError};

use crate::config::RemoteConfig;
use crate::launchers::{self, SshOptions};
use crate::remote_connect::{
    bridge_transport_for, ProcessRunner, RemoteCache, RemoteCacheEntry, RemoteResolver,
};
use crate::remote_probe::parse_discovery_output;

const FIRST_CONTACT_TIMEOUT: Duration = Duration::from_secs(30);

#[async_trait]
pub trait TransportSource: Send + Sync {
    /// A transport for this server. `Err` carries user-facing reason text.
    async fn transport(&self) -> Result<Arc<dyn Transport>, String>;
    /// Forget cached discovery so the next `transport()` re-probes.
    fn invalidate(&self) {}
}

/// A transport that needs no discovery (local Unix socket, tests).
pub struct FixedSource(pub Arc<dyn Transport>);

#[async_trait]
impl TransportSource for FixedSource {
    async fn transport(&self) -> Result<Arc<dyn Transport>, String> {
        Ok(self.0.clone())
    }
}

/// Tries `primary`; on `NotRunning` tries `fallback` once and, if that works,
/// sticks with it. If the sticky fallback later fails, the choice is dropped
/// and `primary` is tried once more in the same call, so a herdr that is simply
/// not running reports `NotRunning` again rather than the fallback's error.
/// Used on Windows for named pipe → local `herdr.exe` bridge.
pub struct FallbackTransport {
    primary: Arc<dyn Transport>,
    fallback: Arc<dyn Transport>,
    use_fallback: AtomicBool,
}

impl FallbackTransport {
    pub fn new(primary: Arc<dyn Transport>, fallback: Arc<dyn Transport>) -> Self {
        Self {
            primary,
            fallback,
            use_fallback: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl Transport for FallbackTransport {
    async fn connect(&self) -> Result<Box<dyn Connection>, TransportError> {
        if self.use_fallback.load(Ordering::SeqCst) {
            match self.fallback.connect().await {
                Ok(conn) => return Ok(conn),
                Err(e) => {
                    tracing::info!(error = %e, primary = %self.primary.describe(), "fallback transport failed; trying primary again");
                    self.use_fallback.store(false, Ordering::SeqCst);
                    return self.primary.connect().await;
                }
            }
        }
        match self.primary.connect().await {
            Ok(conn) => Ok(conn),
            Err(TransportError::NotRunning(primary_reason)) => {
                match self.fallback.connect().await {
                    Ok(conn) => {
                        tracing::warn!(primary = %self.primary.describe(), fallback = %self.fallback.describe(), "primary transport unavailable; using fallback from now on");
                        self.use_fallback.store(true, Ordering::SeqCst);
                        Ok(conn)
                    }
                    Err(_) => Err(TransportError::NotRunning(primary_reason)),
                }
            }
            Err(other) => Err(other),
        }
    }

    fn describe(&self) -> String {
        if self.use_fallback.load(Ordering::SeqCst) {
            format!("{} (fallback)", self.fallback.describe())
        } else {
            self.primary.describe()
        }
    }
}

/// herdr inside WSL on this Windows machine, reached through `wsl.exe`.
pub struct WslSource {
    distro: Option<String>,
    session: String,
    runner: Arc<dyn ProcessRunner>,
    herdr_path: Mutex<Option<String>>,
}

impl WslSource {
    pub fn new(distro: Option<String>, session: String, runner: Arc<dyn ProcessRunner>) -> Self {
        Self {
            distro,
            session,
            runner,
            herdr_path: Mutex::new(None),
        }
    }

    fn label(&self) -> String {
        format!("wsl {}", self.distro.as_deref().unwrap_or("default"))
    }
}

#[async_trait]
impl TransportSource for WslSource {
    async fn transport(&self) -> Result<Arc<dyn Transport>, String> {
        let cached = self
            .herdr_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let path = match cached {
            Some(p) => p,
            None => {
                let command = launchers::wsl_command(
                    self.distro.as_deref(),
                    &launchers::posix_discovery_script(&self.session),
                );
                let out = self
                    .runner
                    .run(&command, FIRST_CONTACT_TIMEOUT)
                    .await
                    .map_err(|e| format!("wsl.exe failed: {e}"))?;
                let path = out
                    .success()
                    .then(|| parse_discovery_output(&out.stdout))
                    .flatten()
                    .ok_or_else(|| {
                        let msg = out.stderr.trim();
                        if msg.is_empty() {
                            "herdr not found in WSL".to_string()
                        } else {
                            msg.lines().last().unwrap_or(msg).to_string()
                        }
                    })?;
                *self
                    .herdr_path
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(path.clone());
                path
            }
        };
        let command = launchers::wsl_command(
            self.distro.as_deref(),
            &launchers::posix_bridge_script(&path, &self.session),
        );
        Ok(Arc::new(BridgeTransport::new(command, self.label())))
    }

    fn invalidate(&self) {
        *self
            .herdr_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// A remote herdr over SSH, with first contact cached in `cache.json`.
pub struct RemoteSource {
    cfg: RemoteConfig,
    runner: Arc<dyn ProcessRunner>,
    ssh: SshOptions,
    cache: Arc<Mutex<RemoteCache>>,
    cache_path: PathBuf,
    /// Serialises first contact so concurrent callers do not resolve twice.
    resolve_lock: tokio::sync::Mutex<()>,
    /// So a persistently unusable control dir logs once, not on every reconnect.
    control_dir_warned: AtomicBool,
}

impl RemoteSource {
    pub fn new(
        cfg: RemoteConfig,
        runner: Arc<dyn ProcessRunner>,
        ssh: SshOptions,
        cache: Arc<Mutex<RemoteCache>>,
        cache_path: PathBuf,
    ) -> Self {
        Self {
            cfg,
            runner,
            ssh,
            cache,
            cache_path,
            resolve_lock: tokio::sync::Mutex::new(()),
            control_dir_warned: AtomicBool::new(false),
        }
    }

    /// The configured SSH options with `control_dir` validated (and re-created if
    /// macOS cleaned `/tmp` since the last connection) or dropped.
    fn effective_ssh(&self) -> SshOptions {
        let mut ssh = self.ssh.clone();
        if let Some(dir) = ssh.control_dir.take() {
            ssh.control_dir = crate::paths::ensure_control_dir(&dir);
            if ssh.control_dir.is_none() && !self.control_dir_warned.swap(true, Ordering::Relaxed) {
                tracing::warn!(target = %self.cfg.ssh_target, "ssh connection sharing disabled for this remote");
            }
        }
        ssh
    }

    fn cached_entry(&self) -> Option<RemoteCacheEntry> {
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&self.cfg.ssh_target)
            .cloned()
    }

    fn save_cache(&self) {
        if let Err(e) = self
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .save(&self.cache_path)
        {
            tracing::warn!(error = %e, "could not save remote cache");
        }
    }
}

#[async_trait]
impl TransportSource for RemoteSource {
    async fn transport(&self) -> Result<Arc<dyn Transport>, String> {
        let ssh = self.effective_ssh();
        let entry = match self.cached_entry() {
            Some(entry) => entry,
            None => {
                let _resolving = self.resolve_lock.lock().await;
                // Another caller may have finished first contact while we waited.
                match self.cached_entry() {
                    Some(entry) => entry,
                    None => {
                        let resolver = RemoteResolver {
                            runner: self.runner.as_ref(),
                            ssh: ssh.clone(),
                            timeout: FIRST_CONTACT_TIMEOUT,
                        };
                        let entry = resolver
                            .resolve(&self.cfg)
                            .await
                            .map_err(|e| e.to_string())?;
                        self.cache
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert(&self.cfg.ssh_target, entry.clone());
                        self.save_cache();
                        entry
                    }
                }
            }
        };
        Ok(Arc::new(bridge_transport_for(&self.cfg, &entry, &ssh)))
    }

    fn invalidate(&self) {
        let removed = {
            let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
            let present = cache.get(&self.cfg.ssh_target).is_some();
            cache.remove(&self.cfg.ssh_target);
            present
        };
        if removed {
            self.save_cache();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Platform;
    use crate::remote_connect::{ProcessRunner, RemoteCache, RunOutput};
    use herdr_client::transport::bridge::ChildCommand;
    use herdr_client::transport::{Connection, StreamConnection, TransportError};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A transport that either fails with NotRunning or hands out a loopback connection.
    struct StubTransport {
        ok: AtomicBool,
        connects: AtomicUsize,
    }

    fn stub(ok: bool) -> Arc<StubTransport> {
        Arc::new(StubTransport {
            ok: AtomicBool::new(ok),
            connects: AtomicUsize::new(0),
        })
    }

    #[async_trait]
    impl Transport for StubTransport {
        async fn connect(&self) -> Result<Box<dyn Connection>, TransportError> {
            self.connects.fetch_add(1, Ordering::SeqCst);
            if self.ok.load(Ordering::SeqCst) {
                let (a, _b) = tokio::io::duplex(64);
                let (r, w) = tokio::io::split(a);
                Ok(Box::new(StreamConnection::new(r, w)))
            } else {
                Err(TransportError::NotRunning("stub".into()))
            }
        }
        fn describe(&self) -> String {
            if self.ok.load(Ordering::SeqCst) {
                "stub ok".into()
            } else {
                "stub down".into()
            }
        }
    }

    struct FakeRunner {
        outputs: Mutex<VecDeque<RunOutput>>,
        calls: Mutex<Vec<ChildCommand>>,
    }
    impl FakeRunner {
        fn new(outputs: Vec<RunOutput>) -> Arc<Self> {
            Arc::new(Self {
                outputs: Mutex::new(outputs.into()),
                calls: Mutex::new(Vec::new()),
            })
        }
        fn calls(&self) -> Vec<ChildCommand> {
            self.calls.lock().unwrap().clone()
        }
    }
    #[async_trait]
    impl ProcessRunner for FakeRunner {
        async fn run(
            &self,
            command: &ChildCommand,
            _timeout: Duration,
        ) -> std::io::Result<RunOutput> {
            self.calls.lock().unwrap().push(command.clone());
            // Yield so concurrent callers interleave as with a real child process.
            tokio::task::yield_now().await;
            Ok(self
                .outputs
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(RunOutput {
                    exit_code: Some(1),
                    stdout: String::new(),
                    stderr: "unscripted".into(),
                }))
        }
    }
    fn ok(stdout: &str) -> RunOutput {
        RunOutput {
            exit_code: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    #[tokio::test]
    async fn fixed_source_returns_the_same_transport() {
        let t: Arc<dyn Transport> = stub(true);
        let source = FixedSource(t.clone());
        assert_eq!(source.transport().await.unwrap().describe(), "stub ok");
    }

    #[tokio::test]
    async fn fallback_transport_switches_when_primary_is_not_running() {
        let primary = stub(false);
        let fallback = stub(true);
        let t = FallbackTransport::new(primary.clone(), fallback.clone());
        assert!(t.connect().await.is_ok());
        assert!(t.connect().await.is_ok());
        assert_eq!(
            primary.connects.load(Ordering::SeqCst),
            1,
            "primary tried once, then skipped"
        );
        assert_eq!(fallback.connects.load(Ordering::SeqCst), 2);
        assert_eq!(t.describe(), "stub ok (fallback)");
    }

    #[tokio::test]
    async fn fallback_transport_reports_primary_error_when_both_fail() {
        let primary = stub(false);
        let fallback = stub(false);
        let t = FallbackTransport::new(primary, fallback);
        assert!(matches!(
            t.connect().await,
            Err(TransportError::NotRunning(_))
        ));
        assert_eq!(t.describe(), "stub down");
    }

    #[tokio::test]
    async fn wsl_source_discovers_once_and_builds_bridge_command() {
        let runner = FakeRunner::new(vec![ok(
            "herdr-status-discovery:/home/me/.local/bin/herdr\n",
        )]);
        let source = WslSource::new(Some("Ubuntu".into()), "default".into(), runner.clone());
        let t = source.transport().await.unwrap();
        assert_eq!(t.describe(), "wsl Ubuntu");
        let _ = source.transport().await.unwrap();
        assert_eq!(runner.calls().len(), 1, "discovery result is cached");
        let call = &runner.calls()[0];
        assert_eq!(call.program, "wsl.exe");
        assert_eq!(&call.args[..4], &["-d", "Ubuntu", "--exec", "sh"]);
        source.invalidate();
        let _ = source.transport().await;
        assert_eq!(runner.calls().len(), 2);
    }

    #[tokio::test]
    async fn wsl_source_failure_is_a_reason_string() {
        let runner = FakeRunner::new(vec![RunOutput {
            exit_code: Some(2),
            stdout: String::new(),
            stderr: "herdr not found or too old on this host".into(),
        }]);
        let source = WslSource::new(None, "default".into(), runner);
        let Err(err) = source.transport().await else {
            panic!("expected discovery failure");
        };
        assert!(err.contains("herdr not found"), "{err}");
    }

    #[tokio::test]
    async fn remote_source_uses_cache_and_invalidates() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("cache.json");
        let runner = FakeRunner::new(vec![
            ok("Linux\nx86_64\n"),
            ok("herdr-status-discovery:/usr/local/bin/herdr\n"),
        ]);
        let cfg = RemoteConfig {
            label: "box".into(),
            ssh_target: "box".into(),
            session: "default".into(),
            os: None,
            herdr_path: None,
        };
        let cache = Arc::new(Mutex::new(RemoteCache::default()));
        let source = RemoteSource::new(
            cfg,
            runner.clone(),
            SshOptions {
                client: Platform::Mac,
                control_dir: None,
            },
            cache.clone(),
            cache_path.clone(),
        );
        let t = source.transport().await.unwrap();
        assert_eq!(t.describe(), "ssh box (posix)");
        assert_eq!(runner.calls().len(), 2);
        assert!(cache_path.exists(), "cache saved to disk");
        let _ = source.transport().await.unwrap();
        assert_eq!(runner.calls().len(), 2, "second call served from cache");
        source.invalidate();
        assert!(cache.lock().unwrap().get("box").is_none());
        assert!(
            source.transport().await.is_err(),
            "re-probe with no scripted output fails"
        );
    }

    #[tokio::test]
    async fn fallback_transport_retries_primary_when_sticky_fallback_fails() {
        let primary = stub(false);
        let fallback = stub(true);
        let t = FallbackTransport::new(primary.clone(), fallback.clone());
        assert!(t.connect().await.is_ok());
        assert_eq!(t.describe(), "stub ok (fallback)");
        fallback.ok.store(false, Ordering::SeqCst);
        assert!(matches!(
            t.connect().await,
            Err(TransportError::NotRunning(_))
        ));
        assert_eq!(
            primary.connects.load(Ordering::SeqCst),
            2,
            "primary retried in the same call"
        );
        assert_eq!(t.describe(), "stub down", "no longer sticky");
        primary.ok.store(true, Ordering::SeqCst);
        assert!(t.connect().await.is_ok());
        assert_eq!(
            fallback.connects.load(Ordering::SeqCst),
            2,
            "fallback not retried once primary works"
        );
    }

    fn remote_source(runner: Arc<FakeRunner>, cache_path: PathBuf) -> RemoteSource {
        let cfg = RemoteConfig {
            label: "box".into(),
            ssh_target: "box".into(),
            session: "default".into(),
            os: None,
            herdr_path: None,
        };
        RemoteSource::new(
            cfg,
            runner,
            SshOptions {
                client: Platform::Mac,
                control_dir: None,
            },
            Arc::new(Mutex::new(RemoteCache::default())),
            cache_path,
        )
    }

    #[tokio::test]
    async fn remote_source_concurrent_first_contact_resolves_once() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new(vec![
            ok("Linux\nx86_64\n"),
            ok("herdr-status-discovery:/usr/local/bin/herdr\n"),
        ]);
        let source = remote_source(runner.clone(), dir.path().join("cache.json"));
        let (a, b) = tokio::join!(source.transport(), source.transport());
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(runner.calls().len(), 2, "one resolve (uname + discovery)");
    }

    #[test]
    fn remote_invalidate_without_entry_does_not_write_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("cache.json");
        let source = remote_source(FakeRunner::new(vec![]), cache_path.clone());
        source.invalidate();
        assert!(!cache_path.exists());
    }
}
