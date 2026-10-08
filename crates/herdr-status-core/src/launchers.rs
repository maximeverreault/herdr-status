//! Builders for every child command the bar runs: the SSH bridge to a remote
//! herdr, the `wsl.exe` bridge, and the local `herdr.exe` fallback. Remote
//! command text mirrors herdr 0.9.3's own saved-machine bridge so hosts that
//! work with `herdr machine add` also work here.

use std::path::{Path, PathBuf};

use base64::Engine;
use herdr_client::transport::bridge::{ChildCommand, READY_MARKER};

use crate::config::RemoteOs;
use crate::paths::Platform;

/// What `herdr --session <s> remote-api-bridge --check` prints when supported.
pub const BRIDGE_CAPABILITY: &str = "herdr-api-bridge-v1";

/// POSIX single-quote quoting, identical to herdr's `shell_quote`.
pub fn quote_posix(value: &str) -> String {
    if !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '@' | '%' | '_' | '+' | '=' | ':' | ',' | '.' | '/' | '-'
                )
        })
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// PowerShell single-quote quoting, identical to herdr's `quote_powershell_arg`.
pub fn quote_powershell_arg(value: &str) -> String {
    if !value.is_empty()
        && !value.starts_with('-')
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b':' | b'+' | b'=')
        })
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "''"))
}

/// Windows `CommandLineToArgv` quoting, identical to herdr's `quote_windows_command_line_arg`.
pub fn quote_windows_command_line_arg(value: &str) -> String {
    if !value.is_empty()
        && !value
            .chars()
            .any(|ch| matches!(ch, ' ' | '\t' | '\n' | '\x0b' | '"'))
    {
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
/// Requires a POSIX-ish remote login shell (tcsh/csh will fail, as with herdr).
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
        printf 'herdr-status-discovery:%s\n' "$candidate"
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
if ($LASTEXITCODE -eq 0 -and $cap -eq '{BRIDGE_CAPABILITY}') {{ [Console]::Out.WriteLine('herdr-status-discovery:' + $c); exit 0 }} }} }}; \
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

/// Longest ControlPath directory that keeps `<dir>/<%C>` plus ssh's 17-char
/// temp suffix under the 104-byte `sun_path` limit (`%C` expands to 40 chars).
const SUN_PATH_LIMIT: usize = 104;
const CONTROL_PATH_SUFFIX: usize = 1 + 40 + 17;

/// The directory is usable in a `ControlPath` option: no characters ssh would
/// expand or unquote, and a socket path short enough for `sun_path`.
fn control_dir_is_usable(dir: &Path) -> bool {
    let text = dir.to_string_lossy();
    !text.contains('"')
        && !text.contains('%')
        && dir.as_os_str().len() + CONTROL_PATH_SUFFIX < SUN_PATH_LIMIT
}

/// `ssh` invocation for one remote command. Never prompts (BatchMode).
///
/// `-T` and `RemoteCommand=none` stop a user's `~/.ssh/config` (`RequestTTY`,
/// `RemoteCommand`) from corrupting the bridge stream. `ControlPath` uses `%C`,
/// which needs OpenSSH 6.7 or newer. `ConnectTimeout=10` sits comfortably
/// inside the runner's 30 s bound.
pub fn ssh_command(target: &str, remote_command: &str, opts: &SshOptions) -> ChildCommand {
    let mut args: Vec<String> = vec!["-T".into()];
    args.extend(
        [
            "RemoteCommand=none",
            "BatchMode=yes",
            "ConnectTimeout=10",
            "ServerAliveInterval=15",
            "ServerAliveCountMax=3",
        ]
        .iter()
        .flat_map(|o| ["-o".to_string(), o.to_string()]),
    );
    if opts.client == Platform::Mac {
        if let Some(dir) = &opts.control_dir {
            if control_dir_is_usable(dir) {
                args.extend(["-o".into(), "ControlMaster=auto".into()]);
                args.extend(["-o".into(), "ControlPersist=600".into()]);
                // ssh splits -o values on whitespace; quote for directories with spaces.
                args.extend(["-o".into(), format!("ControlPath=\"{}/%C\"", dir.display())]);
            } else {
                static WARN: std::sync::Once = std::sync::Once::new();
                WARN.call_once(|| {
                    tracing::warn!(dir = %dir.display(), "ssh control directory unusable (quote, percent or too long); connection sharing disabled");
                });
            }
        }
    }
    args.push("--".into());
    args.push(target.to_string());
    args.push(remote_command.to_string());
    ChildCommand {
        program: "ssh".into(),
        args,
    }
}

