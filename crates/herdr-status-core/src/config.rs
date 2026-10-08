//! `config.toml`: which herdr servers to watch and a few UI knobs.
//! Unknown keys are errors so typos never silently do nothing.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The commented `[[wsl]]` / `[[remote]]` examples, shared by the default file
/// and the prefilled one.
macro_rules! server_help {
    () => {
        r#"# [[wsl]]                       # Windows only; herdr running inside WSL on this machine
# label = "wsl"
# distro = "Ubuntu"             # default distro when omitted
# session = "default"

# [[remote]]
# label = "winbox"
# ssh_target = "winbox"         # any ssh target: alias, user@host, ssh://user@host:2222
# session = "default"
# os = "windows"                # "posix" | "windows"; auto-detected and cached when omitted
# herdr_path = "C:\\Users\\me\\AppData\\Local\\Programs\\Herdr\\bin\\herdr.exe"
"#
    };
}

/// See [`server_help!`].
pub(crate) const SERVER_HELP: &str = server_help!();

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

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
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

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config.toml{}: {message}", .line.map(|l| format!(" line {l}")).unwrap_or_default())]
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
    #[error("invalid server label '{0}' (no surrounding spaces or control characters)")]
    InvalidLabel(String),
    #[error("server '{server}': invalid session name '{session}' (use letters, digits, '.', '_' or '-')")]
    InvalidSession { server: String, session: String },
    #[error("server '{0}': ssh_target must be a host, alias or user@host")]
    InvalidSshTarget(String),
    #[error("ui.{key}: {message}")]
    InvalidValue { key: String, message: String },
}

