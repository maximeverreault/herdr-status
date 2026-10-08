//! First-launch configuration prefill (spec section 13).

use std::collections::HashSet;
use std::time::Duration;

use herdr_client::transport::bridge::ChildCommand;
use serde::Deserialize;

use crate::config::{
    is_valid_session, is_valid_ssh_target, Config, RemoteConfig, WslConfig, LOCAL_SERVER_ID,
    SERVER_HELP,
};
use crate::launchers;
use crate::paths::{self, Env, Platform};
use crate::remote_connect::ProcessRunner;
use crate::remote_probe;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// One row of `herdr machine list --json`.
#[derive(Debug, Clone, Deserialize)]
pub struct MachineRow {
    pub id: String,
    #[serde(default)]
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

/// Enabled rows only. Rows that fail to deserialise are skipped one by one;
/// anything that is not a JSON array is an empty list.
pub fn parse_machine_list(json: &str) -> Vec<MachineRow> {
    let Ok(values) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return Vec::new();
    };
    values
        .into_iter()
        .filter_map(|v| serde_json::from_value::<MachineRow>(v).ok())
        .filter(|r| r.enabled)
        .collect()
}

/// Remotes that pass `Config::validate`: invalid targets are skipped, bad
/// sessions fall back to `default`, labels are made unique and non-reserved.
pub fn remotes_from_machines(rows: &[MachineRow]) -> Vec<RemoteConfig> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut remotes = Vec::new();
    for row in rows {
        let target = row.target.trim();
        if !is_valid_ssh_target(target) {
            continue;
        }
        let cleaned: String = row.label.chars().filter(|c| !c.is_control()).collect();
        let cleaned = cleaned.trim();
        let base = if cleaned.is_empty() || cleaned.to_lowercase() == LOCAL_SERVER_ID {
            format!(
                "{}-{}",
                LOCAL_SERVER_ID,
                row.id
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .take(6)
                    .collect::<String>()
            )
        } else {
            cleaned.to_string()
        };
        let mut label = base.clone();
        let mut n = 2;
        while !seen.insert(label.to_lowercase()) {
            label = format!("{base}-{n}");
            n += 1;
        }
        let session = if is_valid_session(&row.session) {
            row.session.clone()
        } else {
            "default".to_string()
        };
        remotes.push(RemoteConfig {
            label,
            ssh_target: target.to_string(),
            session,
            os: None,
            herdr_path: None,
        });
    }
    remotes
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
            let taken: HashSet<String> =
                cfg.remote.iter().map(|r| r.label.to_lowercase()).collect();
            let mut label = "wsl".to_string();
            let mut n = 2;
            while taken.contains(&label) {
                label = format!("wsl-{n}");
                n += 1;
            }
            cfg.wsl.push(WslConfig {
                label,
                distro: None,
                session: "default".into(),
            });
        }
        if !inputs.native_herdr_present {
            cfg.local.enabled = false;
        }
    }
    validated_or_fallback(cfg)
}

/// Keep `cfg` if valid; otherwise retry without the remotes (keeping the
/// local/WSL decisions); otherwise the defaults.
fn validated_or_fallback(mut cfg: Config) -> Config {
    let Err(err) = cfg.validate() else {
        return cfg;
    };
    tracing::warn!(%err, "prefilled config is invalid; dropping remotes");
    cfg.remote.clear();
    if let Err(err) = cfg.validate() {
        tracing::warn!(%err, "prefilled config is still invalid; using defaults");
        return Config::default();
    }
    cfg
}

/// Where `herdr` lives on macOS. A GUI app inherits launchd's minimal `PATH`,
/// so look in the usual places first and fall back to the bare name.
async fn discover_mac_herdr(runner: &dyn ProcessRunner) -> String {
    let discovery = ChildCommand {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), launchers::posix_discovery_script("default")],
    };
    match runner.run(&discovery, PROBE_TIMEOUT).await {
        Ok(out) if out.success() => {
            remote_probe::parse_discovery_output(&out.stdout).filter(|p| !p.is_empty())
        }
        _ => None,
    }
    .unwrap_or_else(|| "herdr".to_string())
}