/// The long-lived bridge command for a resolved remote.
pub fn remote_bridge_command(
    target: &str,
    os: RemoteOs,
    herdr_path: &str,
    session: &str,
    opts: &SshOptions,
) -> ChildCommand {
    let remote = match os {
        RemoteOs::Posix => posix_sh_command(&posix_bridge_script(herdr_path, session)),
        RemoteOs::Windows => {
            powershell_encoded_command(&windows_bridge_script(herdr_path, session))
        }
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
    args.extend([
        "--exec".into(),
        "sh".into(),
        "-lc".into(),
        script.to_string(),
    ]);
    ChildCommand {
        program: "wsl.exe".into(),
        args,
    }
}

/// Local Windows fallback when the named pipe cannot be opened.
pub fn local_herdr_bridge_command(herdr_exe: &str, session: &str) -> ChildCommand {
    ChildCommand {
        program: herdr_exe.to_string(),
        args: vec![
            "--session".into(),
            session.to_string(),
            "remote-api-bridge".into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::path::PathBuf;

    #[test]
    fn posix_quoting_matches_herdr() {
        assert_eq!(quote_posix("default"), "default");
        assert_eq!(
            quote_posix("/opt/homebrew/bin/herdr"),
            "/opt/homebrew/bin/herdr"
        );
        assert_eq!(quote_posix("user@host:2222"), "user@host:2222");
        assert_eq!(quote_posix("build host"), "'build host'");
        assert_eq!(quote_posix("it's"), "'it'\\''s'");
        assert_eq!(quote_posix(""), "''");
    }

    #[test]
    fn powershell_quoting_matches_herdr() {
        assert_eq!(quote_powershell_arg("herdr.exe"), "herdr.exe");
        assert_eq!(
            quote_powershell_arg("--session default remote-api-bridge"),
            "'--session default remote-api-bridge'"
        );
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
        assert_eq!(
            quote_windows_command_line_arg("my session"),
            "\"my session\""
        );
        assert_eq!(
            quote_windows_command_line_arg(r#"say "hi""#),
            r#""say \"hi\"""#
        );
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
        assert!(spaced
            .ends_with("exec '/Users/me/my tools/herdr' --session 'agent work' remote-api-bridge"));
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
        let full_path = windows_bridge_script(
            r"C:\Users\me\AppData\Local\Programs\Herdr\bin\herdr.exe",
            "agents",
        );
        assert!(full_path.contains(r"-FilePath 'C:\Users\me\AppData\Local\Programs\Herdr\bin\herdr.exe' -ArgumentList '--session agents remote-api-bridge'"));
    }

    #[test]
    fn encoded_command_is_utf16le_base64() {
        let cmd = powershell_encoded_command("exit 0");
        assert!(
            cmd.starts_with("powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand ")
        );
        let b64 = cmd.rsplit(' ').next().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
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
        assert!(s.contains("printf 'herdr-status-discovery:%s\\n' \"$candidate\""));
        assert!(s.contains("*/mise/shims/herdr) continue"));
        assert!(s.ends_with("exit 2"));

        let w = windows_discovery_script("default");
        assert!(w.contains("Get-Command herdr.exe"));
        assert!(w.contains(r"Programs\Herdr\bin\herdr.exe"));
        assert!(w.contains("--session default remote-api-bridge --check"));
        assert!(w.contains("'herdr-api-bridge-v1'"));
        assert!(w.contains("[Console]::Out.WriteLine('herdr-status-discovery:' + $c)"));
    }

    #[test]
    fn ssh_command_has_batch_mode_and_control_options_on_mac_only() {
        let opts = SshOptions {
            client: Platform::Mac,
            control_dir: Some(PathBuf::from("/tmp/cm")),
        };
        let mac = ssh_command("me@winbox", "uname -s; uname -m", &opts);
        assert_eq!(mac.program, "ssh");
        assert_eq!(
            mac.args,
            vec![
                "-T",
                "-o",
                "RemoteCommand=none",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPersist=600",
                "-o",
                "ControlPath=\"/tmp/cm/%C\"",
                "--",
                "me@winbox",
                "uname -s; uname -m",
            ]
        );
        let opts = SshOptions {
            client: Platform::Windows,
            control_dir: Some(PathBuf::from(r"C:\cm")),
        };
        let win = ssh_command("winbox", "x", &opts);
        assert!(!win.args.iter().any(|a| a.starts_with("Control")));
        let no_dir = SshOptions {
            client: Platform::Mac,
            control_dir: None,
        };
        assert!(!ssh_command("h", "x", &no_dir)
            .args
            .iter()
            .any(|a| a.starts_with("Control")));
    }

    #[test]
    fn remote_bridge_command_picks_posix_or_powershell_remote_command() {
        let opts = SshOptions {
            client: Platform::Mac,
            control_dir: None,
        };
        let posix = remote_bridge_command(
            "box",
            RemoteOs::Posix,
            "/usr/local/bin/herdr",
            "default",
            &opts,
        );
        assert_eq!(
            posix.args.last().unwrap(),
            &posix_sh_command(&posix_bridge_script("/usr/local/bin/herdr", "default"))
        );
        let windows =
            remote_bridge_command("winbox", RemoteOs::Windows, "herdr.exe", "default", &opts);
        assert!(windows
            .args
            .last()
            .unwrap()
            .starts_with("powershell.exe -NoLogo"));
    }

    #[test]
    fn wsl_command_uses_exec_and_login_shell() {
        let c = wsl_command(Some("Ubuntu"), "exec herdr");
        assert_eq!(c.program, "wsl.exe");
        assert_eq!(
            c.args,
            vec!["-d", "Ubuntu", "--exec", "sh", "-lc", "exec herdr"]
        );
        assert_eq!(
            wsl_command(None, "x").args,
            vec!["--exec", "sh", "-lc", "x"]
        );
    }

    #[test]
    fn local_windows_bridge_command_is_plain_herdr_exe() {
        let c = local_herdr_bridge_command(r"C:\h\herdr.exe", "default");
        assert_eq!(c.program, r"C:\h\herdr.exe");
        assert_eq!(c.args, vec!["--session", "default", "remote-api-bridge"]);
    }

    fn has_control(c: &ChildCommand) -> bool {
        c.args.iter().any(|a| a.starts_with("Control"))
    }

    #[test]
    fn control_dir_with_space_is_quoted() {
        let opts = SshOptions {
            client: Platform::Mac,
            control_dir: Some(PathBuf::from("/tmp/my cm")),
        };
        let c = ssh_command("h", "x", &opts);
        assert!(c
            .args
            .contains(&"ControlPath=\"/tmp/my cm/%C\"".to_string()));
    }

    #[test]
    fn unusable_control_dirs_disable_all_control_options() {
        let long = PathBuf::from(format!("/tmp/{}", "a".repeat(60)));
        let pct = PathBuf::from("/tmp/100%");
        let quote = PathBuf::from("/tmp/a\"b");
        for dir in [long, pct, quote] {
            let opts = SshOptions {
                client: Platform::Mac,
                control_dir: Some(dir),
            };
            assert!(!has_control(&ssh_command("h", "x", &opts)));
        }
        // 104 - 58 = 46 is the first rejected length
        let ok = SshOptions {
            client: Platform::Mac,
            control_dir: Some(PathBuf::from("a".repeat(45))),
        };
        assert!(has_control(&ssh_command("h", "x", &ok)));
        let bad = SshOptions {
            client: Platform::Mac,
            control_dir: Some(PathBuf::from("a".repeat(46))),
        };
        assert!(!has_control(&ssh_command("h", "x", &bad)));
    }

    #[test]
    fn posix_sh_command_double_escapes_single_quotes() {
        let expected = [
            r"/bin/sh -c 'printf '\''\n%s\n'\'' '\''herdr-remote-output-ready:1'\''",
            r"exec '\''/opt/it'\''\'\'''\''s/herdr'\'' --session default remote-api-bridge'",
        ]
        .join("\n");
        assert_eq!(
            posix_sh_command(&posix_bridge_script("/opt/it's/herdr", "default")),
            expected
        );
    }

    #[cfg(unix)]
    #[test]
    fn posix_discovery_script_finds_herdr_in_a_home_with_spaces() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("my home");
        let bin = home.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let herdr = bin.join("herdr");
        std::fs::write(
            &herdr,
            "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = --check ] && echo herdr-api-bridge-v1; done\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(posix_sh_command(&posix_discovery_script("default")))
            .env_clear()
            .env("HOME", &home)
            .env("PATH", "/nonexistent")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(&format!("herdr-status-discovery:{}", herdr.display())),
            "{stdout}"
        );
    }
}
