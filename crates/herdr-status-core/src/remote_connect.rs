//! First contact with a remote host: detect its OS, find a herdr that supports
//! the API bridge, and cache the answer. All process execution goes through
//! [`ProcessRunner`] so this is testable without SSH.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use herdr_client::transport::bridge::{BridgeTransport, ChildCommand};
use serde::{Deserialize, Serialize};

use crate::config::{RemoteConfig, RemoteOs};
use crate::launchers::{self, SshOptions};
use crate::remote_probe::{
    classify_ssh_failure, looks_like_windows_uname, parse_discovery_output, parse_uname,
    parse_windows_probe, should_try_windows_probe, RemotePlatform, SshFailure,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl RunOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Runs a command to completion. Implemented by [`TokioRunner`] in the app and
/// by fakes in tests.
#[async_trait]
pub trait ProcessRunner: Send + Sync {
    async fn run(&self, command: &ChildCommand, timeout: Duration) -> std::io::Result<RunOutput>;
}

pub struct TokioRunner;

#[async_trait]
impl ProcessRunner for TokioRunner {
    async fn run(&self, command: &ChildCommand, timeout: Duration) -> std::io::Result<RunOutput> {
        let mut cmd = tokio::process::Command::new(&command.program);
        cmd.args(&command.args)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("{} timed out after {timeout:?}", command.program),
                )
            })??;
        Ok(RunOutput {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteCacheEntry {
    pub os: RemoteOs,
    pub arch: String,
    pub herdr_path: String,
    /// Unix seconds.
    pub checked_at: u64,
}

/// `cache.json`: one entry per `ssh_target`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RemoteCache {
    #[serde(default)]
    pub entries: HashMap<String, RemoteCacheEntry>,
}

impl RemoteCache {
    /// A missing or unreadable file is an empty cache (logged, never fatal).
    pub fn load(path: &Path) -> RemoteCache {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), error = %e, "ignoring corrupt remote cache");
                RemoteCache::default()
            }),
            Err(_) => RemoteCache::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            path,
            serde_json::to_string_pretty(self).expect("cache serialises"),
        )
    }

    pub fn get(&self, ssh_target: &str) -> Option<&RemoteCacheEntry> {
        self.entries.get(ssh_target)
    }

    pub fn insert(&mut self, ssh_target: &str, entry: RemoteCacheEntry) {
        self.entries.insert(ssh_target.to_string(), entry);
    }

    pub fn remove(&mut self, ssh_target: &str) {
        self.entries.remove(ssh_target);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("{}", .0.reason(.1))]
    Ssh(SshFailure, String),
    #[error("herdr not found or too old on {0}")]
    HerdrNotFound(String),
    #[error("unsupported remote platform: {0}")]
    UnsupportedPlatform(String),
    #[error("{0}")]
    Io(String),
}

pub struct RemoteResolver<'a> {
    pub runner: &'a dyn ProcessRunner,
    pub ssh: SshOptions,
    pub timeout: Duration,
}

impl<'a> RemoteResolver<'a> {
    /// Platform detection, then herdr discovery, honouring configured overrides.
    pub async fn resolve(&self, cfg: &RemoteConfig) -> Result<RemoteCacheEntry, RemoteError> {
        let platform = match &cfg.os {
            Some(os) => RemotePlatform {
                os: os.clone(),
                arch: "configured".to_string(),
            },
            None => self.detect_platform(cfg).await?,
        };
        let herdr_path = match &cfg.herdr_path {
            Some(path) => path.clone(),
            None => self.discover(cfg, &platform.os).await?,
        };
        Ok(RemoteCacheEntry {
            os: platform.os,
            arch: platform.arch,
            herdr_path,
            checked_at: now_secs(),
        })
    }

    async fn ssh(
        &self,
        cfg: &RemoteConfig,
        remote_command: &str,
    ) -> Result<RunOutput, RemoteError> {
        let command = launchers::ssh_command(&cfg.ssh_target, remote_command, &self.ssh);
        self.runner
            .run(&command, self.timeout)
            .await
            .map_err(|e| RemoteError::Io(e.to_string()))
    }

