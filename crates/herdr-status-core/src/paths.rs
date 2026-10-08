//! Where things live: herdr's socket (mirrors herdr's own `config_dir()` rules)
//! and the bar's config, data and log directories.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Windows,
}

impl Platform {
    /// Windows on Windows hosts; every other host maps to `Mac` (Linux is out of scope).
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

    /// Looks up a variable; empty values count as unset.
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
/// Assumes `session` was validated (see `Config::validate`); the caller must
/// place the result inside double quotes so the variables expand.
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

/// Directory for ssh ControlMaster sockets. Short on purpose: a Unix socket
/// path must fit `sun_path` (104 bytes on macOS). The hub creates it with mode
/// 0700. Windows OpenSSH has no ControlMaster, so there is none.
pub fn ssh_control_dir(env: &Env, platform: Platform) -> Option<PathBuf> {
    match platform {
        Platform::Windows => None,
        Platform::Mac => {
            let user: String = env
                .get("USER")
                .unwrap_or("default")
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            Some(PathBuf::from(format!("/tmp/herdr-status-{user}")))
        }
    }
}

/// Make sure `path` is a private (mode 0700) directory owned by us and return it,
/// or `None` if that cannot be guaranteed (callers then run without connection
/// sharing). Called before every SSH connection because macOS periodically
/// cleans `/tmp`, and OpenSSH refuses to start with a missing ControlPath
/// directory.
///
/// The directory lives in world-writable `/tmp`, so a pre-existing entry may
/// belong to someone else: symlinks and non-directories are refused. Ownership
/// check: we create a marker file inside (this only succeeds if the directory is
/// writable by us) and compare its uid, which is our effective uid, with the
/// directory's uid. A directory that is ours with a looser mode is repaired to 0700.
#[cfg(unix)]
pub fn ensure_control_dir(path: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let check = || -> std::io::Result<bool> {
        if let Err(e) = std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
        {
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(e);
            }
        }
        let meta = std::fs::symlink_metadata(path)?;
        if !meta.is_dir() {
            return Ok(false);
        }
        let marker = path.join(format!(".owner-check-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)?;
        let marker_meta = std::fs::symlink_metadata(&marker);
        let _ = std::fs::remove_file(&marker);
        if meta.uid() != marker_meta?.uid() {
            return Ok(false);
        }
        if meta.mode() & 0o777 != 0o700 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            return Ok(std::fs::symlink_metadata(path)?.mode() & 0o777 == 0o700);
        }
        Ok(true)
    };
    match check() {
        Ok(true) => Some(path.to_path_buf()),
        Ok(false) => {
            tracing::warn!(dir = %path.display(), "ssh control dir is not a private directory we own; connection sharing disabled");
            None
        }
        Err(error) => {
            tracing::warn!(dir = %path.display(), %error, "cannot prepare ssh control dir; connection sharing disabled");
            None
        }
    }
}

#[cfg(not(unix))]
pub fn ensure_control_dir(_path: &Path) -> Option<PathBuf> {
    None
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

    #[test]
    fn windows_named_session_socket_path() {
        let w = env(&[("APPDATA", r"C:\Users\me\AppData\Roaming")]);
        assert_eq!(
            herdr_socket_path(&w, Platform::Windows, "work").to_string_lossy(),
            r"C:\Users\me\AppData\Roaming\herdr\sessions\work\herdr.sock"
        );
    }

    #[test]
    fn ssh_control_dir_is_short_tmp_path_on_mac_only() {
        let e = env(&[("USER", "me")]);
        assert_eq!(
            ssh_control_dir(&e, Platform::Mac),
            Some(PathBuf::from("/tmp/herdr-status-me"))
        );
        assert_eq!(ssh_control_dir(&e, Platform::Windows), None);
        let weird = env(&[("USER", "a b/c\"%")]);
        assert_eq!(
            ssh_control_dir(&weird, Platform::Mac),
            Some(PathBuf::from("/tmp/herdr-status-a_b_c__"))
        );
        assert_eq!(
            ssh_control_dir(&env(&[]), Platform::Mac),
            Some(PathBuf::from("/tmp/herdr-status-default"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn ensure_control_dir_creates_repairs_and_refuses() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let tmp = tempfile::tempdir().unwrap();
        let fresh = tmp.path().join("fresh");
        assert_eq!(ensure_control_dir(&fresh), Some(fresh.clone()));
        assert_eq!(std::fs::metadata(&fresh).unwrap().mode() & 0o777, 0o700);

        let loose = tmp.path().join("loose");
        std::fs::create_dir(&loose).unwrap();
        std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(ensure_control_dir(&loose), Some(loose.clone()));
        assert_eq!(std::fs::metadata(&loose).unwrap().mode() & 0o777, 0o700);

        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&fresh, &link).unwrap();
        assert_eq!(ensure_control_dir(&link), None);

        let file = tmp.path().join("file");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(ensure_control_dir(&file), None);
    }
}