/// Run the probes. Every failure degrades to "nothing found".
pub async fn gather_inputs(
    runner: &dyn ProcessRunner,
    platform: Platform,
    env: &Env,
) -> PrefillInputs {
    let herdr = if platform == Platform::Windows {
        "herdr.exe".to_string()
    } else {
        discover_mac_herdr(runner).await
    };
    let list = ChildCommand {
        program: herdr.clone(),
        args: vec!["machine".into(), "list".into(), "--json".into()],
    };
    let machine_list_json = match runner.run(&list, PROBE_TIMEOUT).await {
        Ok(out) if out.success() => Some(out.stdout.trim().to_string()),
        _ => None,
    };
    if platform != Platform::Windows {
        return PrefillInputs {
            machine_list_json,
            wsl_socket_present: false,
            native_herdr_present: true,
        };
    }
    let wsl_probe = launchers::wsl_command(
        None,
        &format!("test -S \"{}\"", paths::posix_socket_probe_path("default")),
    );
    let version = ChildCommand {
        program: herdr,
        args: vec!["--version".into()],
    };
    let (wsl, native) = tokio::join!(
        runner.run(&wsl_probe, PROBE_TIMEOUT),
        runner.run(&version, PROBE_TIMEOUT)
    );
    let wsl_socket_present = matches!(wsl, Ok(out) if out.success());
    let on_path = matches!(native, Ok(out) if out.success());
    let marker = paths::herdr_socket_path(env, Platform::Windows, "default").exists();
    PrefillInputs {
        machine_list_json,
        wsl_socket_present,
        native_herdr_present: on_path || marker,
    }
}

