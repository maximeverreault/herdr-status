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
        Self {
            vars: std::env::vars().collect(),
        }
    }

    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            vars: pairs.into_iter().collect(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }
}

const HERDR_DIR: &str = "herdr";
const APP_DIR: &str = "herdr-status";

/// herdr's config directory: `$XDG_CONFIG_HOME/herdr`, else `~/.config/herdr`
/// on macOS, `%APPDATA%\herdr` (then `%USERPROFILE%\AppData\Roaming\herdr`) on Windows.
pub fn herdr_config_dir(env: &Env, platform: Platform) -> PathBuf {
    if let Some(xdg) = env.get("XDG_CONFIG_HOME") {
        return join_for(platform, PathBuf::from(xdg), HERDR_DIR);
    }
    match platform {
        Platform::Mac => home(env).join(".config").join(HERDR_DIR),
        Platform::Windows => join_for(platform, roaming(env), HERDR_DIR),
    }
}

/// The socket path herdr would use for `session`, honouring `HERDR_SOCKET_PATH`.
pub fn herdr_socket_path(env: &Env, platform: Platform, session: &str) -> PathBuf {
    if let Some(p) = env.get("HERDR_SOCKET_PATH") {
        return PathBuf::from(p);
    }
    let dir = herdr_config_dir(env, platform);
    if session == "default" {
        join_for(platform, dir, "herdr.sock")
    } else {
        let sessions = join_for(platform, dir, "sessions");
        let session_dir = join_for(platform, sessions, session);
        join_for(platform, session_dir, "herdr.sock")
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
        Platform::Windows => {
            let dir = join_for(platform, roaming(env), APP_DIR);
            join_for(platform, dir, "config.toml")
        }
    }
}

/// Cache, SSH control sockets.
pub fn app_data_dir(env: &Env, platform: Platform) -> PathBuf {
    match platform {
        Platform::Mac => home(env)
            .join("Library")
            .join("Application Support")
            .join(APP_DIR),
        Platform::Windows => join_for(platform, local_appdata(env), APP_DIR),
    }
}

pub fn app_log_dir(env: &Env, platform: Platform) -> PathBuf {
    match platform {
        Platform::Mac => home(env).join("Library").join("Logs").join(APP_DIR),
        Platform::Windows => {
            let dir = join_for(platform, local_appdata(env), APP_DIR);
            join_for(platform, dir, "logs")
        }
    }
}

/// Extends `base` with one component. On Windows the separator is always a
/// single backslash so the result is identical regardless of the host OS.
fn join_for(platform: Platform, base: PathBuf, component: &str) -> PathBuf {
    match platform {
        Platform::Mac => base.join(component),
        Platform::Windows => {
            let base = base.to_string_lossy();
            let base = base.trim_end_matches(['\\', '/']);
            PathBuf::from(format!("{base}\\{component}"))
        }
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
        let dir = join_for(Platform::Windows, PathBuf::from(p), "AppData");
        return join_for(Platform::Windows, dir, "Roaming");
    }
    PathBuf::from(env.get("HOME").unwrap_or("/")).join(".config")
}

