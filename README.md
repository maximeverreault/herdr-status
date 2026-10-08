# herdr-status

A small native status bar for the AI coding agents running under [herdr](https://herdr.dev):
one row per agent, sorted so anything that needs you is on top, with peek / keypad / message
quick actions and a tray icon. macOS and Windows. Built with Tauri 2 and Rust.

## What it watches

- **Local herdr**: the Unix socket (macOS) or named pipe (Windows) of the configured session (default `default`).
- **WSL** (Windows only): herdr running inside a WSL distro, reached through `wsl.exe`.
- **Remote machines over SSH**: Linux, macOS or native Windows hosts running herdr, reached with
  herdr's own `remote-api-bridge`. No `herdr machine add` needed; any SSH target works.

## Requirements

- herdr 0.9.3 or later on every machine you want to watch, with its server running.
- SSH key access to remote hosts (the bar never prompts for passwords; run `ssh host` once first).
- macOS 12+ or Windows 10/11. Linux is not supported.
- Windows uses WebView2 (preinstalled on Windows 10/11); macOS uses the system WebKit.

Requirements to build: Rust stable, bun 1.4, and Xcode Command Line Tools (macOS) or the MSVC build
tools (Windows).

## Install

Download the bundle from the GitHub Actions artifacts (`.dmg` for macOS, `.msi` or `.exe` for Windows),
or build it yourself:

    bun install --cwd ui
    cd ui && bun tauri build

Bundles land in `target/release/bundle/` (`dmg/`, `macos/`, `msi/`, `nsis/`).

The macOS app is ad-hoc signed, not notarized. After the first blocked launch, open System Settings,
Privacy & Security, and click "Open Anyway"; or run
`xattr -dr com.apple.quarantine /Applications/herdr-status.app`. The DMG built by CI is arm64 only. The Windows installer is unsigned:
SmartScreen asks once.

The first launch may pause for a few seconds (up to about 10 s on Windows) while the bar probes
`herdr machine list` and, on Windows, WSL. The window appears once that finishes.

## Configure

The config file is created on first launch and reloaded whenever you save it
(tray menu, *Edit config…*):

- macOS: `~/.config/herdr-status/config.toml`
- Windows: `%APPDATA%\herdr-status\config.toml`

```toml
[local]
enabled = true
session = "default"
# socket_path = "/custom/herdr.sock"   # overrides discovery

[[wsl]]                     # Windows only
label = "wsl"
# distro = "Ubuntu"
# session = "default"

[[remote]]                   # example entry; use your own host
label = "winbox"
ssh_target = "winbox"       # alias, user@host, ssh://user@host:2222
session = "default"
# os = "windows"            # auto-detected when omitted
# herdr_path = "C:\\...\\herdr.exe"

[ui]
peek_lines = 30             # 1-500
reconcile_seconds = 30      # minimum 5
```

On first launch the bar prefills `[[remote]]` entries from `herdr machine list` and, on Windows,
detects herdr inside the default WSL distro. On Windows the prefill may set `[local] enabled = false`
when no native herdr is found. If `config.toml` already exists the prefill does not run again
(delete the file to re-prefill).

`HERDR_SOCKET_PATH` and `XDG_CONFIG_HOME` are honoured.

Notes:

- The config directory is watched, not the file. If `config.toml` is a symlink, it only live-reloads
  when something changes in the link's own directory; editing the symlink target does not trigger a
  reload (restart the app or touch the link).
- Changing only `ui.reconcile_seconds` does not restart running watchers.
- Launch at login (tray menu) uses a LaunchAgent on macOS (`~/Library/LaunchAgents/`) and the `Run`
  registry key on Windows; both start the app with `--hidden`. It is only meaningful for an installed
  (bundled) build; a dev build would register a path under `target/debug`.

## Use

- Rows: amber = needs you, blue = working, green = done, grey = idle. The right-hand text is the
  time in that state since the bar saw it change.
- Click a row for the drawer: **Peek** (last lines of the terminal, refreshed every 2 s),
  **Keys**, **Message** (one-line prompt). Escape closes it.

| Keypad | `1` `2` `3` `y` `n` `↑` `↓` `Enter` `Esc` `^C` |
|---|---|

- Tray / menu bar: colour follows the most urgent state; the count of agents needing you is shown
  next to the macOS icon or as a badge on Windows. Left click toggles the window (on Windows by its
  current visibility); right click: Show / Hide, Reconnect all, Edit config…, Open logs,
  Launch at login, Quit. Closing the window only hides it.
- A second launch focuses the existing window.

## Remotes

SSH connections use a ControlMaster socket under `/tmp/herdr-status-<user>` on macOS. `ControlPersist`
keeps a master connection alive for up to 10 minutes after you quit the app; that is by design.
Windows clients have no SSH connection sharing: each peek on a remote opens a new ssh + bridge.

## Troubleshooting

- Chip stays grey with *herdr is not running*: `herdr status` should report a running server.
- Remote offline with an SSH reason: run the suggested `ssh <target>` in a terminal, then
  *Reconnect all*.
- *herdr not found or too old on <host>*: install or update herdr there; it needs
  `remote-api-bridge` (0.9.x).
- Logs: tray menu, *Open logs*, or look in `~/Library/Logs/herdr-status/` (macOS) or
  `%LOCALAPPDATA%\herdr-status\logs` (Windows). Set `HERDR_STATUS_LOG=debug` (or `trace`) before
  launching for more detail.

## Known limitations

- Rows are not keyboard-accessible (no tabindex/role); a follow-up idea.
- The dim text and group labels are below WCAG AA contrast at 10-12 px (design tokens from the spec).

## Development

    cargo test --workspace --features herdr-client/test-support
    cd ui && bun run test
    cd ui && bun tauri dev

Opt-in live test against your local herdr:
`HERDR_STATUS_LIVE=1 cargo test -p herdr-client --test live_herdr -- --nocapture`.

Manual checklists: `docs/macos-checklist.md`, `docs/windows-smoke-checklist.md`.
Design: `docs/superpowers/specs/2026-10-07-herdr-status-bar-design.md`.
Plans: `docs/superpowers/plans/`.