    async fn detect_platform(&self, cfg: &RemoteConfig) -> Result<RemotePlatform, RemoteError> {
        let out = self.ssh(cfg, "uname -s; uname -m").await?;
        if out.success() {
            if let Some(platform) = parse_uname(&out.stdout) {
                return Ok(platform);
            }
        }
        let hint =
            out.success() && looks_like_windows_uname(out.stdout.lines().next().unwrap_or(""));
        if should_try_windows_probe(out.success(), out.exit_code, hint) {
            let probe_command =
                launchers::powershell_encoded_command(&launchers::windows_platform_probe_script());
            let probe = self.ssh(cfg, &probe_command).await?;
            if probe.success() {
                if let Some(platform) = parse_windows_probe(&probe.stdout) {
                    return Ok(platform);
                }
            }
        }
        if !out.success() && (out.exit_code == Some(255) || !out.stderr.trim().is_empty()) {
            return Err(RemoteError::Ssh(
                classify_ssh_failure(out.exit_code, &out.stderr),
                cfg.ssh_target.clone(),
            ));
        }
        Err(RemoteError::UnsupportedPlatform(
            out.stdout.trim().to_string(),
        ))
    }

    async fn discover(&self, cfg: &RemoteConfig, os: &RemoteOs) -> Result<String, RemoteError> {
        let remote_command = match os {
            RemoteOs::Posix => {
                launchers::posix_sh_command(&launchers::posix_discovery_script(&cfg.session))
            }
            RemoteOs::Windows => launchers::powershell_encoded_command(
                &launchers::windows_discovery_script(&cfg.session),
            ),
        };
        let out = self.ssh(cfg, &remote_command).await?;
        if out.success() {
            if let Some(path) = parse_discovery_output(&out.stdout) {
                return Ok(path);
            }
        }
        if out.exit_code == Some(2) {
            return Err(RemoteError::HerdrNotFound(cfg.ssh_target.clone()));
        }
        Err(RemoteError::Ssh(
            classify_ssh_failure(out.exit_code, &out.stderr),
            cfg.ssh_target.clone(),
        ))
    }
}

