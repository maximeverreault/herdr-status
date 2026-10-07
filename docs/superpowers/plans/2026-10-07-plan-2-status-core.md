# herdr-status-core Implementation Plan (Plan 2 of 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust crate that turns a config file into live, urgency-sorted agent lists for local, WSL and SSH-remote herdr servers, and exposes the three quick actions.

**Architecture:** `herdr-status-core` sits on `herdr-client` (Plan 1). `config` and `paths` are pure parsing. `launchers` builds the exact child commands (ssh, wsl.exe, PowerShell) and `remote_probe` parses their output; `remote_connect` runs first contact through a `ProcessRunner` trait so it is testable without SSH. `model` + `reducer` are pure state. `watcher` is one task per server implementing the subscribe/snapshot/rebuild loop; `hub` owns watchers and publishes a merged `AgentList`. `actions` wraps the quick actions. Plan 3 (the Tauri app) consumes only `Hub`, `AgentList`, `Config` and `actions`.

**Tech Stack:** Rust 2021, tokio, serde + serde_json, toml 0.8, thiserror, tracing, base64 0.22, async-trait. Tests use `herdr-client`'s `test-support` fake server.

**Spec:** `docs/superpowers/specs/2026-10-07-herdr-status-bar-design.md`, sections 7 to 10, 13, 14. Plan 1 must be complete first; this plan uses its public names exactly (`Client`, `Method`, `Subscription`, `Event`, `AgentStatus`, `SessionSnapshot`, `AgentInfo`, `Transport`, `BridgeTransport`, `ChildCommand`, `READY_MARKER`, `UnixSocketTransport`, `NamedPipeTransport`, `test_support::FakeHerdrServer`).

**Conventions:** commands run from the repo root. Unit tests live in `#[cfg(test)] mod tests` at the bottom of each file unless a `tests/` file is named. Commit after each task.

---

## File structure

| File | Responsibility |
|---|---|
| `crates/herdr-status-core/Cargo.toml` | Manifest |
| `src/lib.rs` | Module list, re-exports |
| `src/config.rs` | `Config` and sections, TOML parsing, validation, default file text |
| `src/paths.rs` | herdr socket path rules and the bar's own directories, driven by an injectable `Env` |
| `src/launchers.rs` | Quoting helpers and command builders: POSIX bridge/discovery scripts, PowerShell scripts and `-EncodedCommand`, `ssh`, `wsl.exe`, local `herdr.exe` |
| `src/remote_probe.rs` | Parsers for `uname`, the PowerShell probe and discovery output; SSH failure classification |
| `src/remote_connect.rs` | `ProcessRunner` trait, `TokioRunner`, `RemoteResolver` (platform → discovery → cache entry), `RemoteCache` |
| `src/model.rs` | `AgentRow`, `ServerState`, `ConnectionState`, `AgentList`, `assemble` |
| `src/reducer.rs` | `apply_snapshot`, `apply_status`, `display_name` |
| `src/watcher.rs` | Per-server task: subscribe, snapshot, rebuild, reconcile, backoff |
| `src/transport_source.rs` | `TransportSource` trait with local, WSL and remote implementations |
| `src/hub.rs` | Owns watchers from a `Config`, merges states, publishes `AgentList` |
| `src/actions.rs` | peek / send_key / send_message with key allow-list and friendly errors |
| `src/prefill.rs` | First-launch config prefill from `herdr machine list --json` and the WSL probe |

---

### Task 1: Crate scaffolding

**Files:**
- Modify: `Cargo.toml` (workspace members and deps)
- Create: `crates/herdr-status-core/Cargo.toml`
- Create: `crates/herdr-status-core/src/lib.rs`

- [ ] **Step 1: Add the crate to the workspace**

In the root `Cargo.toml` change `members` and add dependencies:

```toml
[workspace]
resolver = "2"
members = ["crates/herdr-client", "crates/herdr-status-core"]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"

[workspace.dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "net", "io-util", "process", "time", "macros", "sync"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tracing = "0.1"
async-trait = "0.1"
toml = "0.8"
base64 = "0.22"
```

- [ ] **Step 2: Create the crate manifest**

`crates/herdr-status-core/Cargo.toml`:

```toml
[package]
name = "herdr-status-core"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "State model, config and watchers for the herdr-status bar"

[dependencies]
herdr-client = { path = "../herdr-client" }
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tracing.workspace = true
async-trait.workspace = true
toml.workspace = true
base64.workspace = true

[dev-dependencies]
herdr-client = { path = "../herdr-client", features = ["test-support"] }
tempfile = "3"
```

- [ ] **Step 3: Create the library root**

`crates/herdr-status-core/src/lib.rs`:

```rust
//! Core of herdr-status: configuration, per-server watchers and the sorted
//! agent list the UI renders. No UI code lives here.
```

- [ ] **Step 4: Verify the workspace builds**

Run: `cargo build --workspace`
Expected: `Finished`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/herdr-status-core
git commit -m "chore: scaffold herdr-status-core crate"
```

---

### Task 2: Configuration parsing and validation

**Files:**
- Create: `crates/herdr-status-core/src/config.rs`
- Modify: `crates/herdr-status-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

`crates/herdr-status-core/src/config.rs` (tests first):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_gives_defaults() {
        let cfg = Config::parse("").unwrap();
        assert!(cfg.local.enabled);
        assert_eq!(cfg.local.session, "default");
        assert_eq!(cfg.local.socket_path, None);
        assert!(cfg.wsl.is_empty());
        assert!(cfg.remote.is_empty());
        assert_eq!(cfg.ui.peek_lines, 30);
        assert_eq!(cfg.ui.reconcile_seconds, 30);
    }

    #[test]
    fn default_toml_text_parses_to_defaults() {
        let cfg = Config::parse(Config::DEFAULT_TOML).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn parses_every_section() {
        let text = r#"
[local]
enabled = false
session = "work"
socket_path = "/tmp/custom.sock"

[[wsl]]
label = "ubuntu"
distro = "Ubuntu"

[[remote]]
label = "winbox"
ssh_target = "me@winbox"
session = "agents"
os = "windows"
herdr_path = "C:\\Users\\me\\AppData\\Local\\Programs\\Herdr\\bin\\herdr.exe"

[ui]
peek_lines = 50
reconcile_seconds = 10
"#;
        let cfg = Config::parse(text).unwrap();
        assert!(!cfg.local.enabled);
        assert_eq!(cfg.local.session, "work");
        assert_eq!(cfg.local.socket_path.as_deref(), Some(std::path::Path::new("/tmp/custom.sock")));
        assert_eq!(cfg.wsl[0].label, "ubuntu");
        assert_eq!(cfg.wsl[0].distro.as_deref(), Some("Ubuntu"));
        assert_eq!(cfg.wsl[0].session, "default");
        assert_eq!(cfg.remote[0].label, "winbox");
        assert_eq!(cfg.remote[0].ssh_target, "me@winbox");
        assert_eq!(cfg.remote[0].session, "agents");
        assert_eq!(cfg.remote[0].os, Some(RemoteOs::Windows));
        assert!(cfg.remote[0].herdr_path.as_deref().unwrap().ends_with("herdr.exe"));
        assert_eq!(cfg.ui.peek_lines, 50);
        assert_eq!(cfg.ui.reconcile_seconds, 10);
    }

    #[test]
    fn server_ids_are_local_then_wsl_then_remote_labels() {
        let text = "[[wsl]]\nlabel = \"ubuntu\"\n[[remote]]\nlabel = \"winbox\"\nssh_target = \"winbox\"\n";
        let cfg = Config::parse(text).unwrap();
        assert_eq!(cfg.server_ids(), vec!["local", "ubuntu", "winbox"]);
        let disabled = Config::parse("[local]\nenabled = false\n").unwrap();
        assert_eq!(disabled.server_ids(), Vec::<String>::new());
    }

    #[test]
    fn rejects_duplicate_labels() {
        let text = "[[wsl]]\nlabel = \"box\"\n[[remote]]\nlabel = \"box\"\nssh_target = \"box\"\n";
        let err = Config::parse(text).unwrap_err();
        assert!(matches!(err, ConfigError::DuplicateLabel(l) if l == "box"), "{err}");
    }

    #[test]
    fn rejects_reserved_and_empty_labels() {
        let reserved = "[[remote]]\nlabel = \"local\"\nssh_target = \"x\"\n";
        assert!(matches!(Config::parse(reserved).unwrap_err(), ConfigError::ReservedLabel(_)));
        let empty = "[[remote]]\nlabel = \"\"\nssh_target = \"x\"\n";
        assert!(matches!(Config::parse(empty).unwrap_err(), ConfigError::EmptyLabel));
    }

    #[test]
    fn reports_unknown_keys_and_syntax_errors_with_position() {
        let unknown = "[ui]\npeek_lnes = 3\n";
        let err = Config::parse(unknown).unwrap_err();
        match err {
            ConfigError::Toml { message, line } => {
                assert!(message.contains("peek_lnes"), "{message}");
                assert_eq!(line, Some(2));
            }
            other => panic!("unexpected {other:?}"),
        }
        let broken = "[local\nenabled = true\n";
        assert!(matches!(Config::parse(broken).unwrap_err(), ConfigError::Toml { line: Some(1), .. }));
    }
}
```

Add to `lib.rs`:

```rust
pub mod config;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-status-core config`
Expected: compile error, `Config` not found.

- [ ] **Step 3: Implement the config types**

Put this above the tests in `config.rs`:

```rust
//! `config.toml`: which herdr servers to watch and a few UI knobs.
//! Unknown keys are errors so typos never silently do nothing.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const LOCAL_SERVER_ID: &str = "local";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteOs {
    Posix,
    Windows,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_session")]
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WslConfig {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distro: Option<String>,
    #[serde(default = "default_session")]
    pub session: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteConfig {
    pub label: String,
    pub ssh_target: String,
    #[serde(default = "default_session")]
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<RemoteOs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiConfig {
    #[serde(default = "default_peek_lines")]
    pub peek_lines: u32,
    #[serde(default = "default_reconcile_seconds")]
    pub reconcile_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub local: LocalConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wsl: Vec<WslConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote: Vec<RemoteConfig>,
    #[serde(default)]
    pub ui: UiConfig,
}

fn default_true() -> bool {
    true
}
fn default_session() -> String {
    "default".to_string()
}
fn default_peek_lines() -> u32 {
    30
}
fn default_reconcile_seconds() -> u64 {
    30
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self { enabled: true, session: default_session(), socket_path: None }
    }
}
impl Default for UiConfig {
    fn default() -> Self {
        Self { peek_lines: default_peek_lines(), reconcile_seconds: default_reconcile_seconds() }
    }
}
impl Default for Config {
    fn default() -> Self {
        Self { local: LocalConfig::default(), wsl: Vec::new(), remote: Vec::new(), ui: UiConfig::default() }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config.toml line {}: {message}", .line.map(|l| l.to_string()).unwrap_or_else(|| "?".into()))]
    Toml { message: String, line: Option<usize> },
    #[error("duplicate server label '{0}'")]
    DuplicateLabel(String),
    #[error("'{0}' is reserved for the local server")]
    ReservedLabel(String),
    #[error("a server label is empty")]
    EmptyLabel,
}

impl Config {
    /// The file written on first launch: every key, commented with its default.
    pub const DEFAULT_TOML: &'static str = r#"# herdr-status configuration. Saved changes are applied live.

[local]
enabled = true
session = "default"             # named session -> <herdr config>/sessions/<name>/herdr.sock
# socket_path = "/custom/herdr.sock"   # overrides session

# [[wsl]]                       # Windows only; herdr running inside WSL on this machine
# label = "wsl"
# distro = "Ubuntu"             # default distro when omitted
# session = "default"

# [[remote]]
# label = "winbox"
# ssh_target = "winbox"         # any ssh target: alias, user@host, ssh://user@host:2222
# session = "default"
# os = "windows"                # "posix" | "windows"; auto-detected and cached when omitted
# herdr_path = "C:\\Users\\me\\AppData\\Local\\Programs\\Herdr\\bin\\herdr.exe"

[ui]
peek_lines = 30
reconcile_seconds = 30
"#;

    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let cfg: Config = toml::from_str(text).map_err(|e| ConfigError::Toml {
            message: e.message().to_string(),
            line: e.span().map(|span| text[..span.start].matches('\n').count() + 1),
        })?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut seen = std::collections::HashSet::new();
        let labels = self.wsl.iter().map(|w| &w.label).chain(self.remote.iter().map(|r| &r.label));
        for label in labels {
            if label.trim().is_empty() {
                return Err(ConfigError::EmptyLabel);
            }
            if label == LOCAL_SERVER_ID {
                return Err(ConfigError::ReservedLabel(label.clone()));
            }
            if !seen.insert(label.clone()) {
                return Err(ConfigError::DuplicateLabel(label.clone()));
            }
        }
        Ok(())
    }

    /// Server ids in display order: local (if enabled), then WSL, then remotes.
    pub fn server_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        if self.local.enabled {
            ids.push(LOCAL_SERVER_ID.to_string());
        }
        ids.extend(self.wsl.iter().map(|w| w.label.clone()));
        ids.extend(self.remote.iter().map(|r| r.label.clone()));
        ids
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config serialises")
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-status-core config`
Expected: `7 passed`. If `reports_unknown_keys_and_syntax_errors_with_position` fails on the line number, print `e.span()` for both inputs and adjust the expected lines to what `toml` 0.8 reports; the requirement is only that a line is reported.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-status-core/src/config.rs crates/herdr-status-core/src/lib.rs
git commit -m "feat(core): config parsing, validation and default file text"
```

---

### Task 3: Path rules (herdr socket, bar directories)

**Files:**
- Create: `crates/herdr-status-core/src/paths.rs`
- Modify: `crates/herdr-status-core/src/lib.rs`

All functions take an `Env` (a map of environment variables) and a `Platform` so they are tested on the Mac for both operating systems.

- [ ] **Step 1: Write the failing tests**

`crates/herdr-status-core/src/paths.rs` (tests first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_pairs(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())))
    }

    #[test]
    fn herdr_config_dir_mac_defaults_to_dot_config() {
        let e = env(&[("HOME", "/Users/me")]);
        assert_eq!(herdr_config_dir(&e, Platform::Mac), PathBuf::from("/Users/me/.config/herdr"));
    }

    #[test]
    fn herdr_config_dir_honours_xdg_on_both_platforms() {
        let e = env(&[("HOME", "/Users/me"), ("XDG_CONFIG_HOME", "/xdg")]);
        assert_eq!(herdr_config_dir(&e, Platform::Mac), PathBuf::from("/xdg/herdr"));
        let w = env(&[("APPDATA", r"C:\Users\me\AppData\Roaming"), ("XDG_CONFIG_HOME", r"D:\xdg")]);
        assert_eq!(herdr_config_dir(&w, Platform::Windows), PathBuf::from(r"D:\xdg\herdr"));
    }

    #[test]
    fn herdr_config_dir_windows_uses_appdata_then_userprofile() {
        let e = env(&[("APPDATA", r"C:\Users\me\AppData\Roaming")]);
        assert_eq!(herdr_config_dir(&e, Platform::Windows), PathBuf::from(r"C:\Users\me\AppData\Roaming\herdr"));
        let e = env(&[("USERPROFILE", r"C:\Users\me")]);
        assert_eq!(herdr_config_dir(&e, Platform::Windows), PathBuf::from(r"C:\Users\me\AppData\Roaming\herdr"));
    }

    #[test]
    fn socket_path_follows_session_and_override_rules() {
        let e = env(&[("HOME", "/Users/me")]);
        assert_eq!(herdr_socket_path(&e, Platform::Mac, "default"), PathBuf::from("/Users/me/.config/herdr/herdr.sock"));
        assert_eq!(herdr_socket_path(&e, Platform::Mac, "work"), PathBuf::from("/Users/me/.config/herdr/sessions/work/herdr.sock"));
        let o = env(&[("HOME", "/Users/me"), ("HERDR_SOCKET_PATH", "/tmp/x.sock")]);
        assert_eq!(herdr_socket_path(&o, Platform::Mac, "work"), PathBuf::from("/tmp/x.sock"));
        let w = env(&[("APPDATA", r"C:\Users\me\AppData\Roaming")]);
        assert_eq!(herdr_socket_path(&w, Platform::Windows, "default").to_string_lossy(), r"C:\Users\me\AppData\Roaming\herdr\herdr.sock");
    }

    #[test]
    fn bar_directories() {
        let e = env(&[("HOME", "/Users/me")]);
        assert_eq!(app_config_file(&e, Platform::Mac), PathBuf::from("/Users/me/.config/herdr-status/config.toml"));
        assert_eq!(app_data_dir(&e, Platform::Mac), PathBuf::from("/Users/me/Library/Application Support/herdr-status"));
        assert_eq!(app_log_dir(&e, Platform::Mac), PathBuf::from("/Users/me/Library/Logs/herdr-status"));
        let w = env(&[("APPDATA", r"C:\Users\me\AppData\Roaming"), ("LOCALAPPDATA", r"C:\Users\me\AppData\Local")]);
        assert_eq!(app_config_file(&w, Platform::Windows), PathBuf::from(r"C:\Users\me\AppData\Roaming\herdr-status\config.toml"));
        assert_eq!(app_data_dir(&w, Platform::Windows), PathBuf::from(r"C:\Users\me\AppData\Local\herdr-status"));
        assert_eq!(app_log_dir(&w, Platform::Windows), PathBuf::from(r"C:\Users\me\AppData\Local\herdr-status\logs"));
    }

    #[test]
    fn posix_socket_path_string_for_wsl_probe() {
        assert_eq!(posix_socket_probe_path("default"), "${XDG_CONFIG_HOME:-$HOME/.config}/herdr/herdr.sock");
        assert_eq!(posix_socket_probe_path("work"), "${XDG_CONFIG_HOME:-$HOME/.config}/herdr/sessions/work/herdr.sock");
    }
}
```

Add to `lib.rs`:

```rust
pub mod paths;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-status-core paths`
Expected: compile error.

- [ ] **Step 3: Implement the path rules**

Above the tests in `paths.rs`:

```rust
//! Where things live: herdr's socket (mirrors herdr's own `config_dir()` rules)
//! and the bar's config, data and log directories.