fn local_appdata(env: &Env) -> PathBuf {
    if let Some(a) = env.get("LOCALAPPDATA") {
        return PathBuf::from(a);
    }
    if let Some(p) = env.get("USERPROFILE") {
        let dir = join_for(Platform::Windows, PathBuf::from(p), "AppData");
        return join_for(Platform::Windows, dir, "Local");
    }
    PathBuf::from(env.get("HOME").unwrap_or("/"))
        .join(".local")
        .join("share")
}
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
        assert_eq!(
            herdr_config_dir(&e, Platform::Mac),
            PathBuf::from("/Users/me/.config/herdr")
        );
    }

    #[test]
    fn herdr_config_dir_honours_xdg_on_both_platforms() {
        let e = env(&[("HOME", "/Users/me"), ("XDG_CONFIG_HOME", "/xdg")]);
        assert_eq!(
            herdr_config_dir(&e, Platform::Mac),
            PathBuf::from("/xdg/herdr")
        );
        let w = env(&[
            ("APPDATA", r"C:\Users\me\AppData\Roaming"),
            ("XDG_CONFIG_HOME", r"D:\xdg"),
        ]);
        assert_eq!(
            herdr_config_dir(&w, Platform::Windows),
            PathBuf::from(r"D:\xdg\herdr")
        );
    }

    #[test]
    fn herdr_config_dir_windows_uses_appdata_then_userprofile() {
        let e = env(&[("APPDATA", r"C:\Users\me\AppData\Roaming")]);
        assert_eq!(
            herdr_config_dir(&e, Platform::Windows),
            PathBuf::from(r"C:\Users\me\AppData\Roaming\herdr")
        );
        let e = env(&[("USERPROFILE", r"C:\Users\me")]);
        assert_eq!(
            herdr_config_dir(&e, Platform::Windows),
            PathBuf::from(r"C:\Users\me\AppData\Roaming\herdr")
        );
    }

    #[test]
    fn socket_path_follows_session_and_override_rules() {
        let e = env(&[("HOME", "/Users/me")]);
        assert_eq!(
            herdr_socket_path(&e, Platform::Mac, "default"),
            PathBuf::from("/Users/me/.config/herdr/herdr.sock")
        );
        assert_eq!(
            herdr_socket_path(&e, Platform::Mac, "work"),
            PathBuf::from("/Users/me/.config/herdr/sessions/work/herdr.sock")
        );
        let o = env(&[("HOME", "/Users/me"), ("HERDR_SOCKET_PATH", "/tmp/x.sock")]);
        assert_eq!(
            herdr_socket_path(&o, Platform::Mac, "work"),
            PathBuf::from("/tmp/x.sock")
        );
        let w = env(&[("APPDATA", r"C:\Users\me\AppData\Roaming")]);
        assert_eq!(
            herdr_socket_path(&w, Platform::Windows, "default").to_string_lossy(),
            r"C:\Users\me\AppData\Roaming\herdr\herdr.sock"
        );
    }

    #[test]
    fn bar_directories() {
        let e = env(&[("HOME", "/Users/me")]);
        assert_eq!(
            app_config_file(&e, Platform::Mac),
            PathBuf::from("/Users/me/.config/herdr-status/config.toml")
        );
        assert_eq!(
            app_data_dir(&e, Platform::Mac),
            PathBuf::from("/Users/me/Library/Application Support/herdr-status")
        );
        assert_eq!(
            app_log_dir(&e, Platform::Mac),
            PathBuf::from("/Users/me/Library/Logs/herdr-status")
        );
        let w = env(&[
            ("APPDATA", r"C:\Users\me\AppData\Roaming"),
            ("LOCALAPPDATA", r"C:\Users\me\AppData\Local"),
        ]);
        assert_eq!(
            app_config_file(&w, Platform::Windows),
            PathBuf::from(r"C:\Users\me\AppData\Roaming\herdr-status\config.toml")
        );
        assert_eq!(
            app_data_dir(&w, Platform::Windows),
            PathBuf::from(r"C:\Users\me\AppData\Local\herdr-status")
        );
        assert_eq!(
            app_log_dir(&w, Platform::Windows),
            PathBuf::from(r"C:\Users\me\AppData\Local\herdr-status\logs")
        );
    }

    #[test]
    fn posix_socket_path_string_for_wsl_probe() {
        assert_eq!(
            posix_socket_probe_path("default"),
            "${XDG_CONFIG_HOME:-$HOME/.config}/herdr/herdr.sock"
        );
        assert_eq!(
            posix_socket_probe_path("work"),
            "${XDG_CONFIG_HOME:-$HOME/.config}/herdr/sessions/work/herdr.sock"
        );
    }
}