/// The long-lived bridge transport for a resolved remote.
pub fn bridge_transport_for(
    cfg: &RemoteConfig,
    entry: &RemoteCacheEntry,
    ssh: &SshOptions,
) -> BridgeTransport {
    let command = launchers::remote_bridge_command(
        &cfg.ssh_target,
        entry.os.clone(),
        &entry.herdr_path,
        &cfg.session,
        ssh,
    );
    let os = match entry.os {
        RemoteOs::Posix => "posix",
        RemoteOs::Windows => "windows",
    };
    BridgeTransport::new(command, format!("ssh {} ({os})", cfg.ssh_target))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Platform;
    use herdr_client::Transport;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Returns scripted outputs in order and records every command.
    struct FakeRunner {
        outputs: Mutex<VecDeque<RunOutput>>,
        calls: Mutex<Vec<ChildCommand>>,
    }

    impl FakeRunner {
        fn new(outputs: Vec<RunOutput>) -> Self {
            Self {
                outputs: Mutex::new(outputs.into()),
                calls: Mutex::new(Vec::new()),
            }
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
            Ok(self
                .outputs
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(RunOutput {
                    exit_code: Some(1),
                    stdout: String::new(),
                    stderr: "unscripted call".into(),
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
    fn fail(code: i32, stderr: &str) -> RunOutput {
        RunOutput {
            exit_code: Some(code),
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
    fn cfg(target: &str) -> RemoteConfig {
        RemoteConfig {
            label: target.into(),
            ssh_target: target.into(),
            session: "default".into(),
            os: None,
            herdr_path: None,
        }
    }
    fn resolver(runner: &FakeRunner) -> RemoteResolver<'_> {
        RemoteResolver {
            runner,
            ssh: SshOptions {
                client: Platform::Mac,
                control_dir: None,
            },
            timeout: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn linux_host_resolves_posix_and_discovers_path() {
        let runner = FakeRunner::new(vec![
            ok("Linux\nx86_64\n"),
            ok("herdr-status-discovery:/usr/local/bin/herdr\n"),
        ]);
        let entry = resolver(&runner).resolve(&cfg("box")).await.unwrap();
        assert_eq!(entry.os, RemoteOs::Posix);
        assert_eq!(entry.arch, "x86_64");
        assert_eq!(entry.herdr_path, "/usr/local/bin/herdr");
        let calls = runner.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].program, "ssh");
        assert!(calls[0].args.contains(&"BatchMode=yes".to_string()));
        assert_eq!(calls[0].args.last().unwrap(), "uname -s; uname -m");
        assert!(calls[1].args.last().unwrap().starts_with("/bin/sh -c "));
    }

    #[tokio::test]
    async fn windows_host_falls_back_to_powershell_probe() {
        let runner = FakeRunner::new(vec![
            fail(
                1,
                "'uname' is not recognized as an internal or external command",
            ),
            ok("\r\nherdr-windows:AMD64\r\n"),
            ok(r"herdr-status-discovery:C:\Users\me\AppData\Local\Programs\Herdr\bin\herdr.exe"),
        ]);
        let entry = resolver(&runner).resolve(&cfg("winbox")).await.unwrap();
        assert_eq!(entry.os, RemoteOs::Windows);
        assert_eq!(entry.arch, "AMD64");
        assert!(entry.herdr_path.ends_with("herdr.exe"));
        let calls = runner.calls();
        assert!(calls[1]
            .args
            .last()
            .unwrap()
            .starts_with("powershell.exe -NoLogo"));
        assert!(calls[2]
            .args
            .last()
            .unwrap()
            .starts_with("powershell.exe -NoLogo"));
    }

    #[tokio::test]
    async fn auth_denied_is_an_ssh_error_with_hint() {
        let runner = FakeRunner::new(vec![fail(255, "me@box: Permission denied (publickey).")]);
        let err = resolver(&runner).resolve(&cfg("box")).await.unwrap_err();
        assert!(
            matches!(err, RemoteError::Ssh(SshFailure::AuthDenied, ref t) if t == "box"),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "SSH authentication failed; run `ssh box` once in a terminal"
        );
        assert_eq!(
            runner.calls().len(),
            1,
            "no probe after an ssh-level failure"
        );
    }

    #[tokio::test]
    async fn missing_herdr_is_reported() {
        let runner = FakeRunner::new(vec![
            ok("Linux\naarch64\n"),
            fail(2, "herdr not found or too old on this host"),
        ]);
        let err = resolver(&runner).resolve(&cfg("box")).await.unwrap_err();
        assert!(
            matches!(err, RemoteError::HerdrNotFound(ref t) if t == "box"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn configured_os_and_path_skip_all_probing() {
        let runner = FakeRunner::new(vec![]);
        let mut c = cfg("winbox");
        c.os = Some(RemoteOs::Windows);
        c.herdr_path = Some("herdr.exe".into());
        let entry = resolver(&runner).resolve(&c).await.unwrap();
        assert_eq!(entry.os, RemoteOs::Windows);
        assert_eq!(entry.herdr_path, "herdr.exe");
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn cache_round_trips_and_tolerates_missing_or_corrupt_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        assert!(RemoteCache::load(&path).entries.is_empty());
        let mut cache = RemoteCache::default();
        cache.insert(
            "box",
            RemoteCacheEntry {
                os: RemoteOs::Posix,
                arch: "x86_64".into(),
                herdr_path: "/usr/local/bin/herdr".into(),
                checked_at: 42,
            },
        );
        cache.save(&path).unwrap();
        let loaded = RemoteCache::load(&path);
        assert_eq!(
            loaded.get("box").unwrap().herdr_path,
            "/usr/local/bin/herdr"
        );
        std::fs::write(&path, "{ not json").unwrap();
        assert!(RemoteCache::load(&path).entries.is_empty());
    }

    #[test]
    fn bridge_transport_describes_target_and_os() {
        let entry = RemoteCacheEntry {
            os: RemoteOs::Windows,
            arch: "AMD64".into(),
            herdr_path: "herdr.exe".into(),
            checked_at: 0,
        };
        let t = bridge_transport_for(
            &cfg("winbox"),
            &entry,
            &SshOptions {
                client: Platform::Mac,
                control_dir: None,
            },
        );
        assert_eq!(t.describe(), "ssh winbox (windows)");
        assert_eq!(t.command().program, "ssh");
    }
}
