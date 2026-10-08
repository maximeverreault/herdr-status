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
        "Linux" | "Darwin" | "FreeBSD" | "OpenBSD" | "NetBSD" => Some(RemotePlatform {
            os: RemoteOs::Posix,
            arch: arch.to_string(),
        }),
        _ => None,
    }
}

pub fn looks_like_windows_uname(os: &str) -> bool {
    let upper = os.to_ascii_uppercase();
    upper.contains("MINGW")
        || upper.contains("MSYS")
        || upper.contains("CYGWIN")
        || upper.contains("WINDOWS")
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
        .map(|arch| RemotePlatform {
            os: RemoteOs::Windows,
            arch: arch.trim().to_string(),
        })
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
    let ssh_itself = exit_code == Some(255);
    if ssh_itself && lower.contains("permission denied") {
        SshFailure::AuthDenied
    } else if ssh_itself
        && (lower.contains("host key verification failed")
            || lower.contains("remote host identification has changed"))
    {
        SshFailure::HostKey
    } else if exit_code == Some(255)
        || lower.contains("timed out")
        || lower.contains("could not resolve hostname")
        || lower.contains("connection refused")
        || lower.contains("no route to host")
    {
        SshFailure::Unreachable
    } else {
        SshFailure::Other(
            stderr
                .trim()
                .lines()
                .last()
                .unwrap_or("")
                .trim()
                .to_string(),
        )
    }
}

impl SshFailure {
    /// Text shown on the offline group line (spec section 14).
    pub fn reason(&self, target: &str) -> String {
        match self {
            SshFailure::AuthDenied => {
                format!("SSH authentication failed; run `ssh {target}` once in a terminal")
            }
            SshFailure::HostKey => {
                format!("Host key verification failed; connect with `ssh {target}` to accept it")
            }
            SshFailure::Unreachable => "Unreachable".to_string(),
            SshFailure::Other(msg) if msg.is_empty() => "SSH failed".to_string(),
            SshFailure::Other(msg) => format!("SSH failed: {msg}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uname_linux_and_darwin_are_posix() {
        assert_eq!(
            parse_uname("Linux\nx86_64\n"),
            Some(RemotePlatform {
                os: RemoteOs::Posix,
                arch: "x86_64".into()
            })
        );
        assert_eq!(
            parse_uname("Darwin\narm64\n"),
            Some(RemotePlatform {
                os: RemoteOs::Posix,
                arch: "arm64".into()
            })
        );
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
        assert!(
            !should_try_windows_probe(false, Some(255), false),
            "255 is ssh itself failing"
        );
        assert!(!should_try_windows_probe(false, None, false));
    }

    #[test]
    fn windows_probe_line_parses_arch() {
        assert_eq!(
            parse_windows_probe("\r\nherdr-windows:AMD64\r\n"),
            Some(RemotePlatform {
                os: RemoteOs::Windows,
                arch: "AMD64".into()
            })
        );
        assert_eq!(
            parse_windows_probe("banner\nherdr-windows:ARM64"),
            Some(RemotePlatform {
                os: RemoteOs::Windows,
                arch: "ARM64".into()
            })
        );
        assert_eq!(parse_windows_probe("nothing"), None);
    }

    #[test]
    fn discovery_output_uses_prefix_and_ignores_noise() {
        assert_eq!(
            parse_discovery_output("Welcome!\nherdr-status-discovery:/usr/local/bin/herdr\n"),
            Some("/usr/local/bin/herdr".into())
        );
        assert_eq!(
            parse_discovery_output(
                r"herdr-status-discovery:C:\Users\me\AppData\Local\Programs\Herdr\bin\herdr.exe"
            ),
            Some(r"C:\Users\me\AppData\Local\Programs\Herdr\bin\herdr.exe".into())
        );
        assert_eq!(parse_discovery_output("/usr/local/bin/herdr"), None);
    }

    #[test]
    fn ssh_failures_are_classified_and_explained() {
        assert_eq!(
            classify_ssh_failure(Some(255), "me@box: Permission denied (publickey)."),
            SshFailure::AuthDenied
        );
        assert_eq!(
            classify_ssh_failure(Some(255), "Host key verification failed."),
            SshFailure::HostKey
        );
        assert_eq!(
            classify_ssh_failure(
                Some(255),
                "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!"
            ),
            SshFailure::HostKey
        );
        assert_eq!(
            classify_ssh_failure(
                Some(255),
                "ssh: connect to host box port 22: Connection timed out"
            ),
            SshFailure::Unreachable
        );
        assert_eq!(
            classify_ssh_failure(Some(255), "ssh: Could not resolve hostname box"),
            SshFailure::Unreachable
        );
        assert_eq!(
            classify_ssh_failure(Some(1), "something odd\nlast line"),
            SshFailure::Other("last line".into())
        );
        assert_eq!(
            SshFailure::AuthDenied.reason("box"),
            "SSH authentication failed; run `ssh box` once in a terminal"
        );
        assert_eq!(
            SshFailure::HostKey.reason("box"),
            "Host key verification failed; connect with `ssh box` to accept it"
        );
        assert_eq!(SshFailure::Unreachable.reason("box"), "Unreachable");
        assert_eq!(
            SshFailure::Other("boom".into()).reason("box"),
            "SSH failed: boom"
        );
    }

    #[test]
    fn remote_stderr_noise_is_not_an_auth_hint_or_unreachable() {
        assert_eq!(
            classify_ssh_failure(
                Some(1),
                "'uname' is not recognized as an internal or external command,\r\noperable program or batch file."
            ),
            SshFailure::Other("operable program or batch file.".into())
        );
        assert!(matches!(
            classify_ssh_failure(Some(2), ".bashrc: Permission denied"),
            SshFailure::Other(_)
        ));
    }
}