impl Config {
    /// The file written on first launch: every key, commented with its default.
    pub const DEFAULT_TOML: &'static str = concat!(
        r#"# herdr-status configuration. Saved changes are applied live.

[local]
enabled = true
session = "default"             # named session -> <herdr config>/sessions/<name>/herdr.sock
# socket_path = "/custom/herdr.sock"   # overrides session

"#,
        server_help!(),
        r#"
[ui]
peek_lines = 30
reconcile_seconds = 30   # minimum 5
"#
    );

    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let cfg: Config = toml::from_str(text).map_err(|e| {
            let mut message = e.message().trim().to_string();
            if message.is_empty() {
                let at_eof = e.span().is_none_or(|sp| sp.start >= text.len());
                message = if at_eof {
                    "unexpected end of file (a value is missing)"
                } else {
                    "syntax error"
                }
                .to_string();
            }
            ConfigError::Toml {
                message,
                line: e
                    .span()
                    .map(|span| text[..span.start].matches('\n').count() + 1),
            }
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
            if label.trim() != label || label.chars().any(char::is_control) {
                return Err(ConfigError::InvalidLabel(label.clone()));
            }
            let folded = label.to_lowercase();
            if folded == LOCAL_SERVER_ID {
                return Err(ConfigError::ReservedLabel(label.clone()));
            }
            if !seen.insert(folded) {
                return Err(ConfigError::DuplicateLabel(label.clone()));
            }
        }
        check_session(LOCAL_SERVER_ID, &self.local.session)?;
        for w in &self.wsl {
            check_session(&w.label, &w.session)?;
        }
        for r in &self.remote {
            check_session(&r.label, &r.session)?;
            if !is_valid_ssh_target(&r.ssh_target) {
                return Err(ConfigError::InvalidSshTarget(r.label.clone()));
            }
        }
        if !(1..=500).contains(&self.ui.peek_lines) {
            return Err(ConfigError::InvalidValue {
                key: "peek_lines".into(),
                message: "must be between 1 and 500".into(),
            });
        }
        if self.ui.reconcile_seconds < 5 {
            return Err(ConfigError::InvalidValue {
                key: "reconcile_seconds".into(),
                message: "must be at least 5".into(),
            });
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

    /// Serialises the config. Panics only for non-UTF-8 paths; configs built
    /// by `parse` are always serialisable.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config serialises")
    }
}
/// Sessions end up in socket paths and shell command lines, so keep them tame.
pub(crate) fn is_valid_session(session: &str) -> bool {
    !session.is_empty()
        && session != "."
        && session != ".."
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Exactly as typed: no surrounding spaces, no whitespace, no leading `-`
/// (which ssh would read as an option).
pub(crate) fn is_valid_ssh_target(target: &str) -> bool {
    !target.trim().is_empty()
        && target.trim() == target
        && !target.starts_with('-')
        && !target.chars().any(char::is_whitespace)
}

fn check_session(server: &str, session: &str) -> Result<(), ConfigError> {
    if is_valid_session(session) {
        Ok(())
    } else {
        Err(ConfigError::InvalidSession {
            server: server.to_string(),
            session: session.to_string(),
        })
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

    #[test]
    fn syntax_error_at_eof_has_a_message() {
        match Config::parse("[ui]\npeek_lines = ").unwrap_err() {
            ConfigError::Toml { message, line } => {
                assert_eq!(line, Some(2));
                assert_eq!(message, "unexpected end of file (a value is missing)");
                assert!(!message.contains('|') && !message.contains('^'));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn to_toml_round_trips_every_optional_field() {
        let cfg = Config {
            local: LocalConfig {
                enabled: true,
                session: "work".into(),
                socket_path: Some("/tmp/x.sock".into()),
            },
            wsl: vec![WslConfig {
                label: "ubuntu".into(),
                distro: Some("Ubuntu".into()),
                session: "s1".into(),
            }],
            remote: vec![
                RemoteConfig {
                    label: "a".into(),
                    ssh_target: "me@a".into(),
                    session: "s2".into(),
                    os: Some(RemoteOs::Windows),
                    herdr_path: Some("C:\\herdr.exe".into()),
                },
                RemoteConfig {
                    label: "b".into(),
                    ssh_target: "b".into(),
                    session: "default".into(),
                    os: None,
                    herdr_path: None,
                },
            ],
            ui: UiConfig {
                peek_lines: 12,
                reconcile_seconds: 5,
            },
        };
        let text = cfg.to_toml();
        assert!(
            text.contains("[[remote]]") && text.contains("[[wsl]]"),
            "{text}"
        );
        assert_eq!(Config::parse(&text).unwrap(), cfg);
    }

    #[test]
    fn default_round_trips() {
        let cfg = Config::default();
        assert_eq!(Config::parse(&cfg.to_toml()).unwrap(), cfg);
    }

    #[test]
    fn server_ids_with_local_disabled_and_remotes() {
        let text = "[local]\nenabled = false\n[[remote]]\nlabel = \"r\"\nssh_target = \"r\"\n";
        assert_eq!(Config::parse(text).unwrap().server_ids(), vec!["r"]);
    }

    #[test]
    fn rejects_invalid_session_names() {
        for bad in ["a/b", "", "..", ".", "a b", "a\\\"b", "$x"] {
            let text = format!(
                "[local]\nsession = \"{}\"\n",
                bad.replace('\\', "\\\\").replace('"', "\\\"")
            );
            let err = Config::parse(&text).unwrap_err();
            assert!(
                matches!(err, ConfigError::InvalidSession { .. }),
                "{bad:?}: {err}"
            );
        }
        let err = Config::parse("[[wsl]]\nlabel = \"u\"\nsession = \"a/b\"\n").unwrap_err();
        assert_eq!(
            err.to_string(),
            "server 'u': invalid session name 'a/b' (use letters, digits, '.', '_' or '-')"
        );
        assert!(Config::parse("[local]\nsession = \"my-work_1.x\"\n").is_ok());
    }

    #[test]
    fn rejects_untidy_labels_and_compares_case_insensitively() {
        let spaced = "[[wsl]]\nlabel = \" box\"\n";
        assert!(matches!(
            Config::parse(spaced).unwrap_err(),
            ConfigError::InvalidLabel(_)
        ));
        let ctrl = "[[wsl]]\nlabel = \"a\\u0007b\"\n";
        assert!(matches!(
            Config::parse(ctrl).unwrap_err(),
            ConfigError::InvalidLabel(_)
        ));
        let blank = "[[wsl]]\nlabel = \"  \"\n";
        assert!(matches!(
            Config::parse(blank).unwrap_err(),
            ConfigError::EmptyLabel
        ));
        let upper = "[[wsl]]\nlabel = \"LOCAL\"\n";
        assert!(matches!(
            Config::parse(upper).unwrap_err(),
            ConfigError::ReservedLabel(_)
        ));
        let dup = "[[wsl]]\nlabel = \"Box\"\n[[remote]]\nlabel = \"box\"\nssh_target = \"x\"\n";
        assert!(matches!(
            Config::parse(dup).unwrap_err(),
            ConfigError::DuplicateLabel(_)
        ));
    }

    #[test]
    fn rejects_bad_ssh_targets() {
        for bad in ["", "  ", "-oProxyCommand=x", " -oX", "host name"] {
            let text = format!("[[remote]]\nlabel = \"r\"\nssh_target = \"{bad}\"\n");
            let err = Config::parse(&text).unwrap_err();
            assert!(
                matches!(err, ConfigError::InvalidSshTarget(ref l) if l == "r"),
                "{err}"
            );
            assert_eq!(
                err.to_string(),
                "server 'r': ssh_target must be a host, alias or user@host"
            );
        }
    }

    #[test]
    fn rejects_out_of_range_ui_values() {
        for (text, key) in [
            ("[ui]\npeek_lines = 0\n", "peek_lines"),
            ("[ui]\npeek_lines = 501\n", "peek_lines"),
            ("[ui]\nreconcile_seconds = 0\n", "reconcile_seconds"),
            ("[ui]\nreconcile_seconds = 4\n", "reconcile_seconds"),
        ] {
            match Config::parse(text).unwrap_err() {
                ConfigError::InvalidValue { key: k, .. } => assert_eq!(k, key),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(Config::parse("[ui]\npeek_lines = 500\nreconcile_seconds = 5\n").is_ok());
    }

    #[test]
    fn toml_error_without_a_line_omits_the_line_part() {
        let err = ConfigError::Toml {
            message: "boom".into(),
            line: None,
        };
        assert_eq!(err.to_string(), "config.toml: boom");
        let err = ConfigError::Toml {
            message: "boom".into(),
            line: Some(3),
        };
        assert_eq!(err.to_string(), "config.toml line 3: boom");
    }

    #[test]
    fn shared_validators_match_validate() {
        assert!(is_valid_session("default") && is_valid_session("a.b_c-1"));
        assert!(!is_valid_session("") && !is_valid_session("..") && !is_valid_session("a b"));
        assert!(is_valid_ssh_target("ssh://user@host:2222") && is_valid_ssh_target("me@box"));
        assert!(!is_valid_ssh_target("") && !is_valid_ssh_target("-oX"));
        assert!(!is_valid_ssh_target(" a") && !is_valid_ssh_target("a b"));
    }
}