use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Mac
        }
    }
}

/// A snapshot of environment variables; injectable for tests.
#[derive(Debug, Clone, Default)]
pub struct Env {
    vars: HashMap<String, String>,
}

impl Env {
    pub fn from_process() -> Self {
        Self { vars: std::env::vars().collect() }
    }

    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        Self { vars: pairs.into_iter().collect() }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str).filter(|v| !v.is_empty())
    }
}

const HERDR_DIR: &str = "herdr";
const APP_DIR: &str = "herdr-status";

/// herdr's config directory: `$XDG_CONFIG_HOME/herdr`, else `~/.config/herdr`
/// on macOS, `%APPDATA%\herdr` (then `%USERPROFILE%\AppData\Roaming\herdr`) on Windows.
pub fn herdr_config_dir(env: &Env, platform: Platform) -> PathBuf {
    if let Some(xdg) = env.get("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join(HERDR_DIR);
    }
    match platform {
        Platform::Mac => home(env).join(".config").join(HERDR_DIR),
        Platform::Windows => roaming(env).join(HERDR_DIR),
    }
}

/// The socket path herdr would use for `session`, honouring `HERDR_SOCKET_PATH`.
pub fn herdr_socket_path(env: &Env, platform: Platform, session: &str) -> PathBuf {
    if let Some(p) = env.get("HERDR_SOCKET_PATH") {
        return PathBuf::from(p);
    }
    let dir = herdr_config_dir(env, platform);
    if session == "default" {
        dir.join("herdr.sock")
    } else {
        dir.join("sessions").join(session).join("herdr.sock")
    }
}

/// The same rule as a POSIX shell expression, for probing inside WSL.
pub fn posix_socket_probe_path(session: &str) -> String {
    if session == "default" {
        "${XDG_CONFIG_HOME:-$HOME/.config}/herdr/herdr.sock".to_string()
    } else {
        format!("${{XDG_CONFIG_HOME:-$HOME/.config}}/herdr/sessions/{session}/herdr.sock")
    }
}

pub fn app_config_file(env: &Env, platform: Platform) -> PathBuf {
    match platform {
        Platform::Mac => home(env).join(".config").join(APP_DIR).join("config.toml"),
        Platform::Windows => roaming(env).join(APP_DIR).join("config.toml"),
    }
}

/// Cache, SSH control sockets.
pub fn app_data_dir(env: &Env, platform: Platform) -> PathBuf {
    match platform {
        Platform::Mac => home(env).join("Library").join("Application Support").join(APP_DIR),
        Platform::Windows => local_appdata(env).join(APP_DIR),
    }
}

pub fn app_log_dir(env: &Env, platform: Platform) -> PathBuf {
    match platform {
        Platform::Mac => home(env).join("Library").join("Logs").join(APP_DIR),
        Platform::Windows => local_appdata(env).join(APP_DIR).join("logs"),
    }
}

fn home(env: &Env) -> PathBuf {
    PathBuf::from(env.get("HOME").unwrap_or("/"))
}

fn roaming(env: &Env) -> PathBuf {
    if let Some(a) = env.get("APPDATA") {
        return PathBuf::from(a);
    }
    if let Some(p) = env.get("USERPROFILE") {
        return PathBuf::from(p).join("AppData").join("Roaming");
    }
    PathBuf::from(env.get("HOME").unwrap_or("/")).join(".config")
}