/// Text to write as the first `config.toml`. Self-verifying: if the rendered
/// text does not parse back to exactly `cfg`, the default file is used.
pub fn render_prefilled(cfg: &Config) -> String {
    if *cfg == Config::default() {
        return Config::DEFAULT_TOML.to_string();
    }
    let text = format!(
        "# herdr-status configuration. Saved changes are applied live.\n# Prefilled on first launch; edit freely.\n\n{}\n{}",
        cfg.to_toml(),
        SERVER_HELP
    );
    if matches!(Config::parse(&text), Ok(ref parsed) if parsed == cfg) {
        text
    } else {
        tracing::warn!("prefilled config did not round-trip; writing the default file");
        Config::DEFAULT_TOML.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_connect::{ProcessRunner, RunOutput};
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
        assert_eq!(
            remotes[1].label, "local-aaaaaa",
            "reserved label gets an id suffix"
        );
        assert_eq!(remotes[1].session, "default", "empty session means default");
    }

    fn row(id: &str, label: &str, target: &str, session: &str) -> MachineRow {
        MachineRow {
            id: id.into(),
            label: label.into(),
            target: target.into(),
            session: session.into(),
            enabled: true,
        }
    }

    #[test]
    fn remotes_skip_invalid_targets_and_sanitise_sessions() {
        let rows = vec![
            row("1111111111", "empty", "   ", "default"),
            row("2222222222", "spaced", "me@host extra", "default"),
            row("3333333333", "dash", "-oProxyCommand=x", "default"),
            row("4444444444", "  padded  ", "  me@ok  ", "bad session/../x"),
            row("5555555555", "dots", "dots", ".."),
            row("6666666666", "  ", "blank", "agents"),
            row("7777777777", " LOCAL ", "up", " agents "),
        ];
        let remotes = remotes_from_machines(&rows);
        let labels: Vec<_> = remotes.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["padded", "dots", "local-666666", "local-777777"]);
        assert_eq!(remotes[0].ssh_target, "me@ok");
        assert_eq!(remotes[0].session, "default");
        assert_eq!(remotes[1].session, "default");
        assert_eq!(remotes[2].session, "agents");
        assert_eq!(
            remotes[3].session, "default",
            "untrimmed session is invalid"
        );
        let cfg = Config {
            remote: remotes,
            ..Config::default()
        };
        cfg.validate().unwrap();
    }

    #[test]
    fn remotes_deduplicate_labels_case_insensitively() {
        let rows = vec![
            row("1111111111", "Box", "a", "default"),
            row("2222222222", "box", "b", "default"),
            row("3333333333", "BOX", "c", "default"),
            row("4444444444", "box-2", "d", "default"),
        ];
        let remotes = remotes_from_machines(&rows);
        let labels: Vec<_> = remotes.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Box", "box-2", "BOX-3", "box-2-2"]);
        let cfg = Config {
            remote: remotes,
            ..Config::default()
        };
        cfg.validate().unwrap();
    }

    #[test]
    fn prefill_on_mac_adds_remotes_only() {
        let inputs = PrefillInputs {
            machine_list_json: Some(MACHINES.into()),
            wsl_socket_present: true,
            native_herdr_present: true,
        };
        let cfg = prefill_config(Platform::Mac, &inputs);
        assert!(cfg.local.enabled);
        assert!(cfg.wsl.is_empty(), "wsl never prefilled on mac");
        assert_eq!(cfg.remote.len(), 2);
        cfg.validate().unwrap();
    }

    #[test]
    fn prefill_on_windows_adds_wsl_and_disables_local_without_native_herdr() {
        let inputs = PrefillInputs {
            machine_list_json: None,
            wsl_socket_present: true,
            native_herdr_present: false,
        };
        let cfg = prefill_config(Platform::Windows, &inputs);
        assert!(!cfg.local.enabled);
        assert_eq!(cfg.wsl.len(), 1);
        assert_eq!(cfg.wsl[0].label, "wsl");
        assert_eq!(cfg.wsl[0].distro, None);
        let native = PrefillInputs {
            machine_list_json: None,
            wsl_socket_present: false,
            native_herdr_present: true,
        };
        let cfg = prefill_config(Platform::Windows, &native);
        assert!(cfg.local.enabled);
        assert!(cfg.wsl.is_empty());
    }

    #[test]
    fn render_uses_default_text_when_nothing_was_found() {
        assert_eq!(render_prefilled(&Config::default()), Config::DEFAULT_TOML);
        let mut cfg = Config::default();
        cfg.remote.push(RemoteConfig {
            label: "box".into(),
            ssh_target: "box".into(),
            session: "default".into(),
            os: None,
            herdr_path: None,
        });
        let text = render_prefilled(&cfg);
        assert!(text.starts_with("# herdr-status configuration"));
        assert!(text.contains("[[remote]]"));
        assert_eq!(
            Config::parse(&text).unwrap(),
            cfg,
            "rendered text round-trips"
        );
    }

    #[test]
    fn one_bad_row_does_not_drop_the_rest() {
        let json = r#"[{"id":"1","label":"a","target":"a"},
            {"label":"no id","target":"x"},
            "not an object",
            {"id":"3","target":"c"},
            {"id":"4","label":"d","target":"d","enabled":false}]"#;
        let rows = parse_machine_list(json);
        let ids: Vec<_> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["1", "3"]);
        assert_eq!(rows[1].label, "", "missing label defaults to empty");
        assert!(parse_machine_list(r#"{"a":1}"#).is_empty());
    }

    #[test]
    fn ids_are_sanitised_and_may_be_short() {
        let rows = vec![
            row("ab", "", "t1", "default"),
            row("a-b/c d!ef9z", "LOCAL", "t2", "default"),
            row("--", " ", "t3", "default"),
        ];
        let labels: Vec<_> = remotes_from_machines(&rows)
            .into_iter()
            .map(|r| r.label)
            .collect();
        assert_eq!(labels, ["local-ab", "local-abcdef", "local-"]);
    }

    #[test]
    fn ssh_urls_and_non_ascii_labels_are_kept() {
        let rows = vec![row(
            "1111111111",
            "Büro-Mac 🖥",
            "ssh://user@host:2222",
            "default",
        )];
        let remotes = remotes_from_machines(&rows);
        assert_eq!(remotes[0].label, "Büro-Mac 🖥");
        assert_eq!(remotes[0].ssh_target, "ssh://user@host:2222");
        prefill_config(
            Platform::Mac,
            &PrefillInputs {
                machine_list_json: Some(
                    r#"[{"id":"1","label":"Büro-Mac 🖥","target":"ssh://user@host:2222"}]"#.into(),
                ),
                ..PrefillInputs::default()
            },
        )
        .validate()
        .unwrap();
    }

    #[test]
    fn remote_labelled_wsl_pushes_the_wsl_entry_to_wsl_2() {
        let inputs = PrefillInputs {
            machine_list_json: Some(r#"[{"id":"1","label":"wsl","target":"box"}]"#.into()),
            wsl_socket_present: true,
            native_herdr_present: true,
        };
        let cfg = prefill_config(Platform::Windows, &inputs);
        assert_eq!(cfg.remote[0].label, "wsl");
        assert_eq!(cfg.wsl[0].label, "wsl-2");
        cfg.validate().unwrap();
    }

    #[test]
    fn prefill_keeps_wsl_decision_when_a_remote_is_invalid() {
        let mut cfg = Config::default();
        cfg.local.enabled = false;
        cfg.wsl.push(WslConfig {
            label: "wsl".into(),
            distro: None,
            session: "default".into(),
        });
        cfg.remote.push(RemoteConfig {
            label: "bad".into(),
            ssh_target: "-oProxyCommand=x".into(),
            session: "default".into(),
            os: None,
            herdr_path: None,
        });
        let fixed = validated_or_fallback(cfg);
        assert!(fixed.remote.is_empty());
        assert!(!fixed.local.enabled);
        assert_eq!(fixed.wsl.len(), 1);

        // Still invalid without remotes: defaults.
        let mut hopeless = Config::default();
        hopeless.local.session = "bad session".into();
        assert_eq!(validated_or_fallback(hopeless), Config::default());
    }

    #[test]
    fn render_prefilled_output_always_parses_back() {
        let mut cfg = Config::default();
        cfg.local.enabled = false;
        cfg.wsl.push(WslConfig {
            label: "wsl".into(),
            distro: Some("Ubuntu".into()),
            session: "default".into(),
        });
        cfg.remote.push(RemoteConfig {
            label: "Büro \"quoted\" \\ 🖥".into(),
            ssh_target: "ssh://user@host:2222".into(),
            session: "agents".into(),
            os: None,
            herdr_path: None,
        });
        let text = render_prefilled(&cfg);
        assert!(text.starts_with(
            "# herdr-status configuration. Saved changes are applied live.\n# Prefilled on first launch; edit freely.\n"
        ));
        assert!(text.contains("# [[wsl]]") && text.contains("# [[remote]]"));
        assert_eq!(Config::parse(&text).unwrap(), cfg);

        // A config that cannot round-trip falls back to the default text.
        let mut broken = Config::default();
        broken.remote.push(RemoteConfig {
            label: "local".into(),
            ssh_target: "x".into(),
            session: "default".into(),
            os: None,
            herdr_path: None,
        });
        assert_eq!(render_prefilled(&broken), Config::DEFAULT_TOML);
    }

    struct FakeRunner {
        outputs: Mutex<VecDeque<RunOutput>>,
        calls: Mutex<Vec<ChildCommand>>,
    }
    impl FakeRunner {
        fn scripted(outputs: &[(i32, &str)]) -> Self {
            Self {
                outputs: Mutex::new(
                    outputs
                        .iter()
                        .map(|(code, out)| RunOutput {
                            exit_code: Some(*code),
                            stdout: (*out).into(),
                            stderr: String::new(),
                        })
                        .collect(),
                ),
                calls: Mutex::new(Vec::new()),
            }
        }
    }
    #[async_trait::async_trait]
    impl ProcessRunner for FakeRunner {
        async fn run(&self, command: &ChildCommand, _t: Duration) -> std::io::Result<RunOutput> {
            self.calls.lock().unwrap().push(command.clone());
            Ok(self
                .outputs
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(RunOutput {
                    exit_code: Some(1),
                    stdout: String::new(),
                    stderr: String::new(),
                }))
        }
    }

    #[tokio::test]
    async fn gather_inputs_runs_the_expected_commands() {
        let runner = FakeRunner {
            outputs: Mutex::new(VecDeque::from(vec![
                RunOutput {
                    exit_code: Some(0),
                    stdout: "[]".into(),
                    stderr: String::new(),
                },
                RunOutput {
                    exit_code: Some(0),
                    stdout: String::new(),
                    stderr: String::new(),
                },
                RunOutput {
                    exit_code: Some(0),
                    stdout: "herdr 0.9.3".into(),
                    stderr: String::new(),
                },
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

        let mac_runner = FakeRunner::scripted(&[
            (0, "herdr-status-discovery:/opt/homebrew/bin/herdr\n"),
            (0, "[]"),
        ]);
        let inputs = gather_inputs(&mac_runner, Platform::Mac, &Env::default()).await;
        assert_eq!(inputs.machine_list_json.as_deref(), Some("[]"));
        assert!(!inputs.wsl_socket_present);
        assert!(inputs.native_herdr_present, "not probed on mac");
        let calls = mac_runner.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2, "discovery and the machine list on mac");
        assert_eq!(calls[0].program, "/bin/sh");
        assert_eq!(calls[0].args[0], "-c");
        assert_eq!(
            calls[0].args[1],
            launchers::posix_discovery_script("default")
        );
        assert_eq!(calls[1].program, "/opt/homebrew/bin/herdr");
        assert_eq!(calls[1].args, vec!["machine", "list", "--json"]);
    }

    #[tokio::test]
    async fn mac_prefill_falls_back_to_bare_herdr_when_discovery_fails() {
        let runner = FakeRunner::scripted(&[(1, ""), (0, "[]")]);
        let inputs = gather_inputs(&runner, Platform::Mac, &Env::default()).await;
        assert_eq!(inputs.machine_list_json.as_deref(), Some("[]"));
        let calls = runner.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].program, "herdr");
        assert_eq!(calls[1].args, vec!["machine", "list", "--json"]);

        // Discovery that succeeds but prints no marker also falls back.
        let runner = FakeRunner::scripted(&[(0, "noise\n"), (1, "")]);
        let inputs = gather_inputs(&runner, Platform::Mac, &Env::default()).await;
        assert!(inputs.machine_list_json.is_none());
        assert_eq!(runner.calls.lock().unwrap()[1].program, "herdr");
    }

    #[tokio::test]
    async fn windows_probes_use_exact_wsl_argv() {
        let runner = FakeRunner::scripted(&[(0, "[]"), (0, ""), (0, "herdr")]);
        let env = Env::from_pairs(vec![("APPDATA".to_string(), "/nonexistent".to_string())]);
        gather_inputs(&runner, Platform::Windows, &env).await;
        let calls = runner.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 3);
        let wsl = calls.iter().find(|c| c.program == "wsl.exe").unwrap();
        assert_eq!(
            wsl.args,
            vec![
                "--exec",
                "sh",
                "-lc",
                "test -S \"${XDG_CONFIG_HOME:-$HOME/.config}/herdr/herdr.sock\""
            ]
        );
        assert!(calls.iter().any(|c| c.args == ["--version"]));
    }

    #[tokio::test]
    async fn native_herdr_present_via_socket_marker_when_version_fails() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("herdr.sock");
        std::fs::write(&marker, "").unwrap();
        let env = Env::from_pairs(vec![
            ("APPDATA".to_string(), dir.path().to_string_lossy().into()),
            (
                "HERDR_SOCKET_PATH".to_string(),
                marker.to_string_lossy().into(),
            ),
        ]);
        let runner = FakeRunner::scripted(&[(1, ""), (1, ""), (1, "")]);
        let inputs = gather_inputs(&runner, Platform::Windows, &env).await;
        assert!(inputs.native_herdr_present);
        assert!(!inputs.wsl_socket_present);
        let missing = Env::from_pairs(vec![(
            "HERDR_SOCKET_PATH".to_string(),
            dir.path().join("nope.sock").to_string_lossy().into(),
        )]);
        let runner = FakeRunner::scripted(&[(1, ""), (1, ""), (1, "")]);
        let inputs = gather_inputs(&runner, Platform::Windows, &missing).await;
        assert!(!inputs.native_herdr_present);
    }
}
