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
        Self {
            enabled: true,
            session: default_session(),
            socket_path: None,
        }
    }
}
impl Default for UiConfig {
    fn default() -> Self {
        Self {
            peek_lines: default_peek_lines(),
            reconcile_seconds: default_reconcile_seconds(),
        }
    }
}
impl Default for Config {
    fn default() -> Self {
        Self {
            local: LocalConfig::default(),
            wsl: Vec::new(),
            remote: Vec::new(),
            ui: UiConfig::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config.toml line {}: {message}", .line.map(|l| l.to_string()).unwrap_or_else(|| "?".into()))]
    Toml {
        message: String,
        line: Option<usize>,
    },
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
            line: e
                .span()
                .map(|span| text[..span.start].matches('\n').count() + 1),
        })?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut seen = std::collections::HashSet::new();
        let labels = self
            .wsl
            .iter()
            .map(|w| &w.label)
            .chain(self.remote.iter().map(|r| &r.label));
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
        assert_eq!(
            cfg.local.socket_path.as_deref(),
            Some(std::path::Path::new("/tmp/custom.sock"))
        );
        assert_eq!(cfg.wsl[0].label, "ubuntu");
        assert_eq!(cfg.wsl[0].distro.as_deref(), Some("Ubuntu"));
        assert_eq!(cfg.wsl[0].session, "default");
        assert_eq!(cfg.remote[0].label, "winbox");
        assert_eq!(cfg.remote[0].ssh_target, "me@winbox");
        assert_eq!(cfg.remote[0].session, "agents");
        assert_eq!(cfg.remote[0].os, Some(RemoteOs::Windows));
        assert!(cfg.remote[0]
            .herdr_path
            .as_deref()
            .unwrap()
            .ends_with("herdr.exe"));
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
        assert!(
            matches!(err, ConfigError::DuplicateLabel(ref l) if l == "box"),
            "{err}"
        );
    }

    #[test]
    fn rejects_reserved_and_empty_labels() {
        let reserved = "[[remote]]\nlabel = \"local\"\nssh_target = \"x\"\n";
        assert!(matches!(
            Config::parse(reserved).unwrap_err(),
            ConfigError::ReservedLabel(_)
        ));
        let empty = "[[remote]]\nlabel = \"\"\nssh_target = \"x\"\n";
        assert!(matches!(
            Config::parse(empty).unwrap_err(),
            ConfigError::EmptyLabel
        ));
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
        assert!(matches!(
            Config::parse(broken).unwrap_err(),
            ConfigError::Toml { line: Some(1), .. }
        ));
    }
}