fn local_appdata(env: &Env) -> PathBuf {
    if let Some(a) = env.get("LOCALAPPDATA") {
        return PathBuf::from(a);
    }
    if let Some(p) = env.get("USERPROFILE") {
        return PathBuf::from(p).join("AppData").join("Local");
    }
    PathBuf::from(env.get("HOME").unwrap_or("/")).join(".local").join("share")
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-status-core paths`
Expected: `6 passed`. Note: on macOS `PathBuf::from(r"C:\...")` is one component with backslashes inside, and `join` inserts `/`. If the Windows assertions fail only because of the `/` separator, compare with `.to_string_lossy().replace('/', "\\")` on both sides; the real app only runs these branches on Windows.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-status-core/src/paths.rs crates/herdr-status-core/src/lib.rs
git commit -m "feat(core): herdr socket path rules and bar directories"
```

---

### Task 4: Command builders (`launchers.rs`)

**Files:**
- Create: `crates/herdr-status-core/src/launchers.rs`
- Modify: `crates/herdr-status-core/src/lib.rs`

These strings mirror herdr 0.9.3's own saved-machine bridge (`src/remote/attach.rs` and `src/platform/mod.rs` in herdr). The tests pin the exact text so a host that works with `herdr machine add` also works with the bar.

- [ ] **Step 1: Write the failing tests**

`crates/herdr-status-core/src/launchers.rs` (tests first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::path::PathBuf;

    #[test]
    fn posix_quoting_matches_herdr() {
        assert_eq!(quote_posix("default"), "default");
        assert_eq!(quote_posix("/opt/homebrew/bin/herdr"), "/opt/homebrew/bin/herdr");
        assert_eq!(quote_posix("user@host:2222"), "user@host:2222");
        assert_eq!(quote_posix("build host"), "'build host'");
        assert_eq!(quote_posix("it's"), "'it'\\''s'");
        assert_eq!(quote_posix(""), "''");
    }

    #[test]
    fn powershell_quoting_matches_herdr() {
        assert_eq!(quote_powershell_arg("herdr.exe"), "herdr.exe");
        assert_eq!(quote_powershell_arg("--session default remote-api-bridge"), "'--session default remote-api-bridge'");
        assert_eq!(
            quote_powershell_arg(r"C:\Users\me\AppData\Local\Programs\Herdr\bin\herdr.exe"),
            r"'C:\Users\me\AppData\Local\Programs\Herdr\bin\herdr.exe'"
        );
        assert_eq!(quote_powershell_arg("it's"), "'it''s'");
        assert_eq!(quote_powershell_arg("-x"), "'-x'");
    }

    #[test]
    fn windows_command_line_quoting_matches_herdr() {
        assert_eq!(quote_windows_command_line_arg("default"), "default");
        assert_eq!(quote_windows_command_line_arg(r"C:\dir\"), r"C:\dir\");
        assert_eq!(quote_windows_command_line_arg("my session"), "\"my session\"");
        assert_eq!(quote_windows_command_line_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_windows_command_line_arg(r"a b\"), r#""a b\\""#);
    }

    #[test]
    fn posix_bridge_script_prints_marker_then_execs() {
        let script = posix_bridge_script("/opt/homebrew/bin/herdr", "default");
        assert_eq!(
            script,
            "printf '\\n%s\\n' 'herdr-remote-output-ready:1'\nexec /opt/homebrew/bin/herdr --session default remote-api-bridge"
        );
        let spaced = posix_bridge_script("/Users/me/my tools/herdr", "agent work");
        assert!(spaced.ends_with("exec '/Users/me/my tools/herdr' --session 'agent work' remote-api-bridge"));
        assert_eq!(posix_sh_command("echo hi"), "/bin/sh -c 'echo hi'");
    }

    #[test]
    fn windows_bridge_script_matches_herdr_start_process_fixture() {
        let script = windows_bridge_script("herdr.exe", "default");
        let expected_tail = "$process = Start-Process -FilePath herdr.exe -ArgumentList '--session default remote-api-bridge' -NoNewWindow -PassThru -ErrorAction Stop; $null = $process.Handle; $process.WaitForExit(); exit $process.ExitCode";
        assert_eq!(
            script,
            format!("[Console]::Out.WriteLine(); [Console]::Out.WriteLine('herdr-remote-output-ready:1'); [Console]::Out.Flush(); {expected_tail}")
        );
        let full_path = windows_bridge_script(r"C:\Users\me\AppData\Local\Programs\Herdr\bin\herdr.exe", "agents");
        assert!(full_path.contains(r"-FilePath 'C:\Users\me\AppData\Local\Programs\Herdr\bin\herdr.exe' -ArgumentList '--session agents remote-api-bridge'"));
    }

    #[test]
    fn encoded_command_is_utf16le_base64() {
        let cmd = powershell_encoded_command("exit 0");
        assert!(cmd.starts_with("powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand "));
        let b64 = cmd.rsplit(' ').next().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        let units: Vec<u16> = bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(String::from_utf16(&units).unwrap(), "exit 0");
    }

    #[test]
    fn windows_probe_script_matches_herdr() {
        assert_eq!(
            windows_platform_probe_script(),
            "$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }; [Console]::Out.WriteLine('herdr-windows:' + $arch); exit 0"
        );
    }

    #[test]
    fn discovery_scripts_check_each_candidate() {
        let s = posix_discovery_script("default");
        for c in [
            "command -v herdr",
            "$HOME/.local/bin/herdr",
            "/opt/homebrew/bin/herdr",
            "/usr/local/bin/herdr",
            "/home/linuxbrew/.linuxbrew/bin/herdr",
            "$HOME/.nix-profile/bin/herdr",
        ] {
            assert!(s.contains(c), "missing candidate {c}");
        }
        assert!(s.contains("--session default remote-api-bridge --check"));
        assert!(s.contains("= herdr-api-bridge-v1"));
        assert!(s.contains("*/mise/shims/herdr) continue"));
        assert!(s.ends_with("exit 2"));

        let w = windows_discovery_script("default");
        assert!(w.contains("Get-Command herdr.exe"));
        assert!(w.contains(r"Programs\Herdr\bin\herdr.exe"));
        assert!(w.contains("--session default remote-api-bridge --check"));
        assert!(w.contains("'herdr-api-bridge-v1'"));
    }

    #[test]
    fn ssh_command_has_batch_mode_and_control_options_on_mac_only() {
        let opts = SshOptions { client: Platform::Mac, control_dir: Some(PathBuf::from("/tmp/cm")) };
        let mac = ssh_command("me@winbox", "uname -s; uname -m", &opts);
        assert_eq!(mac.program, "ssh");
        assert_eq!(
            mac.args,
            vec![
                "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3",
                "-o", "ControlMaster=auto", "-o", "ControlPersist=600", "-o", "ControlPath=/tmp/cm/%C",
                "--", "me@winbox", "uname -s; uname -m",
            ]
        );
        let opts = SshOptions { client: Platform::Windows, control_dir: Some(PathBuf::from(r"C:\cm")) };
        let win = ssh_command("winbox", "x", &opts);
        assert!(!win.args.iter().any(|a| a.starts_with("Control")));
        let no_dir = SshOptions { client: Platform::Mac, control_dir: None };
        assert!(!ssh_command("h", "x", &no_dir).args.iter().any(|a| a.starts_with("Control")));
    }

    #[test]
    fn remote_bridge_command_picks_posix_or_powershell_remote_command() {
        let opts = SshOptions { client: Platform::Mac, control_dir: None };
        let posix = remote_bridge_command("box", RemoteOs::Posix, "/usr/local/bin/herdr", "default", &opts);
        assert_eq!(posix.args.last().unwrap(), &posix_sh_command(&posix_bridge_script("/usr/local/bin/herdr", "default")));
        let windows = remote_bridge_command("winbox", RemoteOs::Windows, "herdr.exe", "default", &opts);
        assert!(windows.args.last().unwrap().starts_with("powershell.exe -NoLogo"));
    }

    #[test]
    fn wsl_command_uses_exec_and_login_shell() {
        let c = wsl_command(Some("Ubuntu"), "exec herdr");
        assert_eq!(c.program, "wsl.exe");
        assert_eq!(c.args, vec!["-d", "Ubuntu", "--exec", "sh", "-lc", "exec herdr"]);
        assert_eq!(wsl_command(None, "x").args, vec!["--exec", "sh", "-lc", "x"]);
    }

    #[test]
    fn local_windows_bridge_command_is_plain_herdr_exe() {
        let c = local_herdr_bridge_command(r"C:\h\herdr.exe", "default");
        assert_eq!(c.program, r"C:\h\herdr.exe");
        assert_eq!(c.args, vec!["--session", "default", "remote-api-bridge"]);
    }
}
```

Add to `lib.rs`:

```rust
pub mod launchers;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-status-core launchers`
Expected: compile error.

- [ ] **Step 3: Implement the builders**

Above the tests in `launchers.rs`:

```rust
//! Builders for every child command the bar runs: the SSH bridge to a remote
//! herdr, the `wsl.exe` bridge, and the local `herdr.exe` fallback. Remote
//! command text mirrors herdr 0.9.3's own saved-machine bridge so hosts that
//! work with `herdr machine add` also work here.

use std::path::PathBuf;

use base64::Engine;
use herdr_client::transport::bridge::{ChildCommand, READY_MARKER};

use crate::config::RemoteOs;
use crate::paths::Platform;

/// What `herdr --session <s> remote-api-bridge --check` prints when supported.
pub const BRIDGE_CAPABILITY: &str = "herdr-api-bridge-v1";

/// POSIX single-quote quoting, identical to herdr's `shell_quote`.
pub fn quote_posix(value: &str) -> String {
    if !value.is_empty()
        && value.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '@' | '%' | '_' | '+' | '=' | ':' | ',' | '.' | '/' | '-'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// PowerShell single-quote quoting, identical to herdr's `quote_powershell_arg`.
pub fn quote_powershell_arg(value: &str) -> String {
    if !value.is_empty()
        && !value.starts_with('-')
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b':' | b'+' | b'='))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "''"))
}

/// Windows `CommandLineToArgv` quoting, identical to herdr's `quote_windows_command_line_arg`.
pub fn quote_windows_command_line_arg(value: &str) -> String {
    if !value.is_empty() && !value.chars().any(|ch| matches!(ch, ' ' | '\t' | '\n' | '\x0b' | '"')) {
        return value.to_string();
    }
    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            backslashes += 1;
            continue;
        }
        if ch == '"' {
            quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
        } else {
            quoted.push_str(&"\\".repeat(backslashes));
        }
        backslashes = 0;
        quoted.push(ch);
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

/// Script run on a POSIX host: print the ready marker, then become the bridge.
pub fn posix_bridge_script(herdr_path: &str, session: &str) -> String {
    format!(
        "printf '\\n%s\\n' '{READY_MARKER}'\nexec {} --session {} remote-api-bridge",
        quote_posix(herdr_path),
        quote_posix(session)
    )
}

/// Wrap a script for `ssh host <command>` so the remote login shell does not interpret it.
pub fn posix_sh_command(script: &str) -> String {
    format!("/bin/sh -c {}", quote_posix(script))
}

/// Finds a herdr binary on a POSIX host that supports the API bridge.
/// Prints the path and exits 0, or exits 2 with a message on stderr.
pub fn posix_discovery_script(session: &str) -> String {
    let session = quote_posix(session);
    format!(
        r#"set -f
candidates="$(command -v herdr 2>/dev/null)
$HOME/.local/bin/herdr
/opt/homebrew/bin/herdr
/usr/local/bin/herdr
/home/linuxbrew/.linuxbrew/bin/herdr
$HOME/.nix-profile/bin/herdr"
IFS='
'
for candidate in $candidates; do
    case "$candidate" in
        */mise/shims/herdr) continue ;;
        /*) ;;
        *) continue ;;
    esac
    [ -x "$candidate" ] || continue
    if capability=$("$candidate" --session {session} remote-api-bridge --check </dev/null 2>/dev/null) && [ "$capability" = {BRIDGE_CAPABILITY} ]; then
        printf '%s\n' "$candidate"
        exit 0
    fi
done
printf '%s\n' 'herdr not found or too old on this host' >&2
exit 2"#
    )
}

/// PowerShell script run on a Windows host: marker lines, then herdr through
/// Start-Process so the child inherits raw stdio handles.
pub fn windows_bridge_script(herdr_path: &str, session: &str) -> String {
    let command_line = ["--session", session, "remote-api-bridge"]
        .iter()
        .map(|a| quote_windows_command_line_arg(a))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "[Console]::Out.WriteLine(); [Console]::Out.WriteLine('{READY_MARKER}'); [Console]::Out.Flush(); \
$process = Start-Process -FilePath {} -ArgumentList {} -NoNewWindow -PassThru -ErrorAction Stop; \
$null = $process.Handle; $process.WaitForExit(); exit $process.ExitCode",
        quote_powershell_arg(herdr_path),
        quote_powershell_arg(&command_line)
    )
}

/// Finds `herdr.exe` on a Windows host. Prints the path and exits 0, or exits 2.
pub fn windows_discovery_script(session: &str) -> String {
    let session = quote_powershell_arg(session);
    format!(
        "$candidates = @(); $cmd = Get-Command herdr.exe -ErrorAction SilentlyContinue; if ($cmd) {{ $candidates += $cmd.Source }}; \
$candidates += (Join-Path $env:LOCALAPPDATA 'Programs\\Herdr\\bin\\herdr.exe'); \
foreach ($c in $candidates) {{ if (Test-Path $c) {{ $cap = & $c --session {session} remote-api-bridge --check 2>$null; \
if ($LASTEXITCODE -eq 0 -and $cap -eq '{BRIDGE_CAPABILITY}') {{ [Console]::Out.WriteLine($c); exit 0 }} }} }}; \
[Console]::Error.WriteLine('herdr not found or too old on this host'); exit 2"
    )
}

/// Identical to herdr's Windows platform probe.
pub fn windows_platform_probe_script() -> String {
    "$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }; [Console]::Out.WriteLine('herdr-windows:' + $arch); exit 0".to_string()
}

/// `powershell.exe … -EncodedCommand <base64 of UTF-16LE script>`: no quoting
/// problems through Windows OpenSSH's default shell.
pub fn powershell_encoded_command(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    format!(
        "powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand {}",
        base64::engine::general_purpose::STANDARD.encode(utf16)
    )
}

#[derive(Debug, Clone)]
pub struct SshOptions {
    pub client: Platform,
    /// Directory for ControlMaster sockets (macOS only; Windows OpenSSH lacks it).
    pub control_dir: Option<PathBuf>,
}

/// `ssh` invocation for one remote command. Never prompts (BatchMode).
pub fn ssh_command(target: &str, remote_command: &str, opts: &SshOptions) -> ChildCommand {
    let mut args: Vec<String> = [
        "BatchMode=yes",
        "ConnectTimeout=10",
        "ServerAliveInterval=15",
        "ServerAliveCountMax=3",
    ]
    .iter()
    .flat_map(|o| ["-o".to_string(), o.to_string()])
    .collect();
    if opts.client == Platform::Mac {
        if let Some(dir) = &opts.control_dir {
            args.extend(["-o".into(), "ControlMaster=auto".into()]);
            args.extend(["-o".into(), "ControlPersist=600".into()]);
            args.extend(["-o".into(), format!("ControlPath={}/%C", dir.display())]);
        }
    }
    args.push("--".into());
    args.push(target.to_string());
    args.push(remote_command.to_string());
    ChildCommand { program: "ssh".into(), args }
}

/// The long-lived bridge command for a resolved remote.
pub fn remote_bridge_command(target: &str, os: RemoteOs, herdr_path: &str, session: &str, opts: &SshOptions) -> ChildCommand {
    let remote = match os {
        RemoteOs::Posix => posix_sh_command(&posix_bridge_script(herdr_path, session)),
        RemoteOs::Windows => powershell_encoded_command(&windows_bridge_script(herdr_path, session)),
    };
    ssh_command(target, &remote, opts)
}

/// `wsl.exe [-d distro] --exec sh -lc <script>`: `--exec` skips the distro's
/// default shell so the script is passed as one argv element.
pub fn wsl_command(distro: Option<&str>, script: &str) -> ChildCommand {
    let mut args: Vec<String> = Vec::new();
    if let Some(d) = distro {
        args.extend(["-d".into(), d.to_string()]);
    }
    args.extend(["--exec".into(), "sh".into(), "-lc".into(), script.to_string()]);
    ChildCommand { program: "wsl.exe".into(), args }
}

/// Local Windows fallback when the named pipe cannot be opened.
pub fn local_herdr_bridge_command(herdr_exe: &str, session: &str) -> ChildCommand {
    ChildCommand {
        program: herdr_exe.to_string(),
        args: vec!["--session".into(), session.to_string(), "remote-api-bridge".into()],
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-status-core launchers`
Expected: `12 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-status-core/src/launchers.rs crates/herdr-status-core/src/lib.rs
git commit -m "feat(core): ssh, wsl and powershell command builders mirroring herdr's bridge"
```

---

### Task 5: Remote output parsers and SSH failure classification

**Files:**
- Create: `crates/herdr-status-core/src/remote_probe.rs`
- Modify: `crates/herdr-status-core/src/launchers.rs` (discovery scripts print a prefixed line)
- Modify: `crates/herdr-status-core/src/lib.rs`

Discovery output travels through the remote login shell, which may print its own lines. So both discovery scripts print `herdr-status-discovery:<path>` and the parser looks for that prefix.

- [ ] **Step 1: Write the failing tests**

`crates/herdr-status-core/src/remote_probe.rs` (tests first):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uname_linux_and_darwin_are_posix() {
        assert_eq!(parse_uname("Linux\nx86_64\n"), Some(RemotePlatform { os: RemoteOs::Posix, arch: "x86_64".into() }));
        assert_eq!(parse_uname("Darwin\narm64\n"), Some(RemotePlatform { os: RemoteOs::Posix, arch: "arm64".into() }));
        assert_eq!(parse_uname("MINGW64_NT-10.0\nx86_64\n"), None);
        assert_eq!(parse_uname(""), None);
    }

    #[test]
    fn windows_hint_and_probe_rule_match_herdr() {
        assert!(looks_like_windows_uname("MINGW64_NT-10.0"));
        assert!(looks_like_windows_uname("MSYS_NT-10.0"));
        assert!(!looks_like_windows_uname("Linux"));
        assert!(should_try_windows_probe(true, Some(0), true));
        assert!(!should_try_windows_probe(true, Some(0), false));
        assert!(should_try_windows_probe(false, Some(1), false));
        assert!(!should_try_windows_probe(false, Some(255), false), "255 is ssh itself failing");
        assert!(!should_try_windows_probe(false, None, false));
    }

    #[test]
    fn windows_probe_line_parses_arch() {
        assert_eq!(parse_windows_probe("\r\nherdr-windows:AMD64\r\n"), Some(RemotePlatform { os: RemoteOs::Windows, arch: "AMD64".into() }));
        assert_eq!(parse_windows_probe("banner\nherdr-windows:ARM64"), Some(RemotePlatform { os: RemoteOs::Windows, arch: "ARM64".into() }));
        assert_eq!(parse_windows_probe("nothing"), None);
    }

    #[test]
    fn discovery_output_uses_prefix_and_ignores_noise() {
        assert_eq!(parse_discovery_output("Welcome!\nherdr-status-discovery:/usr/local/bin/herdr\n"), Some("/usr/local/bin/herdr".into()));
        assert_eq!(parse_discovery_output(r"herdr-status-discovery:C:\Users\me\AppData\Local\Programs\Herdr\bin\herdr.exe"), Some(r"C:\Users\me\AppData\Local\Programs\Herdr\bin\herdr.exe".into()));
        assert_eq!(parse_discovery_output("/usr/local/bin/herdr"), None);
    }

    #[test]
    fn ssh_failures_are_classified_and_explained() {
        assert_eq!(classify_ssh_failure(Some(255), "me@box: Permission denied (publickey)."), SshFailure::AuthDenied);
        assert_eq!(classify_ssh_failure(Some(255), "Host key verification failed."), SshFailure::HostKey);
        assert_eq!(classify_ssh_failure(Some(255), "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!"), SshFailure::HostKey);
        assert_eq!(classify_ssh_failure(Some(255), "ssh: connect to host box port 22: Connection timed out"), SshFailure::Unreachable);
        assert_eq!(classify_ssh_failure(Some(255), "ssh: Could not resolve hostname box"), SshFailure::Unreachable);
        assert_eq!(classify_ssh_failure(Some(1), "something odd\nlast line"), SshFailure::Other("last line".into()));
        assert_eq!(SshFailure::AuthDenied.reason("box"), "SSH authentication failed; run `ssh box` once in a terminal");
        assert_eq!(SshFailure::HostKey.reason("box"), "Host key verification failed; connect with `ssh box` to accept it");
        assert_eq!(SshFailure::Unreachable.reason("box"), "Unreachable");
        assert_eq!(SshFailure::Other("boom".into()).reason("box"), "SSH failed: boom");
    }
}
```

Add to the `discovery_scripts_check_each_candidate` test in `launchers.rs`:

```rust
        assert!(s.contains("printf 'herdr-status-discovery:%s\\n' \"$candidate\""));
        assert!(w.contains("[Console]::Out.WriteLine('herdr-status-discovery:' + $c)"));
```

Add to `lib.rs`:

```rust
pub mod remote_probe;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-status-core remote_probe launchers`
Expected: compile error for `remote_probe`; the two new launcher assertions fail.

- [ ] **Step 3: Implement the parsers and update the discovery scripts**

In `launchers.rs`, in `posix_discovery_script`, replace the line

```
        printf '%s\n' "$candidate"
```

with

```
        printf 'herdr-status-discovery:%s\n' "$candidate"
```

and in `windows_discovery_script` replace `[Console]::Out.WriteLine($c)` with `[Console]::Out.WriteLine('herdr-status-discovery:' + $c)`.

Then `crates/herdr-status-core/src/remote_probe.rs` above the tests:

```rust
//! Parsers for what a remote host prints during first contact, plus SSH
//! failure classification for user-facing reasons.

use crate::config::RemoteOs;

pub const DISCOVERY_PREFIX: &str = "herdr-status-discovery:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePlatform {
    pub os: RemoteOs,
    pub arch: String,
}

/// Parse `uname -s; uname -m` output. Anything not a known Unix is `None`.
pub fn parse_uname(stdout: &str) -> Option<RemotePlatform> {
    let mut lines = stdout.lines().map(str::trim).filter(|l| !l.is_empty());
    let os = lines.next()?;
    let arch = lines.next().unwrap_or("unknown");
    match os {
        "Linux" | "Darwin" | "FreeBSD" | "OpenBSD" | "NetBSD" => {
            Some(RemotePlatform { os: RemoteOs::Posix, arch: arch.to_string() })
        }
        _ => None,
    }
}

pub fn looks_like_windows_uname(os: &str) -> bool {
    let upper = os.to_ascii_uppercase();
    upper.contains("MINGW") || upper.contains("MSYS") || upper.contains("CYGWIN") || upper.contains("WINDOWS")
}

/// herdr's rule: probe for Windows when `uname` succeeded but looked like
/// Windows, or failed with any exit code except 255 (255 is ssh itself failing).
pub fn should_try_windows_probe(success: bool, exit_code: Option<i32>, windows_hint: bool) -> bool {
    (success && windows_hint) || (!success && exit_code.is_some_and(|code| code != 255))
}

pub fn parse_windows_probe(stdout: &str) -> Option<RemotePlatform> {
    stdout
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("herdr-windows:"))
        .map(|arch| RemotePlatform { os: RemoteOs::Windows, arch: arch.trim().to_string() })
}

/// The herdr path printed by a discovery script, ignoring any other output.
pub fn parse_discovery_output(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix(DISCOVERY_PREFIX))
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshFailure {
    AuthDenied,
    HostKey,
    Unreachable,
    Other(String),
}

pub fn classify_ssh_failure(exit_code: Option<i32>, stderr: &str) -> SshFailure {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("permission denied") {
        SshFailure::AuthDenied
    } else if lower.contains("host key verification failed") || lower.contains("remote host identification has changed") {
        SshFailure::HostKey
    } else if exit_code == Some(255)
        || lower.contains("timed out")
        || lower.contains("could not resolve hostname")
        || lower.contains("connection refused")
        || lower.contains("no route to host")
    {
        SshFailure::Unreachable
    } else {
        SshFailure::Other(stderr.trim().lines().last().unwrap_or("").trim().to_string())
    }
}

impl SshFailure {
    /// Text shown on the offline group line (spec section 14).
    pub fn reason(&self, target: &str) -> String {
        match self {
            SshFailure::AuthDenied => format!("SSH authentication failed; run `ssh {target}` once in a terminal"),
            SshFailure::HostKey => format!("Host key verification failed; connect with `ssh {target}` to accept it"),
            SshFailure::Unreachable => "Unreachable".to_string(),
            SshFailure::Other(msg) if msg.is_empty() => "SSH failed".to_string(),
            SshFailure::Other(msg) => format!("SSH failed: {msg}"),
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-status-core remote_probe launchers`
Expected: all pass (5 new + 12 launcher tests).

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-status-core/src/remote_probe.rs crates/herdr-status-core/src/launchers.rs crates/herdr-status-core/src/lib.rs
git commit -m "feat(core): remote probe parsers and ssh failure classification"
```

---

### Task 6: Remote first contact (`remote_connect.rs`): runner trait, cache, resolver

**Files:**
- Create: `crates/herdr-status-core/src/remote_connect.rs`
- Modify: `crates/herdr-status-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

`crates/herdr-status-core/src/remote_connect.rs` (tests first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Returns scripted outputs in order and records every command.
    struct FakeRunner {
        outputs: Mutex<VecDeque<RunOutput>>,
        calls: Mutex<Vec<ChildCommand>>,
    }

    impl FakeRunner {
        fn new(outputs: Vec<RunOutput>) -> Self {
            Self { outputs: Mutex::new(outputs.into()), calls: Mutex::new(Vec::new()) }
        }
        fn calls(&self) -> Vec<ChildCommand> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ProcessRunner for FakeRunner {
        async fn run(&self, command: &ChildCommand, _timeout: Duration) -> std::io::Result<RunOutput> {
            self.calls.lock().unwrap().push(command.clone());
            Ok(self.outputs.lock().unwrap().pop_front().unwrap_or(RunOutput {
                exit_code: Some(1),
                stdout: String::new(),
                stderr: "unscripted call".into(),
            }))
        }
    }

    fn ok(stdout: &str) -> RunOutput {
        RunOutput { exit_code: Some(0), stdout: stdout.into(), stderr: String::new() }
    }
    fn fail(code: i32, stderr: &str) -> RunOutput {
        RunOutput { exit_code: Some(code), stdout: String::new(), stderr: stderr.into() }
    }
    fn cfg(target: &str) -> RemoteConfig {
        RemoteConfig { label: target.into(), ssh_target: target.into(), session: "default".into(), os: None, herdr_path: None }
    }
    fn resolver(runner: &FakeRunner) -> RemoteResolver<'_> {
        RemoteResolver { runner, ssh: SshOptions { client: Platform::Mac, control_dir: None }, timeout: Duration::from_secs(5) }
    }

    #[tokio::test]
    async fn linux_host_resolves_posix_and_discovers_path() {
        let runner = FakeRunner::new(vec![ok("Linux\nx86_64\n"), ok("herdr-status-discovery:/usr/local/bin/herdr\n")]);
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
            fail(1, "'uname' is not recognized as an internal or external command"),
            ok("\r\nherdr-windows:AMD64\r\n"),
            ok(r"herdr-status-discovery:C:\Users\me\AppData\Local\Programs\Herdr\bin\herdr.exe"),
        ]);
        let entry = resolver(&runner).resolve(&cfg("winbox")).await.unwrap();
        assert_eq!(entry.os, RemoteOs::Windows);
        assert_eq!(entry.arch, "AMD64");
        assert!(entry.herdr_path.ends_with("herdr.exe"));
        let calls = runner.calls();
        assert!(calls[1].args.last().unwrap().starts_with("powershell.exe -NoLogo"));
        assert!(calls[2].args.last().unwrap().starts_with("powershell.exe -NoLogo"));
    }

    #[tokio::test]
    async fn auth_denied_is_an_ssh_error_with_hint() {
        let runner = FakeRunner::new(vec![fail(255, "me@box: Permission denied (publickey).")]);
        let err = resolver(&runner).resolve(&cfg("box")).await.unwrap_err();
        assert!(matches!(err, RemoteError::Ssh(SshFailure::AuthDenied, ref t) if t == "box"), "{err:?}");
        assert_eq!(err.to_string(), "SSH authentication failed; run `ssh box` once in a terminal");
        assert_eq!(runner.calls().len(), 1, "no probe after an ssh-level failure");
    }

    #[tokio::test]
    async fn missing_herdr_is_reported() {
        let runner = FakeRunner::new(vec![ok("Linux\naarch64\n"), fail(2, "herdr not found or too old on this host")]);
        let err = resolver(&runner).resolve(&cfg("box")).await.unwrap_err();
        assert!(matches!(err, RemoteError::HerdrNotFound(ref t) if t == "box"), "{err:?}");
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
        cache.insert("box", RemoteCacheEntry { os: RemoteOs::Posix, arch: "x86_64".into(), herdr_path: "/usr/local/bin/herdr".into(), checked_at: 42 });
        cache.save(&path).unwrap();
        let loaded = RemoteCache::load(&path);
        assert_eq!(loaded.get("box").unwrap().herdr_path, "/usr/local/bin/herdr");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(RemoteCache::load(&path).entries.is_empty());
    }

    #[test]
    fn bridge_transport_describes_target_and_os() {
        let entry = RemoteCacheEntry { os: RemoteOs::Windows, arch: "AMD64".into(), herdr_path: "herdr.exe".into(), checked_at: 0 };
        let t = bridge_transport_for(&cfg("winbox"), &entry, &SshOptions { client: Platform::Mac, control_dir: None });
        assert_eq!(t.describe(), "ssh winbox (windows)");
        assert_eq!(t.command().program, "ssh");
    }
}
```

Add to `lib.rs`:

```rust
pub mod remote_connect;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-status-core remote_connect`
Expected: compile error.

- [ ] **Step 3: Implement runner, cache and resolver**

Above the tests in `remote_connect.rs`:

```rust
//! First contact with a remote host: detect its OS, find a herdr that supports
//! the API bridge, and cache the answer. All process execution goes through
//! [`ProcessRunner`] so this is testable without SSH.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use herdr_client::transport::bridge::{BridgeTransport, ChildCommand};
use herdr_client::Transport;
use serde::{Deserialize, Serialize};

use crate::config::{RemoteConfig, RemoteOs};
use crate::launchers::{self, SshOptions};
use crate::paths::Platform;
use crate::remote_probe::{
    classify_ssh_failure, looks_like_windows_uname, parse_discovery_output, parse_uname, parse_windows_probe,
    should_try_windows_probe, RemotePlatform, SshFailure,
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
        cmd.args(&command.args).stdin(Stdio::null()).kill_on_drop(true);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, format!("{} timed out after {timeout:?}", command.program)))??;
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
        std::fs::write(path, serde_json::to_string_pretty(self).expect("cache serialises"))
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
            Some(os) => RemotePlatform { os: os.clone(), arch: "configured".to_string() },
            None => self.detect_platform(cfg).await?,
        };
        let herdr_path = match &cfg.herdr_path {
            Some(path) => path.clone(),
            None => self.discover(cfg, &platform.os).await?,
        };
        Ok(RemoteCacheEntry { os: platform.os, arch: platform.arch, herdr_path, checked_at: now_secs() })
    }

    async fn ssh(&self, cfg: &RemoteConfig, remote_command: &str) -> Result<RunOutput, RemoteError> {
        let command = launchers::ssh_command(&cfg.ssh_target, remote_command, &self.ssh);
        self.runner.run(&command, self.timeout).await.map_err(|e| RemoteError::Io(e.to_string()))
    }

    async fn detect_platform(&self, cfg: &RemoteConfig) -> Result<RemotePlatform, RemoteError> {
        let out = self.ssh(cfg, "uname -s; uname -m").await?;
        if out.success() {
            if let Some(platform) = parse_uname(&out.stdout) {
                return Ok(platform);
            }
        }
        let hint = out.success() && looks_like_windows_uname(out.stdout.lines().next().unwrap_or(""));
        if should_try_windows_probe(out.success(), out.exit_code, hint) {
            let probe_command = launchers::powershell_encoded_command(&launchers::windows_platform_probe_script());
            let probe = self.ssh(cfg, &probe_command).await?;
            if probe.success() {
                if let Some(platform) = parse_windows_probe(&probe.stdout) {
                    return Ok(platform);
                }
            }
        }
        if !out.success() && (out.exit_code == Some(255) || !out.stderr.trim().is_empty()) {
            return Err(RemoteError::Ssh(classify_ssh_failure(out.exit_code, &out.stderr), cfg.ssh_target.clone()));
        }
        Err(RemoteError::UnsupportedPlatform(out.stdout.trim().to_string()))
    }

    async fn discover(&self, cfg: &RemoteConfig, os: &RemoteOs) -> Result<String, RemoteError> {
        let remote_command = match os {
            RemoteOs::Posix => launchers::posix_sh_command(&launchers::posix_discovery_script(&cfg.session)),
            RemoteOs::Windows => launchers::powershell_encoded_command(&launchers::windows_discovery_script(&cfg.session)),
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
        Err(RemoteError::Ssh(classify_ssh_failure(out.exit_code, &out.stderr), cfg.ssh_target.clone()))
    }
}

/// The long-lived bridge transport for a resolved remote.
pub fn bridge_transport_for(cfg: &RemoteConfig, entry: &RemoteCacheEntry, ssh: &SshOptions) -> BridgeTransport {
    let command = launchers::remote_bridge_command(&cfg.ssh_target, entry.os.clone(), &entry.herdr_path, &cfg.session, ssh);
    let os = match entry.os {
        RemoteOs::Posix => "posix",
        RemoteOs::Windows => "windows",
    };
    BridgeTransport::new(command, format!("ssh {} ({os})", cfg.ssh_target))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[allow(dead_code)]
fn _platform_is_used(_: Platform) {}
```

Remove the trailing `_platform_is_used` helper if clippy does not need it (it only exists to keep the `Platform` import used by tests on all platforms; if the import is unused in non-test builds, move `use crate::paths::Platform;` into the test module instead).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-status-core remote_connect`
Expected: `7 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-status-core/src/remote_connect.rs crates/herdr-status-core/src/lib.rs
git commit -m "feat(core): remote resolver with process runner trait and cache"
```

---

### Task 7: State model and `assemble` (`model.rs`)

**Files:**
- Create: `crates/herdr-status-core/src/model.rs`
- Modify: `crates/herdr-status-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

`crates/herdr-status-core/src/model.rs` (tests first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn row(server: &str, name: &str, status: AgentStatus, since: Option<SystemTime>) -> AgentRow {
        AgentRow {
            server_id: server.into(),
            server_label: server.into(),
            pane_id: format!("{server}:{name}"),
            workspace_label: "ws".into(),
            agent_kind: Some("claude".into()),
            name: name.into(),
            status,
            since,
            flash_seq: 0,
        }
    }

    #[test]
    fn sorts_by_urgency_then_newest_change_then_server_then_name() {
        let mut rows = vec![
            row("local", "idle-old", AgentStatus::Idle, Some(t(10))),
            row("local", "working-new", AgentStatus::Working, Some(t(50))),
            row("winbox", "blocked", AgentStatus::Blocked, None),
            row("local", "working-old", AgentStatus::Working, Some(t(20))),
            row("local", "working-unknown-since", AgentStatus::Working, None),
            row("local", "done", AgentStatus::Done, Some(t(30))),
            row("box", "zeta", AgentStatus::Unknown, None),
            row("box", "alpha", AgentStatus::Unknown, None),
        ];
        sort_rows(&mut rows);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["blocked", "working-new", "working-old", "working-unknown-since", "done", "idle-old", "alpha", "zeta"]
        );
    }

    #[test]
    fn assemble_splits_online_rows_from_offline_groups_and_counts_online_only() {
        let mut local = ServerState::new("local", "local", ServerKind::Local);
        local.connection = ConnectionState::Online;
        local.rows = vec![
            row("local", "a", AgentStatus::Working, None),
            row("local", "b", AgentStatus::Blocked, None),
        ];
        let mut remote = ServerState::new("winbox", "winbox", ServerKind::Remote);
        remote.connection = ConnectionState::Offline { reason: "Unreachable".into(), retry_at: t(100) };
        remote.rows = vec![row("winbox", "c", AgentStatus::Done, None)];
        let connecting = ServerState::new("wsl", "wsl", ServerKind::Wsl);

        let list = assemble(&[local, remote, connecting]);
        assert_eq!(list.servers.len(), 3);
        assert_eq!(list.servers[1].connection, ConnectionState::Offline { reason: "Unreachable".into(), retry_at: t(100) });
        assert_eq!(list.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["b", "a"]);
        assert_eq!(list.offline.len(), 1);
        assert_eq!(list.offline[0].label, "winbox");
        assert_eq!(list.offline[0].reason, "Unreachable");
        assert_eq!(list.offline[0].rows[0].name, "c");
        assert_eq!(list.counts, Counts { blocked: 1, working: 1, done: 0, idle: 0, unknown: 0 });
    }

    #[test]
    fn serialises_times_as_unix_millis_and_statuses_lowercase() {
        let r = row("local", "a", AgentStatus::Blocked, Some(t(5)));
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["since"], 5000);
        assert_eq!(v["status"], "blocked");
        let none = row("local", "a", AgentStatus::Idle, None);
        assert_eq!(serde_json::to_value(&none).unwrap()["since"], serde_json::Value::Null);
        let offline = ConnectionState::Offline { reason: "x".into(), retry_at: t(7) };
        let v = serde_json::to_value(&offline).unwrap();
        assert_eq!(v["state"], "offline");
        assert_eq!(v["retry_at"], 7000);
        assert_eq!(serde_json::to_value(ConnectionState::Online).unwrap()["state"], "online");
    }
}
```

Add to `lib.rs`:

```rust
pub mod model;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-status-core model`
Expected: compile error.

- [ ] **Step 3: Implement the model**

Above the tests in `model.rs`:

```rust
//! What the UI renders. Pure data; serialised as the Tauri `agents` event.

use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Serialize, Serializer};

pub use herdr_client::AgentStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerKind {
    Local,
    Wsl,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ConnectionState {
    Connecting,
    Online,
    Offline {
        reason: String,
        #[serde(serialize_with = "ser_millis")]
        retry_at: SystemTime,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentRow {
    pub server_id: String,
    pub server_label: String,
    pub pane_id: String,
    pub workspace_label: String,
    pub agent_kind: Option<String>,
    pub name: String,
    pub status: AgentStatus,
    /// When the bar observed the current status; `None` until the first change.
    #[serde(serialize_with = "ser_opt_millis")]
    pub since: Option<SystemTime>,
    /// Bumped on every observed status change so the UI can replay the flash.
    pub flash_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerState {
    pub id: String,
    pub label: String,
    pub kind: ServerKind,
    pub connection: ConnectionState,
    pub rows: Vec<AgentRow>,
    /// `Transport::describe()`, for tooltips.
    pub transport: String,
}

impl ServerState {
    pub fn new(id: &str, label: &str, kind: ServerKind) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            kind,
            connection: ConnectionState::Connecting,
            rows: Vec::new(),
            transport: String::new(),
        }
    }

    pub fn is_online(&self) -> bool {
        self.connection == ConnectionState::Online
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerChip {
    pub id: String,
    pub label: String,
    pub kind: ServerKind,
    pub connection: ConnectionState,
    pub transport: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OfflineGroup {
    pub server_id: String,
    pub label: String,
    pub reason: String,
    #[serde(serialize_with = "ser_millis")]
    pub retry_at: SystemTime,
    pub rows: Vec<AgentRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub blocked: usize,
    pub working: usize,
    pub done: usize,
    pub idle: usize,
    pub unknown: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AgentList {
    pub servers: Vec<ServerChip>,
    /// Online servers only, urgency-sorted.
    pub rows: Vec<AgentRow>,
    pub offline: Vec<OfflineGroup>,
    pub counts: Counts,
}

/// Urgency rank, then newest change first (`None` last), then server, then name.
pub fn sort_rows(rows: &mut [AgentRow]) {
    rows.sort_by(|a, b| {
        a.status
            .cmp(&b.status)
            .then_with(|| cmp_since_desc(a.since, b.since))
            .then_with(|| a.server_label.cmp(&b.server_label))
            .then_with(|| a.name.cmp(&b.name))
    });
}

fn cmp_since_desc(a: Option<SystemTime>, b: Option<SystemTime>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

pub fn assemble(servers: &[ServerState]) -> AgentList {
    let mut list = AgentList::default();
    for server in servers {
        list.servers.push(ServerChip {
            id: server.id.clone(),
            label: server.label.clone(),
            kind: server.kind,
            connection: server.connection.clone(),
            transport: server.transport.clone(),
        });
        match &server.connection {
            ConnectionState::Online => list.rows.extend(server.rows.iter().cloned()),
            ConnectionState::Offline { reason, retry_at } => list.offline.push(OfflineGroup {
                server_id: server.id.clone(),
                label: server.label.clone(),
                reason: reason.clone(),
                retry_at: *retry_at,
                rows: server.rows.clone(),
            }),
            ConnectionState::Connecting => {}
        }
    }
    sort_rows(&mut list.rows);
    for row in &list.rows {
        match row.status {
            AgentStatus::Blocked => list.counts.blocked += 1,
            AgentStatus::Working => list.counts.working += 1,
            AgentStatus::Done => list.counts.done += 1,
            AgentStatus::Idle => list.counts.idle += 1,
            AgentStatus::Unknown => list.counts.unknown += 1,
        }
    }
    list
}

fn millis(t: &SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn ser_millis<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(millis(t))
}

fn ser_opt_millis<S: Serializer>(t: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
    match t {
        Some(t) => s.serialize_some(&millis(t)),
        None => s.serialize_none(),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-status-core model`
Expected: `3 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-status-core/src/model.rs crates/herdr-status-core/src/lib.rs
git commit -m "feat(core): agent list model with urgency sort and offline groups"
```

---

### Task 8: Reducer (`reducer.rs`)

**Files:**
- Create: `crates/herdr-status-core/src/reducer.rs`
- Modify: `crates/herdr-status-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

`crates/herdr-status-core/src/reducer.rs` (tests first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use herdr_client::protocol::{parse_line, ResponseResult, ServerLine};
    use herdr_client::{AgentInfo, PaneAgentStatusChanged, SessionSnapshot};
    use std::time::Duration;

    const FIXTURE: &str = include_str!("../../herdr-client/tests/fixtures/snapshot.json");

    fn fixture() -> SessionSnapshot {
        let compact: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        match parse_line(&compact.to_string()).unwrap() {
            ServerLine::Response { result: ResponseResult::SessionSnapshot { snapshot }, .. } => snapshot,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn status_event(pane_id: &str, status: AgentStatus) -> PaneAgentStatusChanged {
        PaneAgentStatusChanged {
            pane_id: pane_id.into(),
            workspace_id: "w2".into(),
            agent_status: status,
            agent: Some("claude".into()),
            title: None,
            display_agent: None,
        }
    }

    fn server() -> ServerState {
        ServerState::new("local", "local", ServerKind::Local)
    }

    #[test]
    fn first_snapshot_builds_rows_without_since_or_flash() {
        let mut s = server();
        apply_snapshot(&mut s, &fixture(), t(1));
        assert_eq!(s.rows.len(), 2);
        let w2 = s.rows.iter().find(|r| r.pane_id == "w2:p1").unwrap();
        assert_eq!(w2.name, "Herdr agent status bar");
        assert_eq!(w2.workspace_label, "herdr-status");
        assert_eq!(w2.agent_kind.as_deref(), Some("claude"));
        assert_eq!(w2.status, AgentStatus::Working);
        assert_eq!(w2.since, None);
        assert_eq!(w2.flash_seq, 0);
        assert_eq!(w2.server_label, "local");
    }

    #[test]
    fn status_event_sets_since_and_bumps_flash_only_on_change() {
        let mut s = server();
        apply_snapshot(&mut s, &fixture(), t(1));
        assert!(apply_status(&mut s, &status_event("w2:p1", AgentStatus::Idle), t(2)));
        let w2 = s.rows.iter().find(|r| r.pane_id == "w2:p1").unwrap();
        assert_eq!(w2.status, AgentStatus::Idle);
        assert_eq!(w2.since, Some(t(2)));
        assert_eq!(w2.flash_seq, 1);
        assert!(apply_status(&mut s, &status_event("w2:p1", AgentStatus::Idle), t(3)));
        let w2 = s.rows.iter().find(|r| r.pane_id == "w2:p1").unwrap();
        assert_eq!(w2.since, Some(t(2)), "unchanged status keeps since");
        assert_eq!(w2.flash_seq, 1);
    }

    #[test]
    fn status_event_for_unknown_pane_returns_false() {
        let mut s = server();
        apply_snapshot(&mut s, &fixture(), t(1));
        assert!(!apply_status(&mut s, &status_event("w9:p9", AgentStatus::Blocked), t(2)));
    }

    #[test]
    fn later_snapshot_keeps_since_for_same_status_and_resets_on_change() {
        let mut s = server();
        apply_snapshot(&mut s, &fixture(), t(1));
        apply_status(&mut s, &status_event("w2:p1", AgentStatus::Idle), t(2));
        let mut snap = fixture();
        snap.agents.iter_mut().find(|a| a.pane_id == "w2:p1").unwrap().agent_status = AgentStatus::Idle;
        apply_snapshot(&mut s, &snap, t(5));
        let w2 = s.rows.iter().find(|r| r.pane_id == "w2:p1").unwrap();
        assert_eq!(w2.since, Some(t(2)));
        assert_eq!(w2.flash_seq, 1);
        snap.agents.iter_mut().find(|a| a.pane_id == "w2:p1").unwrap().agent_status = AgentStatus::Blocked;
        apply_snapshot(&mut s, &snap, t(9));
        let w2 = s.rows.iter().find(|r| r.pane_id == "w2:p1").unwrap();
        assert_eq!(w2.status, AgentStatus::Blocked);
        assert_eq!(w2.since, Some(t(9)));
        assert_eq!(w2.flash_seq, 2);
    }

    #[test]
    fn snapshot_drops_rows_for_panes_that_are_gone_and_remove_pane_works() {
        let mut s = server();
        apply_snapshot(&mut s, &fixture(), t(1));
        let mut snap = fixture();
        snap.agents.retain(|a| a.pane_id != "w1:p1");
        apply_snapshot(&mut s, &snap, t(2));
        assert_eq!(s.rows.len(), 1);
        remove_pane(&mut s, "w2:p1");
        assert!(s.rows.is_empty());
    }

    #[test]
    fn display_name_priority() {
        let base = AgentInfo {
            pane_id: "w1:p1".into(),
            workspace_id: "w1".into(),
            tab_id: String::new(),
            agent: Some("codex".into()),
            agent_status: AgentStatus::Idle,
            name: None,
            title: None,
            display_agent: None,
            terminal_title_stripped: None,
            terminal_title: None,
            focused: false,
            cwd: None,
        };
        assert_eq!(display_name(&base, "swat"), "codex · swat");
        let with_terminal = AgentInfo { terminal_title_stripped: Some(" PR review ".into()), ..base.clone() };
        assert_eq!(display_name(&with_terminal, "swat"), "PR review");
        let with_title = AgentInfo { title: Some("Reviewer pane".into()), ..with_terminal.clone() };
        assert_eq!(display_name(&with_title, "swat"), "Reviewer pane");
        let with_name = AgentInfo { name: Some("reviewer".into()), ..with_title };
        assert_eq!(display_name(&with_name, "swat"), "reviewer");
        let no_kind = AgentInfo { agent: None, ..base };
        assert_eq!(display_name(&no_kind, "swat"), "agent · swat");
    }

    #[test]
    fn offline_keeps_rows_and_online_restores_state() {
        let mut s = server();
        apply_snapshot(&mut s, &fixture(), t(1));
        set_offline(&mut s, "Unreachable".into(), t(30));
        assert_eq!(s.connection, ConnectionState::Offline { reason: "Unreachable".into(), retry_at: t(30) });
        assert_eq!(s.rows.len(), 2);
        set_online(&mut s, "unix socket /x");
        assert!(s.is_online());
        assert_eq!(s.transport, "unix socket /x");
    }
}
```

Add to `lib.rs`:

```rust
pub mod reducer;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-status-core reducer`
Expected: compile error.

- [ ] **Step 3: Implement the reducer**

Above the tests in `reducer.rs`:

```rust
//! Pure transitions of one server's state. The watcher calls these; nothing
//! here does I/O.

use std::time::SystemTime;

use herdr_client::{AgentInfo, PaneAgentStatusChanged, SessionSnapshot};

use crate::model::{AgentRow, AgentStatus, ConnectionState, ServerKind, ServerState};

/// Row name: live agent name, else herdr's presentation title, else the
/// terminal title, else `"<kind> · <workspace>"`.
pub fn display_name(agent: &AgentInfo, workspace_label: &str) -> String {
    for candidate in [&agent.name, &agent.title, &agent.terminal_title_stripped] {
        if let Some(text) = candidate.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            return text.to_string();
        }
    }
    format!("{} · {}", agent.agent.as_deref().unwrap_or("agent"), workspace_label)
}

/// Rebuild rows from an authoritative snapshot. Rows whose status is unchanged
/// keep `since` and `flash_seq`; a changed status counts as observed now.
pub fn apply_snapshot(state: &mut ServerState, snapshot: &SessionSnapshot, now: SystemTime) {
    let mut rows = Vec::with_capacity(snapshot.agents.len());
    for agent in &snapshot.agents {
        let workspace_label = snapshot
            .workspace_label(&agent.workspace_id)
            .filter(|l| !l.is_empty())
            .unwrap_or(&agent.workspace_id)
            .to_string();
        let name = display_name(agent, &workspace_label);
        let previous = state.rows.iter().find(|r| r.pane_id == agent.pane_id);
        let (since, flash_seq) = match previous {
            Some(p) if p.status == agent.agent_status => (p.since, p.flash_seq),
            Some(p) => (Some(now), p.flash_seq + 1),
            None => (None, 0),
        };
        rows.push(AgentRow {
            server_id: state.id.clone(),
            server_label: state.label.clone(),
            pane_id: agent.pane_id.clone(),
            workspace_label,
            agent_kind: agent.agent.clone(),
            name,
            status: agent.agent_status,
            since,
            flash_seq,
        });
    }
    state.rows = rows;
}

/// Apply a per-pane status event. Returns `false` when the pane has no row yet;
/// the caller should then take a snapshot instead of inventing a row.
pub fn apply_status(state: &mut ServerState, event: &PaneAgentStatusChanged, now: SystemTime) -> bool {
    let Some(row) = state.rows.iter_mut().find(|r| r.pane_id == event.pane_id) else {
        return false;
    };
    if row.status != event.agent_status {
        row.status = event.agent_status;
        row.since = Some(now);
        row.flash_seq += 1;
    }
    true
}

pub fn remove_pane(state: &mut ServerState, pane_id: &str) {
    state.rows.retain(|r| r.pane_id != pane_id);
}

/// Rows are kept so the UI can show them dimmed.
pub fn set_offline(state: &mut ServerState, reason: String, retry_at: SystemTime) {
    state.connection = ConnectionState::Offline { reason, retry_at };
}

pub fn set_online(state: &mut ServerState, transport: &str) {
    state.connection = ConnectionState::Online;
    state.transport = transport.to_string();
}

pub fn set_connecting(state: &mut ServerState) {
    state.connection = ConnectionState::Connecting;
}

#[allow(dead_code)]
fn _kinds_are_used(_: ServerKind) {}
```

If `ServerKind` ends up unused outside tests, drop the `_kinds_are_used` helper and move the `ServerKind` import into the test module.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-status-core reducer`
Expected: `7 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-status-core/src/reducer.rs crates/herdr-status-core/src/lib.rs
git commit -m "feat(core): snapshot and status reducer with since/flash tracking"
```

---

### Task 9: Transport sources (`transport_source.rs`)

**Files:**
- Create: `crates/herdr-status-core/src/transport_source.rs`
- Modify: `crates/herdr-status-core/src/lib.rs`

A `TransportSource` turns a configured server into a `Transport`, possibly after first contact (WSL and SSH discovery), and can forget what it learned after a failure. `FallbackTransport` implements the Windows rule: if the named pipe reports "not running", try the local `herdr.exe … remote-api-bridge` once; if that works, keep using it.

- [ ] **Step 1: Write the failing tests**

`crates/herdr-status-core/src/transport_source.rs` (tests first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_connect::{ProcessRunner, RemoteCache, RunOutput};
    use herdr_client::transport::{Connection, StreamConnection, TransportError};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A transport that either fails with NotRunning or hands out a loopback connection.
    struct StubTransport {
        ok: bool,
        connects: AtomicUsize,
    }

    #[async_trait]
    impl Transport for StubTransport {
        async fn connect(&self) -> Result<Box<dyn Connection>, TransportError> {
            self.connects.fetch_add(1, Ordering::SeqCst);
            if self.ok {
                let (a, _b) = tokio::io::duplex(64);
                let (r, w) = tokio::io::split(a);
                Ok(Box::new(StreamConnection::new(r, w)))
            } else {
                Err(TransportError::NotRunning("stub".into()))
            }
        }
        fn describe(&self) -> String {
            if self.ok { "stub ok".into() } else { "stub down".into() }
        }
    }

    struct FakeRunner {
        outputs: Mutex<VecDeque<RunOutput>>,
        calls: Mutex<Vec<ChildCommand>>,
    }
    impl FakeRunner {
        fn new(outputs: Vec<RunOutput>) -> Arc<Self> {
            Arc::new(Self { outputs: Mutex::new(outputs.into()), calls: Mutex::new(Vec::new()) })
        }
        fn calls(&self) -> Vec<ChildCommand> {
            self.calls.lock().unwrap().clone()
        }
    }
    #[async_trait]
    impl ProcessRunner for FakeRunner {
        async fn run(&self, command: &ChildCommand, _timeout: Duration) -> std::io::Result<RunOutput> {
            self.calls.lock().unwrap().push(command.clone());
            Ok(self.outputs.lock().unwrap().pop_front().unwrap_or(RunOutput { exit_code: Some(1), stdout: String::new(), stderr: "unscripted".into() }))
        }
    }
    fn ok(stdout: &str) -> RunOutput {
        RunOutput { exit_code: Some(0), stdout: stdout.into(), stderr: String::new() }
    }

    #[tokio::test]
    async fn fixed_source_returns_the_same_transport() {
        let t: Arc<dyn Transport> = Arc::new(StubTransport { ok: true, connects: AtomicUsize::new(0) });
        let source = FixedSource(t.clone());
        assert_eq!(source.transport().await.unwrap().describe(), "stub ok");
    }

    #[tokio::test]
    async fn fallback_transport_switches_when_primary_is_not_running() {
        let primary = Arc::new(StubTransport { ok: false, connects: AtomicUsize::new(0) });
        let fallback = Arc::new(StubTransport { ok: true, connects: AtomicUsize::new(0) });
        let t = FallbackTransport::new(primary.clone(), fallback.clone());
        assert!(t.connect().await.is_ok());
        assert!(t.connect().await.is_ok());
        assert_eq!(primary.connects.load(Ordering::SeqCst), 1, "primary tried once, then skipped");
        assert_eq!(fallback.connects.load(Ordering::SeqCst), 2);
        assert_eq!(t.describe(), "stub ok (fallback)");
    }

    #[tokio::test]
    async fn fallback_transport_reports_primary_error_when_both_fail() {
        let primary = Arc::new(StubTransport { ok: false, connects: AtomicUsize::new(0) });
        let fallback = Arc::new(StubTransport { ok: false, connects: AtomicUsize::new(0) });
        let t = FallbackTransport::new(primary, fallback);
        assert!(matches!(t.connect().await, Err(TransportError::NotRunning(_))));
        assert_eq!(t.describe(), "stub down");
    }

    #[tokio::test]
    async fn wsl_source_discovers_once_and_builds_bridge_command() {
        let runner = FakeRunner::new(vec![ok("herdr-status-discovery:/home/me/.local/bin/herdr\n")]);
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
        let runner = FakeRunner::new(vec![RunOutput { exit_code: Some(2), stdout: String::new(), stderr: "herdr not found or too old on this host".into() }]);
        let source = WslSource::new(None, "default".into(), runner);
        let err = source.transport().await.unwrap_err();
        assert!(err.contains("herdr not found"), "{err}");
    }

    #[tokio::test]
    async fn remote_source_uses_cache_and_invalidates() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("cache.json");
        let runner = FakeRunner::new(vec![ok("Linux\nx86_64\n"), ok("herdr-status-discovery:/usr/local/bin/herdr\n")]);
        let cfg = RemoteConfig { label: "box".into(), ssh_target: "box".into(), session: "default".into(), os: None, herdr_path: None };
        let cache = Arc::new(Mutex::new(RemoteCache::default()));
        let source = RemoteSource::new(cfg, runner.clone(), SshOptions { client: Platform::Mac, control_dir: None }, cache.clone(), cache_path.clone());
        let t = source.transport().await.unwrap();
        assert_eq!(t.describe(), "ssh box (posix)");
        assert_eq!(runner.calls().len(), 2);
        assert!(cache_path.exists(), "cache saved to disk");
        let _ = source.transport().await.unwrap();
        assert_eq!(runner.calls().len(), 2, "second call served from cache");
        source.invalidate();
        assert!(cache.lock().unwrap().get("box").is_none());
        assert!(source.transport().await.is_err(), "re-probe with no scripted output fails");
    }
}
```

Add to `lib.rs`:

```rust
pub mod transport_source;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-status-core transport_source`
Expected: compile error.

- [ ] **Step 3: Implement the sources**

Above the tests in `transport_source.rs`:

```rust
//! How each server kind obtains its [`Transport`]. Remote and WSL sources run
//! first contact through a [`ProcessRunner`] and cache what they learn;
//! `invalidate` forgets it after a failure so the next attempt re-probes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use herdr_client::transport::bridge::{BridgeTransport, ChildCommand};
use herdr_client::transport::{Connection, Transport, TransportError};

use crate::config::RemoteConfig;
use crate::launchers::{self, SshOptions};
use crate::paths::Platform;
use crate::remote_connect::{bridge_transport_for, ProcessRunner, RemoteCache, RemoteResolver};
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
/// sticks with it. Used on Windows for named pipe → local `herdr.exe` bridge.
pub struct FallbackTransport {
    primary: Arc<dyn Transport>,
    fallback: Arc<dyn Transport>,
    use_fallback: AtomicBool,
}

impl FallbackTransport {
    pub fn new(primary: Arc<dyn Transport>, fallback: Arc<dyn Transport>) -> Self {
        Self { primary, fallback, use_fallback: AtomicBool::new(false) }
    }
}

#[async_trait]
impl Transport for FallbackTransport {
    async fn connect(&self) -> Result<Box<dyn Connection>, TransportError> {
        if self.use_fallback.load(Ordering::SeqCst) {
            return self.fallback.connect().await;
        }
        match self.primary.connect().await {
            Ok(conn) => Ok(conn),
            Err(TransportError::NotRunning(primary_reason)) => match self.fallback.connect().await {
                Ok(conn) => {
                    tracing::warn!(primary = %self.primary.describe(), fallback = %self.fallback.describe(), "primary transport unavailable; using fallback from now on");
                    self.use_fallback.store(true, Ordering::SeqCst);
                    Ok(conn)
                }
                Err(_) => Err(TransportError::NotRunning(primary_reason)),
            },
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
        Self { distro, session, runner, herdr_path: Mutex::new(None) }
    }

    fn label(&self) -> String {
        format!("wsl {}", self.distro.as_deref().unwrap_or("default"))
    }
}

#[async_trait]
impl TransportSource for WslSource {
    async fn transport(&self) -> Result<Arc<dyn Transport>, String> {
        let cached = self.herdr_path.lock().unwrap().clone();
        let path = match cached {
            Some(p) => p,
            None => {
                let command = launchers::wsl_command(self.distro.as_deref(), &launchers::posix_discovery_script(&self.session));
                let out = self.runner.run(&command, FIRST_CONTACT_TIMEOUT).await.map_err(|e| format!("wsl.exe failed: {e}"))?;
                let path = out
                    .success()
                    .then(|| parse_discovery_output(&out.stdout))
                    .flatten()
                    .ok_or_else(|| {
                        let msg = out.stderr.trim();
                        if msg.is_empty() { "herdr not found in WSL".to_string() } else { msg.lines().last().unwrap_or(msg).to_string() }
                    })?;
                *self.herdr_path.lock().unwrap() = Some(path.clone());
                path
            }
        };
        let command = launchers::wsl_command(self.distro.as_deref(), &launchers::posix_bridge_script(&path, &self.session));
        Ok(Arc::new(BridgeTransport::new(command, self.label())))
    }

    fn invalidate(&self) {
        *self.herdr_path.lock().unwrap() = None;
    }
}

/// A remote herdr over SSH, with first contact cached in `cache.json`.
pub struct RemoteSource {
    cfg: RemoteConfig,
    runner: Arc<dyn ProcessRunner>,
    ssh: SshOptions,
    cache: Arc<Mutex<RemoteCache>>,
    cache_path: PathBuf,
}

impl RemoteSource {
    pub fn new(cfg: RemoteConfig, runner: Arc<dyn ProcessRunner>, ssh: SshOptions, cache: Arc<Mutex<RemoteCache>>, cache_path: PathBuf) -> Self {
        Self { cfg, runner, ssh, cache, cache_path }
    }

    fn save_cache(&self) {
        if let Err(e) = self.cache.lock().unwrap().save(&self.cache_path) {
            tracing::warn!(error = %e, "could not save remote cache");
        }
    }
}

#[async_trait]
impl TransportSource for RemoteSource {
    async fn transport(&self) -> Result<Arc<dyn Transport>, String> {
        let cached = self.cache.lock().unwrap().get(&self.cfg.ssh_target).cloned();
        let entry = match cached {
            Some(entry) => entry,
            None => {
                let resolver = RemoteResolver { runner: self.runner.as_ref(), ssh: self.ssh.clone(), timeout: FIRST_CONTACT_TIMEOUT };
                let entry = resolver.resolve(&self.cfg).await.map_err(|e| e.to_string())?;
                self.cache.lock().unwrap().insert(&self.cfg.ssh_target, entry.clone());
                self.save_cache();
                entry
            }
        };
        Ok(Arc::new(bridge_transport_for(&self.cfg, &entry, &self.ssh)))
    }

    fn invalidate(&self) {
        self.cache.lock().unwrap().remove(&self.cfg.ssh_target);
        self.save_cache();
    }
}

#[allow(dead_code)]
fn _used(_: Platform, _: ChildCommand) {}
```

Drop the `_used` helper and trim imports once clippy shows which ones are only needed by the tests.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-status-core transport_source`
Expected: `6 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-status-core/src/transport_source.rs crates/herdr-status-core/src/lib.rs
git commit -m "feat(core): transport sources for local, wsl and remote servers with pipe fallback"
```

---

### Task 10: Watcher (`watcher.rs`)

**Files:**
- Create: `crates/herdr-status-core/src/watcher.rs`
- Create: `crates/herdr-status-core/tests/watcher_fake.rs`
- Modify: `crates/herdr-client/src/test_support.rs` (add `reset_responder`)
- Modify: `crates/herdr-status-core/src/lib.rs`

One task per server. Spec section 8: snapshot → subscribe (lifecycle + one entry per pane) → authoritative snapshot → event loop; rebuild on pane set changes or `events_lost`; reconcile on a timer; backoff on failure; rows kept while offline.

- [ ] **Step 1: Write the failing tests**

Add to `impl FakeHerdrServer` in `crates/herdr-client/src/test_support.rs`:

```rust
    /// Restore the built-in responder.
    pub fn reset_responder(&self) {
        *self.inner.responder.lock().unwrap() = default_responder();
    }
```

`crates/herdr-status-core/tests/watcher_fake.rs`:

```rust
#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use herdr_client::test_support::{fixture_snapshot_result, FakeHerdrServer, Reply};
use herdr_client::transport::unix::UnixSocketTransport;
use herdr_status_core::model::{AgentStatus, ConnectionState, ServerKind, ServerState};
use herdr_status_core::transport_source::FixedSource;
use herdr_status_core::watcher::{spawn_watcher, WatcherConfig, WatcherHandle, LIFECYCLE_SUBSCRIPTIONS};
use tokio::sync::mpsc;

fn test_cfg() -> WatcherConfig {
    WatcherConfig {
        reconcile: Duration::from_secs(60),
        debounce: Duration::from_millis(20),
        backoff_min: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        not_running_retry: Duration::from_millis(50),
        stable_after: Duration::from_secs(60),
    }
}

fn start(server: &FakeHerdrServer, cfg: WatcherConfig) -> (WatcherHandle, mpsc::Receiver<ServerState>) {
    let (tx, rx) = mpsc::channel(64);
    let source = Arc::new(FixedSource(Arc::new(server.transport())));
    let handle = spawn_watcher(ServerState::new("local", "local", ServerKind::Local), source, cfg, tx);
    (handle, rx)
}

async fn next_matching(rx: &mut mpsc::Receiver<ServerState>, pred: impl Fn(&ServerState) -> bool) -> ServerState {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let s = rx.recv().await.expect("watcher is alive");
            if pred(&s) {
                return s;
            }
        }
    })
    .await
    .expect("expected a matching state within 3s")
}

fn methods(server: &FakeHerdrServer) -> Vec<String> {
    server.requests().iter().map(|r| r["method"].as_str().unwrap().to_string()).collect()
}

async fn wait_for_subscribe_count(server: &FakeHerdrServer, n: usize) -> Vec<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let subs: Vec<_> = server.requests().into_iter().filter(|r| r["method"] == "events.subscribe").collect();
            if subs.len() >= n {
                return subs;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected subscribe requests")
}

/// Every state published during `window`; asserts the watcher stayed online.
async fn assert_stays_online(rx: &mut mpsc::Receiver<ServerState>, window: Duration) {
    let deadline = tokio::time::Instant::now() + window;
    while let Ok(Some(s)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        assert!(s.is_online(), "unexpected non-online state: {:?}", s.connection);
    }
}

#[tokio::test]
async fn goes_online_with_snapshot_rows_and_full_subscription() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    let online = next_matching(&mut rx, |s| s.is_online()).await;
    assert_eq!(online.rows.len(), 2);
    assert!(online.transport.contains("unix socket"));
    assert_eq!(methods(&server), vec!["session.snapshot", "events.subscribe", "session.snapshot"]);
    let subs = server.requests()[1]["params"]["subscriptions"].clone();
    let types: Vec<String> = subs.as_array().unwrap().iter().map(|s| s["type"].as_str().unwrap().to_string()).collect();
    for t in LIFECYCLE_SUBSCRIPTIONS {
        assert!(types.contains(&t.to_string()), "missing lifecycle subscription {t}");
    }
    let panes: Vec<&str> = subs.as_array().unwrap().iter().filter_map(|s| s["pane_id"].as_str()).collect();
    assert_eq!(panes, vec!["w1:p1", "w1:p2", "w2:p1"]);
    handle.shutdown().await;
}

#[tokio::test]
async fn status_event_updates_the_row() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    server.wait_for_subscribers(1).await;
    server.push_json(serde_json::json!({"event":"pane.agent_status_changed","data":{"pane_id":"w2:p1","workspace_id":"w2","agent_status":"blocked","agent":"claude"}}));
    let s = next_matching(&mut rx, |s| s.rows.iter().any(|r| r.pane_id == "w2:p1" && r.status == AgentStatus::Blocked)).await;
    let row = s.rows.iter().find(|r| r.pane_id == "w2:p1").unwrap();
    assert!(row.since.is_some());
    assert_eq!(row.flash_seq, 1);
    handle.shutdown().await;
}

#[tokio::test]
async fn pane_created_rebuilds_subscription_with_the_new_pane_without_going_offline() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    server.wait_for_subscribers(1).await;
    server.set_responder(|method, _| match method {
        "session.snapshot" => {
            let mut r = fixture_snapshot_result();
            r["snapshot"]["panes"].as_array_mut().unwrap().push(serde_json::json!({
                "pane_id":"w2:p2","terminal_id":"t9","workspace_id":"w2","tab_id":"w2:t1","focused":false,"agent_status":"unknown","revision":0
            }));
            Reply::Result(r)
        }
        "events.subscribe" => Reply::Result(serde_json::json!({"type":"subscription_started"})),
        other => Reply::Error { code: "invalid_request".into(), message: format!("unexpected {other}") },
    });
    server.push_json(serde_json::json!({"event":"pane_created","data":{"type":"pane_created","pane":{
        "pane_id":"w2:p2","terminal_id":"t9","workspace_id":"w2","tab_id":"w2:t1","focused":false,"agent_status":"unknown","revision":0}}}));
    let subs = wait_for_subscribe_count(&server, 2).await;
    assert!(subs[1]["params"]["subscriptions"].to_string().contains("w2:p2"));
    assert_stays_online(&mut rx, Duration::from_millis(300)).await;
    handle.shutdown().await;
}

#[tokio::test]
async fn events_lost_rebuilds_without_going_offline() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    server.wait_for_subscribers(1).await;
    server.push_line(r#"{"id":"hs-2","error":{"code":"events_lost","message":"subscriber fell behind"}}"#);
    wait_for_subscribe_count(&server, 2).await;
    assert_stays_online(&mut rx, Duration::from_millis(300)).await;
    handle.shutdown().await;
}

#[tokio::test]
async fn server_close_goes_offline_keeps_rows_and_recovers() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    server.wait_for_subscribers(1).await;
    server.set_responder(|_, _| Reply::CloseSilently);
    server.close_subscriptions();
    let off = next_matching(&mut rx, |s| matches!(s.connection, ConnectionState::Offline { .. })).await;
    assert_eq!(off.rows.len(), 2, "rows retained while offline");
    server.reset_responder();
    next_matching(&mut rx, |s| s.is_online()).await;
    handle.shutdown().await;
}

#[tokio::test]
async fn reconcile_timer_takes_periodic_snapshots() {
    let server = FakeHerdrServer::start().await;
    let mut cfg = test_cfg();
    cfg.reconcile = Duration::from_millis(100);
    let (handle, mut rx) = start(&server, cfg);
    next_matching(&mut rx, |s| s.is_online()).await;
    tokio::time::sleep(Duration::from_millis(450)).await;
    let snapshots = methods(&server).iter().filter(|m| *m == "session.snapshot").count();
    assert!(snapshots >= 5, "expected initial 2 plus at least 3 reconciles, got {snapshots}");
    handle.shutdown().await;
}

#[tokio::test]
async fn reconnect_command_resubscribes() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    handle.reconnect();
    wait_for_subscribe_count(&server, 2).await;
    handle.shutdown().await;
}

#[tokio::test]
async fn not_running_server_reports_reason_and_keeps_retrying() {
    let dir = tempfile::tempdir().unwrap();
    let transport: Arc<dyn herdr_client::Transport> = Arc::new(UnixSocketTransport::new(dir.path().join("absent.sock")));
    let (tx, mut rx) = mpsc::channel(64);
    let handle = spawn_watcher(ServerState::new("local", "local", ServerKind::Local), Arc::new(FixedSource(transport)), test_cfg(), tx);
    let off = next_matching(&mut rx, |s| matches!(s.connection, ConnectionState::Offline { .. })).await;
    assert!(matches!(&off.connection, ConnectionState::Offline { reason, .. } if reason == "herdr is not running"));
    let again = next_matching(&mut rx, |s| matches!(s.connection, ConnectionState::Offline { .. })).await;
    assert!(matches!(again.connection, ConnectionState::Offline { .. }), "retries keep reporting offline");
    handle.shutdown().await;
}

#[tokio::test]
async fn shutdown_ends_the_task_promptly() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    tokio::time::timeout(Duration::from_secs(1), handle.shutdown()).await.expect("shutdown within 1s");
}
```

Add to `lib.rs`:

```rust
pub mod watcher;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-status-core --test watcher_fake`
Expected: compile error, `herdr_status_core::watcher` not found.

- [ ] **Step 3: Implement the watcher**

`crates/herdr-status-core/src/watcher.rs`:

```rust
//! One task per server: keeps a subscription open, reconciles with snapshots,
//! rebuilds when the pane set changes, backs off on failure.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use herdr_client::transport::TransportError;
use herdr_client::{Client, ClientError, Event, Subscription};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::model::ServerState;
use crate::reducer::{apply_snapshot, apply_status, set_connecting, set_offline, set_online};
use crate::transport_source::TransportSource;

/// Parameterless subscriptions requested on every connection.
pub const LIFECYCLE_SUBSCRIPTIONS: [&str; 13] = [
    "pane.created",
    "pane.closed",
    "pane.updated",
    "pane.exited",
    "pane.agent_detected",
    "workspace.created",
    "workspace.closed",
    "workspace.renamed",
    "workspace.updated",
    "workspace.metadata_updated",
    "tab.created",
    "tab.closed",
    "tab.renamed",
];

pub fn subscriptions_for(pane_ids: &[String]) -> Vec<Subscription> {
    LIFECYCLE_SUBSCRIPTIONS
        .iter()
        .map(|t| Subscription::lifecycle(t))
        .chain(pane_ids.iter().map(|p| Subscription::pane_agent_status(p)))
        .collect()
}

#[derive(Debug, Clone)]
pub struct WatcherConfig {
    pub reconcile: Duration,
    pub debounce: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// Retry interval when the server is simply not running (local socket missing).
    pub not_running_retry: Duration,
    /// Once online this long, the next failure starts again at `backoff_min`.
    pub stable_after: Duration,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            reconcile: Duration::from_secs(30),
            debounce: Duration::from_millis(250),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            not_running_retry: Duration::from_secs(5),
            stable_after: Duration::from_secs(60),
        }
    }
}

impl WatcherConfig {
    pub fn with_reconcile_seconds(mut self, seconds: u64) -> Self {
        self.reconcile = Duration::from_secs(seconds.max(5));
        self
    }
}

#[derive(Debug)]
pub enum WatcherCommand {
    Reconnect,
    Shutdown,
}

pub struct WatcherHandle {
    commands: mpsc::Sender<WatcherCommand>,
    task: JoinHandle<()>,
}

impl WatcherHandle {
    /// Drop the current connection and reconnect now, resetting backoff.
    pub fn reconnect(&self) {
        let _ = self.commands.try_send(WatcherCommand::Reconnect);
    }

    pub async fn shutdown(self) {
        let _ = self.commands.send(WatcherCommand::Shutdown).await;
        let _ = self.task.await;
    }
}

pub fn spawn_watcher(
    state: ServerState,
    source: Arc<dyn TransportSource>,
    cfg: WatcherConfig,
    updates: mpsc::Sender<ServerState>,
) -> WatcherHandle {
    let (commands, rx) = mpsc::channel(8);
    let backoff = cfg.backoff_min;
    let watcher = Watcher { state, source, cfg, updates, commands: rx, backoff };
    let task = tokio::spawn(watcher.run());
    WatcherHandle { commands, task }
}

/// User-facing text for a failure (spec section 14).
pub fn reason_text(error: &ClientError) -> String {
    match error {
        ClientError::Transport(TransportError::NotRunning(_)) => "herdr is not running".to_string(),
        ClientError::Transport(TransportError::Launch(msg)) => msg.clone(),
        ClientError::Transport(TransportError::Timeout(_)) => "bridge did not start".to_string(),
        ClientError::Transport(TransportError::Io(e)) => format!("connection error: {e}"),
        ClientError::Api(body) => format!("herdr error {}: {}", body.code, body.message),
        ClientError::Protocol(msg) => msg.clone(),
    }
}

struct Watcher {
    state: ServerState,
    source: Arc<dyn TransportSource>,
    cfg: WatcherConfig,
    updates: mpsc::Sender<ServerState>,
    commands: mpsc::Receiver<WatcherCommand>,
    backoff: Duration,
}

/// Why a session ended.
enum SessionEnd {
    /// Pane set changed or events were lost: resubscribe without touching the chip.
    Rebuild,
    /// User asked for a reconnect.
    Reconnect,
    Failed { reason: String, not_running: bool },
}

/// The receiver is gone or Shutdown was requested.
struct Stop;

impl Watcher {
    async fn run(mut self) {
        let mut planned_rebuild = false;
        loop {
            if !planned_rebuild {
                set_connecting(&mut self.state);
                if self.publish().await.is_err() {
                    return;
                }
            }
            planned_rebuild = false;
            match self.run_session().await {
                Err(Stop) => return,
                Ok(SessionEnd::Rebuild) => planned_rebuild = true,
                Ok(SessionEnd::Reconnect) => self.backoff = self.cfg.backoff_min,
                Ok(SessionEnd::Failed { reason, not_running }) => {
                    let wait = if not_running { self.cfg.not_running_retry } else { self.backoff };
                    tracing::info!(server = %self.state.id, %reason, ?wait, "server offline");
                    set_offline(&mut self.state, reason, SystemTime::now() + wait);
                    if self.publish().await.is_err() {
                        return;
                    }
                    if !not_running {
                        self.backoff = (self.backoff * 2).min(self.cfg.backoff_max);
                        self.source.invalidate();
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        cmd = self.commands.recv() => match cmd {
                            Some(WatcherCommand::Reconnect) => self.backoff = self.cfg.backoff_min,
                            Some(WatcherCommand::Shutdown) | None => return,
                        },
                    }
                }
            }
        }
    }

    async fn publish(&self) -> Result<(), Stop> {
        self.updates.send(self.state.clone()).await.map_err(|_| Stop)
    }

    fn failed(&self, error: ClientError) -> SessionEnd {
        let not_running = matches!(error, ClientError::Transport(TransportError::NotRunning(_)));
        SessionEnd::Failed { reason: reason_text(&error), not_running }
    }

    async fn run_session(&mut self) -> Result<SessionEnd, Stop> {
        let transport = match self.source.transport().await {
            Ok(t) => t,
            Err(reason) => return Ok(SessionEnd::Failed { reason, not_running: false }),
        };
        let client = Client::new(transport.clone());

        let first = match client.snapshot().await {
            Ok(s) => s,
            Err(e) => return Ok(self.failed(e)),
        };
        let pane_ids: Vec<String> = first.pane_ids().iter().map(|p| p.to_string()).collect();
        let mut stream = match client.subscribe(subscriptions_for(&pane_ids)).await {
            Ok(s) => s,
            Err(e) => return Ok(self.failed(e)),
        };
        let authoritative = match client.snapshot().await {
            Ok(s) => s,
            Err(e) => return Ok(self.failed(e)),
        };
        apply_snapshot(&mut self.state, &authoritative, SystemTime::now());
        set_online(&mut self.state, &transport.describe());
        self.publish().await?;
        let online_since = Instant::now();

        let mut reconcile = tokio::time::interval(self.cfg.reconcile);
        reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        reconcile.tick().await; // the immediate first tick
        let mut dirty: Option<(Instant, bool)> = None; // (deadline, rebuild)

        loop {
            let dirty_deadline = dirty.map(|(d, _)| d);
            tokio::select! {
                event = stream.next() => match event {
                    Ok(Some(Event::PaneAgentStatusChanged(e))) => {
                        if apply_status(&mut self.state, &e, SystemTime::now()) {
                            self.publish().await?;
                        } else {
                            dirty = Some((Instant::now() + self.cfg.debounce, dirty.is_some_and(|(_, r)| r)));
                        }
                    }
                    Ok(Some(Event::PaneCreated { .. } | Event::PaneClosed { .. } | Event::PaneExited { .. } | Event::PaneAgentDetected { .. })) => {
                        dirty = Some((Instant::now() + self.cfg.debounce, true));
                    }
                    Ok(Some(Event::PaneUpdated { .. } | Event::Other)) => {
                        if dirty.is_none() {
                            dirty = Some((Instant::now() + self.cfg.debounce, false));
                        }
                    }
                    Ok(None) => return Ok(SessionEnd::Failed { reason: "connection closed".into(), not_running: false }),
                    Err(e) => {
                        if e.api_code() == Some("events_lost") {
                            tracing::info!(server = %self.state.id, "events lost; rebuilding subscription");
                            return Ok(SessionEnd::Rebuild);
                        }
                        self.reset_backoff_if_stable(online_since);
                        return Ok(self.failed(e));
                    }
                },
                _ = sleep_until_opt(dirty_deadline) => {
                    let (_, rebuild) = dirty.take().expect("dirty is set when its sleep fires");
                    if rebuild {
                        return Ok(SessionEnd::Rebuild);
                    }
                    if let Some(end) = self.resnapshot(&client, online_since).await? {
                        return Ok(end);
                    }
                }
                _ = reconcile.tick() => {
                    if let Some(end) = self.resnapshot(&client, online_since).await? {
                        return Ok(end);
                    }
                }
                cmd = self.commands.recv() => match cmd {
                    Some(WatcherCommand::Reconnect) => return Ok(SessionEnd::Reconnect),
                    Some(WatcherCommand::Shutdown) | None => return Err(Stop),
                },
            }
        }
    }

    /// Take a snapshot and publish. `Ok(Some(end))` when the server failed.
    async fn resnapshot(&mut self, client: &Client, online_since: Instant) -> Result<Option<SessionEnd>, Stop> {
        match client.snapshot().await {
            Ok(snapshot) => {
                apply_snapshot(&mut self.state, &snapshot, SystemTime::now());
                self.publish().await?;
                Ok(None)
            }
            Err(e) => {
                self.reset_backoff_if_stable(online_since);
                Ok(Some(self.failed(e)))
            }
        }
    }

    fn reset_backoff_if_stable(&mut self, online_since: Instant) {
        if online_since.elapsed() >= self.cfg.stable_after {
            self.backoff = self.cfg.backoff_min;
        }
    }
}

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending::<()>().await,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-status-core --test watcher_fake`
Expected: `9 passed`. These tests use real time with small durations; if one is flaky on a loaded machine, raise the 3 s timeouts in the helpers rather than loosening assertions.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-status-core/src/watcher.rs crates/herdr-status-core/tests/watcher_fake.rs crates/herdr-status-core/src/lib.rs crates/herdr-client/src/test_support.rs
git commit -m "feat(core): per-server watcher with subscribe, rebuild, reconcile and backoff"
```

---

### Task 11: Hub (`hub.rs`)

**Files:**
- Create: `crates/herdr-status-core/src/hub.rs`
- Create: `crates/herdr-status-core/tests/hub_fake.rs`
- Modify: `crates/herdr-status-core/src/lib.rs`

The hub turns a `Config` into running watchers, merges their `ServerState`s into one `AgentList`, and hands out `Client`s for quick actions. A `SourceFactory` closure decides how a server gets its transport so tests can plug in the fake server.

- [ ] **Step 1: Write the failing tests**

`crates/herdr-status-core/tests/hub_fake.rs`:

```rust
#![cfg(unix)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use herdr_client::test_support::FakeHerdrServer;
use herdr_client::{Method, ResponseResult};
use herdr_status_core::config::Config;
use herdr_status_core::hub::{specs_from_config, Hub, ServerSpec, SourceFactory};
use herdr_status_core::model::{AgentList, ServerKind};
use herdr_status_core::paths::Platform;
use herdr_status_core::transport_source::{FixedSource, TransportSource};
use herdr_status_core::watcher::WatcherConfig;

fn fast() -> WatcherConfig {
    WatcherConfig {
        reconcile: Duration::from_secs(60),
        debounce: Duration::from_millis(20),
        backoff_min: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        not_running_retry: Duration::from_millis(50),
        stable_after: Duration::from_secs(60),
    }
}

/// Maps server ids to fake servers; unknown ids get a transport to a missing socket.
fn factory(map: HashMap<String, Arc<FakeHerdrServer>>) -> SourceFactory {
    let map = Arc::new(Mutex::new(map));
    Arc::new(move |spec: &ServerSpec| -> Arc<dyn TransportSource> {
        let guard = map.lock().unwrap();
        match guard.get(spec.id()) {
            Some(server) => Arc::new(FixedSource(Arc::new(server.transport()))),
            None => {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("absent.sock");
                std::mem::forget(dir);
                Arc::new(FixedSource(Arc::new(herdr_client::transport::unix::UnixSocketTransport::new(path))))
            }
        }
    })
}

async fn wait_list(rx: &mut tokio::sync::watch::Receiver<AgentList>, pred: impl Fn(&AgentList) -> bool) -> AgentList {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if pred(&rx.borrow()) {
                return rx.borrow().clone();
            }
            rx.changed().await.expect("hub alive");
        }
    })
    .await
    .expect("expected list within 3s")
}

#[tokio::test]
async fn apply_config_starts_local_and_publishes_rows() {
    let local = Arc::new(FakeHerdrServer::start().await);
    let mut hub = Hub::new(factory(HashMap::from([("local".to_string(), local.clone())])), fast());
    let mut rx = hub.subscribe();
    hub.apply_config(&Config::parse("").unwrap(), Platform::Mac).await;
    let list = wait_list(&mut rx, |l| l.rows.len() == 2).await;
    assert_eq!(list.servers.len(), 1);
    assert_eq!(list.servers[0].id, "local");
    assert_eq!(list.servers[0].kind, ServerKind::Local);
    assert_eq!(list.counts.working, 1);
    hub.shutdown().await;
}

#[tokio::test]
async fn config_changes_add_and_remove_servers() {
    let local = Arc::new(FakeHerdrServer::start().await);
    let remote = Arc::new(FakeHerdrServer::start().await);
    let mut hub = Hub::new(
        factory(HashMap::from([("local".to_string(), local.clone()), ("box".to_string(), remote.clone())])),
        fast(),
    );
    let mut rx = hub.subscribe();
    let with_remote = Config::parse("[[remote]]\nlabel = \"box\"\nssh_target = \"box\"\n").unwrap();
    hub.apply_config(&with_remote, Platform::Mac).await;
    let list = wait_list(&mut rx, |l| l.servers.len() == 2 && l.rows.len() == 4).await;
    assert_eq!(list.servers[1].id, "box");
    assert_eq!(list.servers[1].kind, ServerKind::Remote);
    assert!(list.rows.iter().any(|r| r.server_id == "box"));

    hub.apply_config(&Config::parse("").unwrap(), Platform::Mac).await;
    let list = wait_list(&mut rx, |l| l.servers.len() == 1).await;
    assert!(list.rows.iter().all(|r| r.server_id == "local"));
    assert_eq!(list.rows.len(), 2);
    hub.shutdown().await;
}

#[tokio::test]
async fn unchanged_servers_are_not_restarted() {
    let local = Arc::new(FakeHerdrServer::start().await);
    let mut hub = Hub::new(factory(HashMap::from([("local".to_string(), local.clone())])), fast());
    let mut rx = hub.subscribe();
    let cfg = Config::parse("").unwrap();
    hub.apply_config(&cfg, Platform::Mac).await;
    wait_list(&mut rx, |l| l.rows.len() == 2).await;
    let before = local.requests().len();
    hub.apply_config(&cfg, Platform::Mac).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(local.requests().len(), before, "same config must not reconnect");
    hub.shutdown().await;
}

#[tokio::test]
async fn reconnect_all_and_client_for() {
    let local = Arc::new(FakeHerdrServer::start().await);
    let mut hub = Hub::new(factory(HashMap::from([("local".to_string(), local.clone())])), fast());
    let mut rx = hub.subscribe();
    hub.apply_config(&Config::parse("").unwrap(), Platform::Mac).await;
    wait_list(&mut rx, |l| l.rows.len() == 2).await;
    hub.reconnect_all();
    tokio::time::timeout(Duration::from_secs(3), async {
        while local.requests().iter().filter(|r| r["method"] == "events.subscribe").count() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("resubscribed");
    let client = hub.client_for("local").await.expect("client for local");
    assert!(matches!(client.request(Method::Ping).await.unwrap(), ResponseResult::Pong { .. }));
    assert!(hub.client_for("nope").await.is_none());
    hub.shutdown().await;
}

#[test]
fn specs_follow_platform_and_enabled_flag() {
    let cfg = Config::parse("[[wsl]]\nlabel = \"u\"\n[[remote]]\nlabel = \"box\"\nssh_target = \"box\"\n").unwrap();
    let mac: Vec<String> = specs_from_config(&cfg, Platform::Mac).iter().map(|s| s.id().to_string()).collect();
    assert_eq!(mac, vec!["local", "box"], "wsl entries are ignored off Windows");
    let win: Vec<String> = specs_from_config(&cfg, Platform::Windows).iter().map(|s| s.id().to_string()).collect();
    assert_eq!(win, vec!["local", "u", "box"]);
    let off = Config::parse("[local]\nenabled = false\n").unwrap();
    assert!(specs_from_config(&off, Platform::Mac).is_empty());
}
```

Add to `lib.rs`:

```rust
pub mod hub;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-status-core --test hub_fake`
Expected: compile error.

- [ ] **Step 3: Implement the hub**

`crates/herdr-status-core/src/hub.rs`:

```rust
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
use crate::transport_source::{FallbackTransport, FixedSource, RemoteSource, TransportSource, WslSource};
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
pub fn default_source_factory(env: Env, platform: Platform, runner: Arc<dyn ProcessRunner>, data_dir: PathBuf) -> SourceFactory {
    let cache_path = data_dir.join("cache.json");
    let cache = Arc::new(Mutex::new(RemoteCache::load(&cache_path)));
    let control_dir = data_dir.join("cm");
    Arc::new(move |spec: &ServerSpec| -> Arc<dyn TransportSource> {
        match spec {
            ServerSpec::Local(local) => {
                let path = local
                    .socket_path
                    .clone()
                    .unwrap_or_else(|| paths::herdr_socket_path(&env, platform, &local.session));
                Arc::new(FixedSource(local_transport(platform, &path, &local.session)))
            }
            ServerSpec::Wsl(wsl) => Arc::new(WslSource::new(wsl.distro.clone(), wsl.session.clone(), runner.clone())),
            ServerSpec::Remote(remote) => {
                let _ = std::fs::create_dir_all(&control_dir);
                let ssh = SshOptions { client: platform, control_dir: Some(control_dir.clone()) };
                Arc::new(RemoteSource::new(remote.clone(), runner.clone(), ssh, cache.clone(), cache_path.clone()))
            }
        }
    })
}

fn local_transport(platform: Platform, socket_path: &std::path::Path, session: &str) -> Arc<dyn Transport> {
    match platform {
        Platform::Mac => Arc::new(herdr_client::transport::unix::UnixSocketTransport::new(socket_path)),
        Platform::Windows => {
            let pipe: Arc<dyn Transport> = windows_pipe_transport(&socket_path.to_string_lossy());
            let fallback: Arc<dyn Transport> = Arc::new(BridgeTransport::new(
                launchers::local_herdr_bridge_command("herdr.exe", session),
                "local herdr.exe bridge",
            ));
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
    Arc::new(herdr_client::transport::unix::UnixSocketTransport::new(socket_path))
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
        Self { factory, watcher_cfg, running: HashMap::new(), order, states, list_tx, updates_tx, merge_task }
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
        let watcher_cfg = self.watcher_cfg.clone().with_reconcile_seconds(cfg.ui.reconcile_seconds);
        self.watcher_cfg = watcher_cfg.clone();

        let stale: Vec<String> = self
            .running
            .iter()
            .filter(|(id, r)| wanted.get(id.as_str()).map_or(true, |spec| **spec != r.spec))
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
            let handle = spawn_watcher(state, source.clone(), watcher_cfg.clone(), self.updates_tx.clone());
            self.running.insert(spec.id().to_string(), Running { spec: spec.clone(), source, handle });
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

fn publish(order: &Mutex<Vec<String>>, states: &Mutex<HashMap<String, ServerState>>, list_tx: &watch::Sender<AgentList>) {
    let order = order.lock().unwrap();
    let states = states.lock().unwrap();
    let ordered: Vec<ServerState> = order.iter().filter_map(|id| states.get(id).cloned()).collect();
    let _ = list_tx.send(assemble(&ordered));
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-status-core --test hub_fake`
Expected: `5 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-status-core/src/hub.rs crates/herdr-status-core/tests/hub_fake.rs crates/herdr-status-core/src/lib.rs
git commit -m "feat(core): hub that runs watchers from config and publishes the merged agent list"
```

---

### Task 12: Quick actions (`actions.rs`)

**Files:**
- Create: `crates/herdr-status-core/src/actions.rs`
- Create: `crates/herdr-status-core/tests/actions_fake.rs`
- Modify: `crates/herdr-status-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

`crates/herdr-status-core/tests/actions_fake.rs`:

```rust
#![cfg(unix)]

use std::sync::Arc;

use herdr_client::test_support::{FakeHerdrServer, Reply};
use herdr_client::Client;
use herdr_status_core::actions::{peek, send_key, send_message, KEYPAD};

async fn client(server: &FakeHerdrServer) -> Client {
    Client::new(Arc::new(server.transport()))
}

#[tokio::test]
async fn peek_reads_recent_unwrapped_text() {
    let server = FakeHerdrServer::start().await;
    let read = peek(&client(&server).await, "w2:p1", 30, "local").await.unwrap();
    assert_eq!(read.text, "$ echo hi\nhi");
    let req = &server.requests()[0];
    assert_eq!(req["method"], "agent.read");
    assert_eq!(req["params"]["target"], "w2:p1");
    assert_eq!(req["params"]["lines"], 30);
    assert_eq!(req["params"]["source"], "recent_unwrapped");
}

#[tokio::test]
async fn send_key_validates_against_keypad_before_calling() {
    let server = FakeHerdrServer::start().await;
    let c = client(&server).await;
    for key in KEYPAD {
        send_key(&c, "w2:p1", key, "local").await.unwrap();
    }
    assert_eq!(server.requests().len(), KEYPAD.len());
    assert_eq!(server.requests()[9]["params"]["keys"], serde_json::json!(["ctrl+c"]));
    let err = send_key(&c, "w2:p1", "rm -rf", "local").await.unwrap_err();
    assert_eq!(err.code, "invalid_key");
    assert_eq!(server.requests().len(), KEYPAD.len(), "no request for a rejected key");
}

#[tokio::test]
async fn send_message_trims_and_rejects_empty() {
    let server = FakeHerdrServer::start().await;
    let c = client(&server).await;
    send_message(&c, "w2:p1", "  continue  ", "local").await.unwrap();
    assert_eq!(server.requests()[0]["params"]["text"], "continue");
    let err = send_message(&c, "w2:p1", "   ", "local").await.unwrap_err();
    assert_eq!(err.code, "empty_message");
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn herdr_errors_get_friendly_text() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|method, _| match method {
        "agent.prompt" => Reply::Error { code: "agent_blocked".into(), message: "waiting".into() },
        "agent.read" => Reply::Error { code: "not_found".into(), message: "gone".into() },
        _ => Reply::Error { code: "weird".into(), message: "something else".into() },
    });
    let c = client(&server).await;
    let blocked = send_message(&c, "w2:p1", "hi", "local").await.unwrap_err();
    assert_eq!(blocked.code, "agent_blocked");
    assert_eq!(blocked.message, "Waiting at a prompt. Answer it with the keypad first.");
    let gone = peek(&c, "w2:p1", 10, "local").await.unwrap_err();
    assert_eq!(gone.message, "That agent is gone.");
    let other = send_key(&c, "w2:p1", "enter", "local").await.unwrap_err();
    assert_eq!(other.code, "weird");
    assert_eq!(other.message, "something else");
}

#[tokio::test]
async fn transport_failure_names_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let c = Client::new(Arc::new(herdr_client::transport::unix::UnixSocketTransport::new(dir.path().join("none.sock"))));
    let err = peek(&c, "w2:p1", 10, "winbox").await.unwrap_err();
    assert_eq!(err.code, "transport");
    assert_eq!(err.message, "Could not reach winbox.");
}
```

Add to `lib.rs`:

```rust
pub mod actions;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-status-core --test actions_fake`
Expected: compile error.

- [ ] **Step 3: Implement the actions**

`crates/herdr-status-core/src/actions.rs`:

```rust
//! The three quick actions, with the keypad allow-list and friendly errors.

use herdr_client::{Client, ClientError, Method, ReadResult, ResponseResult};
use serde::Serialize;

/// Keys the UI keypad may send, in display order.
pub const KEYPAD: [&str; 10] = ["1", "2", "3", "y", "n", "up", "down", "enter", "esc", "ctrl+c"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionError {
    pub code: String,
    pub message: String,
}

impl ActionError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), message: message.into() }
    }
}

/// Map a client error to what the drawer shows (spec section 10).
pub fn friendly(error: &ClientError, server_label: &str) -> ActionError {
    match error {
        ClientError::Api(body) => match body.code.as_str() {
            "agent_blocked" => ActionError::new("agent_blocked", "Waiting at a prompt. Answer it with the keypad first."),
            "not_found" => ActionError::new("not_found", "That agent is gone."),
            other => ActionError::new(other, body.message.clone()),
        },
        ClientError::Transport(_) => ActionError::new("transport", format!("Could not reach {server_label}.")),
        ClientError::Protocol(msg) => ActionError::new("protocol", format!("Unexpected reply from herdr: {msg}")),
    }
}

pub async fn peek(client: &Client, pane_id: &str, lines: u32, server_label: &str) -> Result<ReadResult, ActionError> {
    match client.request(Method::AgentRead { target: pane_id.to_string(), lines }).await {
        Ok(ResponseResult::PaneRead { read }) => Ok(read),
        Ok(other) => Err(ActionError::new("protocol", format!("Unexpected reply from herdr: {other:?}"))),
        Err(e) => Err(friendly(&e, server_label)),
    }
}

pub async fn send_key(client: &Client, pane_id: &str, key: &str, server_label: &str) -> Result<(), ActionError> {
    if !KEYPAD.contains(&key) {
        return Err(ActionError::new("invalid_key", format!("'{key}' is not a keypad key")));
    }
    client
        .request(Method::AgentSendKeys { target: pane_id.to_string(), keys: vec![key.to_string()] })
        .await
        .map(|_| ())
        .map_err(|e| friendly(&e, server_label))
}

pub async fn send_message(client: &Client, pane_id: &str, text: &str, server_label: &str) -> Result<(), ActionError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ActionError::new("empty_message", "Type a message first."));
    }
    client
        .request(Method::AgentPrompt { target: pane_id.to_string(), text: text.to_string() })
        .await
        .map(|_| ())
        .map_err(|e| friendly(&e, server_label))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-status-core --test actions_fake`
Expected: `5 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-status-core/src/actions.rs crates/herdr-status-core/tests/actions_fake.rs crates/herdr-status-core/src/lib.rs
git commit -m "feat(core): peek, send-key and send-message actions with friendly errors"
```

---

### Task 13: First-launch prefill (`prefill.rs`)

**Files:**
- Create: `crates/herdr-status-core/src/prefill.rs`
- Modify: `crates/herdr-status-core/src/lib.rs`

Spec section 13: on first launch, remotes come from `herdr machine list --json` (rows `{id,label,target,session,enabled,selected}`); on Windows a WSL probe may add a `[[wsl]]`, and `[local].enabled` becomes `false` when no native herdr exists.

- [ ] **Step 1: Write the failing tests**

`crates/herdr-status-core/src/prefill.rs` (tests first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_connect::RunOutput;
    use herdr_client::transport::bridge::ChildCommand;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Duration;

    const MACHINES: &str = r#"[{"id":"0123456789abcdef0123456789abcdef","label":"winbox","target":"me@winbox","session":"default","enabled":true,"selected":false},
{"id":"fedcba9876543210fedcba9876543210","label":"old","target":"old","session":"agents","enabled":false,"selected":false},
{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","label":"local","target":"localhost","session":"","enabled":true,"selected":true}]"#;

    #[test]
    fn parses_machine_list_and_skips_disabled() {
        let rows = parse_machine_list(MACHINES);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "winbox");
        assert!(parse_machine_list("not json").is_empty());
        assert!(parse_machine_list("[]").is_empty());
    }

    #[test]
    fn remotes_from_machines_maps_fields_and_avoids_reserved_label() {
        let remotes = remotes_from_machines(&parse_machine_list(MACHINES));
        assert_eq!(remotes[0].label, "winbox");
        assert_eq!(remotes[0].ssh_target, "me@winbox");
        assert_eq!(remotes[0].session, "default");
        assert_eq!(remotes[1].label, "local-aaaaaa", "reserved label gets an id suffix");
        assert_eq!(remotes[1].session, "default", "empty session means default");
    }

    #[test]
    fn prefill_on_mac_adds_remotes_only() {
        let inputs = PrefillInputs { machine_list_json: Some(MACHINES.into()), wsl_socket_present: true, native_herdr_present: true };
        let cfg = prefill_config(Platform::Mac, &inputs);
        assert!(cfg.local.enabled);
        assert!(cfg.wsl.is_empty(), "wsl never prefilled on mac");
        assert_eq!(cfg.remote.len(), 2);
        cfg.validate().unwrap();
    }

    #[test]
    fn prefill_on_windows_adds_wsl_and_disables_local_without_native_herdr() {
        let inputs = PrefillInputs { machine_list_json: None, wsl_socket_present: true, native_herdr_present: false };
        let cfg = prefill_config(Platform::Windows, &inputs);
        assert!(!cfg.local.enabled);
        assert_eq!(cfg.wsl.len(), 1);
        assert_eq!(cfg.wsl[0].label, "wsl");
        assert_eq!(cfg.wsl[0].distro, None);
        let native = PrefillInputs { machine_list_json: None, wsl_socket_present: false, native_herdr_present: true };
        let cfg = prefill_config(Platform::Windows, &native);
        assert!(cfg.local.enabled);
        assert!(cfg.wsl.is_empty());
    }

    #[test]
    fn render_uses_default_text_when_nothing_was_found() {
        assert_eq!(render_prefilled(&Config::default()), Config::DEFAULT_TOML);
        let mut cfg = Config::default();
        cfg.remote.push(RemoteConfig { label: "box".into(), ssh_target: "box".into(), session: "default".into(), os: None, herdr_path: None });
        let text = render_prefilled(&cfg);
        assert!(text.starts_with("# herdr-status configuration"));
        assert!(text.contains("[[remote]]"));
        assert_eq!(Config::parse(&text).unwrap(), cfg, "rendered text round-trips");
    }

    struct FakeRunner {
        outputs: Mutex<VecDeque<RunOutput>>,
        calls: Mutex<Vec<ChildCommand>>,
    }
    #[async_trait::async_trait]
    impl ProcessRunner for FakeRunner {
        async fn run(&self, command: &ChildCommand, _t: Duration) -> std::io::Result<RunOutput> {
            self.calls.lock().unwrap().push(command.clone());
            Ok(self.outputs.lock().unwrap().pop_front().unwrap_or(RunOutput { exit_code: Some(1), stdout: String::new(), stderr: String::new() }))
        }
    }

    #[tokio::test]
    async fn gather_inputs_runs_the_expected_commands() {
        let runner = FakeRunner {
            outputs: Mutex::new(VecDeque::from(vec![
                RunOutput { exit_code: Some(0), stdout: "[]".into(), stderr: String::new() },
                RunOutput { exit_code: Some(0), stdout: String::new(), stderr: String::new() },
                RunOutput { exit_code: Some(0), stdout: "herdr 0.9.3".into(), stderr: String::new() },
            ])),
            calls: Mutex::new(Vec::new()),
        };
        let env = Env::from_pairs(vec![("APPDATA".to_string(), "/nonexistent".to_string())]);
        let inputs = gather_inputs(&runner, Platform::Windows, &env).await;
        assert_eq!(inputs.machine_list_json.as_deref(), Some("[]"));
        assert!(inputs.wsl_socket_present);
        assert!(inputs.native_herdr_present);
        let calls = runner.calls.lock().unwrap().clone();
        assert_eq!(calls[0].program, "herdr.exe");
        assert_eq!(calls[0].args, vec!["machine", "list", "--json"]);
        assert_eq!(calls[1].program, "wsl.exe");
        assert!(calls[1].args.last().unwrap().starts_with("test -S "));
        assert_eq!(calls[2].args, vec!["--version"]);

        let mac_runner = FakeRunner { outputs: Mutex::new(VecDeque::new()), calls: Mutex::new(Vec::new()) };
        let inputs = gather_inputs(&mac_runner, Platform::Mac, &Env::default()).await;
        assert!(inputs.machine_list_json.is_none(), "failed command gives None");
        assert!(!inputs.wsl_socket_present);
        assert!(inputs.native_herdr_present, "not probed on mac");
        assert_eq!(mac_runner.calls.lock().unwrap().len(), 1, "only the machine list on mac");
    }
}
```

Add to `lib.rs`:

```rust
pub mod prefill;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-status-core prefill`
Expected: compile error.

- [ ] **Step 3: Implement prefill**

Above the tests in `prefill.rs`:

```rust
//! First-launch configuration prefill (spec section 13).

use std::time::Duration;

use herdr_client::transport::bridge::ChildCommand;
use serde::Deserialize;

use crate::config::{Config, RemoteConfig, WslConfig, LOCAL_SERVER_ID};
use crate::launchers;
use crate::paths::{self, Env, Platform};
use crate::remote_connect::ProcessRunner;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// One row of `herdr machine list --json`.
#[derive(Debug, Clone, Deserialize)]
pub struct MachineRow {
    pub id: String,
    pub label: String,
    pub target: String,
    #[serde(default)]
    pub session: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Enabled rows only; anything unparseable is an empty list.
pub fn parse_machine_list(json: &str) -> Vec<MachineRow> {
    serde_json::from_str::<Vec<MachineRow>>(json)
        .map(|rows| rows.into_iter().filter(|r| r.enabled).collect())
        .unwrap_or_default()
}

pub fn remotes_from_machines(rows: &[MachineRow]) -> Vec<RemoteConfig> {
    rows.iter()
        .map(|row| {
            let label = if row.label.trim().is_empty() || row.label == LOCAL_SERVER_ID {
                format!("{}-{}", LOCAL_SERVER_ID, &row.id[..row.id.len().min(6)])
            } else {
                row.label.clone()
            };
            let session = if row.session.trim().is_empty() { "default".to_string() } else { row.session.clone() };
            RemoteConfig { label, ssh_target: row.target.clone(), session, os: None, herdr_path: None }
        })
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct PrefillInputs {
    pub machine_list_json: Option<String>,
    pub wsl_socket_present: bool,
    pub native_herdr_present: bool,
}

pub fn prefill_config(platform: Platform, inputs: &PrefillInputs) -> Config {
    let mut cfg = Config::default();
    if let Some(json) = &inputs.machine_list_json {
        cfg.remote = remotes_from_machines(&parse_machine_list(json));
    }
    if platform == Platform::Windows {
        if inputs.wsl_socket_present {
            cfg.wsl.push(WslConfig { label: "wsl".into(), distro: None, session: "default".into() });
        }
        if !inputs.native_herdr_present {
            cfg.local.enabled = false;
        }
    }
    cfg
}

/// Run the probes. Every failure degrades to "nothing found".
pub async fn gather_inputs(runner: &dyn ProcessRunner, platform: Platform, env: &Env) -> PrefillInputs {
    let herdr = if platform == Platform::Windows { "herdr.exe" } else { "herdr" };
    let list = ChildCommand { program: herdr.into(), args: vec!["machine".into(), "list".into(), "--json".into()] };
    let machine_list_json = match runner.run(&list, PROBE_TIMEOUT).await {
        Ok(out) if out.success() => Some(out.stdout.trim().to_string()),
        _ => None,
    };
    if platform != Platform::Windows {
        return PrefillInputs { machine_list_json, wsl_socket_present: false, native_herdr_present: true };
    }
    let probe = launchers::wsl_command(None, &format!("test -S \"{}\"", paths::posix_socket_probe_path("default")));
    let wsl_socket_present = matches!(runner.run(&probe, PROBE_TIMEOUT).await, Ok(out) if out.success());
    let version = ChildCommand { program: herdr.into(), args: vec!["--version".into()] };
    let on_path = matches!(runner.run(&version, PROBE_TIMEOUT).await, Ok(out) if out.success());
    let marker = paths::herdr_socket_path(env, Platform::Windows, "default").exists();
    PrefillInputs { machine_list_json, wsl_socket_present, native_herdr_present: on_path || marker }
}

/// Text to write as the first `config.toml`.
pub fn render_prefilled(cfg: &Config) -> String {
    if *cfg == Config::default() {
        return Config::DEFAULT_TOML.to_string();
    }
    format!("# herdr-status configuration. Saved changes are applied live.\n# Prefilled from `herdr machine list` on first launch.\n\n{}", cfg.to_toml())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-status-core prefill`
Expected: `6 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-status-core/src/prefill.rs crates/herdr-status-core/src/lib.rs
git commit -m "feat(core): first-launch config prefill from herdr machines and wsl probe"
```

---

### Task 14: Re-exports, lint pass, done criteria

**Files:**
- Modify: `crates/herdr-status-core/src/lib.rs`

- [ ] **Step 1: Finalise the library root**

Replace `crates/herdr-status-core/src/lib.rs` with:

```rust
//! Core of herdr-status: configuration, per-server watchers and the sorted
//! agent list the UI renders. No UI code lives here.
//!
//! Entry points for the app: [`config::Config`], [`hub::Hub`],
//! [`hub::default_source_factory`], [`model::AgentList`], [`actions`],
//! [`prefill`], [`paths`].

pub mod actions;
pub mod config;
pub mod hub;
pub mod launchers;
pub mod model;
pub mod paths;
pub mod prefill;
pub mod reducer;
pub mod remote_connect;
pub mod remote_probe;
pub mod transport_source;
pub mod watcher;

pub use actions::{ActionError, KEYPAD};
pub use config::{Config, ConfigError, LocalConfig, RemoteConfig, RemoteOs, UiConfig, WslConfig};
pub use hub::{default_source_factory, Hub, ServerSpec, SourceFactory};
pub use model::{AgentList, AgentRow, AgentStatus, ConnectionState, Counts, ServerChip, ServerKind, ServerState};
pub use paths::{Env, Platform};
pub use remote_connect::{ProcessRunner, TokioRunner};
pub use watcher::WatcherConfig;
```

- [ ] **Step 2: Format and lint**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets --features herdr-client/test-support -- -D warnings`
Expected: clean. Remove the `_used`/`_kinds_are_used`/`_platform_is_used` placeholder helpers from Tasks 6, 8 and 9 and fix any unused-import warnings they were masking.

- [ ] **Step 3: Run the whole workspace**

Run: `cargo test --workspace --features herdr-client/test-support`
Expected: all green. Counts for this crate: config 7, paths 6, launchers 12, remote_probe 5, remote_connect 7, model 3, reducer 7, transport_source 6, watcher 9, hub 5, actions 5, prefill 6.

- [ ] **Step 4: Commit**

```bash
git add -A crates/herdr-status-core crates/herdr-client
git commit -m "chore(core): re-exports, docs and clippy clean"
```

---

## Plan 2 done criteria

- `cargo test --workspace --features herdr-client/test-support` green on macOS; clippy clean.
- `Hub::new(default_source_factory(..), WatcherConfig::default())` + `apply_config(&config, Platform::current())` is all the app needs to get live `AgentList`s from `hub.subscribe()`.
- `hub.client_for(id)` + `actions::{peek, send_key, send_message}` implement the drawer.
- `prefill::{gather_inputs, prefill_config, render_prefilled}` implement first launch.
- Plan 3 (the Tauri app and UI) uses only the names re-exported in `lib.rs`.
