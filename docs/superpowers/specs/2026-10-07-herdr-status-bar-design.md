# herdr-status: a native agent status bar for herdr

Design spec. Date: 2026-10-07. Status: approved in brainstorming, pending written review.

## 1. Summary

herdr-status is a small desktop app for macOS and Windows that shows, at a glance, the state of every AI coding agent running under herdr: locally, inside WSL on the same Windows machine, and on remote machines over SSH. It is a narrow vertical window meant to sit on the edge of a second monitor, plus a tray / menu bar icon. Rows are sorted by urgency so an agent that needs the user is always at the top. Each row offers three quick actions: peek at recent output, answer a blocked prompt with a keypad, and send a one-line message.

Built with Tauri 2 (Rust core, system webview UI). One codebase for both platforms.

## 2. Goals and non-goals

Goals

- Readable at a glance: dark panel, one row per agent, urgency-sorted, state colours, a one-second flash on change.
- Live: state changes appear within a second via herdr's event subscription, not polling.
- Multi-server: local herdr, herdr in WSL on the same Windows machine, remote herdr over SSH (Linux, macOS, native Windows hosts).
- Quick actions without leaving the bar: peek, keypad, message.
- Lives in the tray: close hides, launch at login, count in the tray.

Non-goals (see section 19 for the full list)

- Focusing or jumping to a pane in herdr.
- Remote machines where herdr runs inside WSL.
- Notifications or sounds (herdr already provides them).
- Settings UI, light theme, Linux packaging, auto-update, code signing.

## 3. Decisions made during brainstorming

| Topic | Decision |
|---|---|
| Layout | Compact rows (one line per agent), sorted by urgency |
| Row click | Opens an action drawer; never focuses the pane |
| Quick actions | Peek recent output, keypad for blocked prompts, send a short message |
| Servers | Local + WSL on the bar's own machine + remote via SSH; remotes may be native Windows hosts running herdr |
| Window | Normal resizable window, position and size remembered, tray / menu bar icon, launch at login, close hides |
| Attention | Visual only plus a one-second flash on the changed row |
| Stack | Tauri 2 with a Rust core; the bar runs its own SSH bridge rather than `herdr --machine` |
| Theme | Dark only |

## 4. herdr facts this design relies on

Verified against herdr 0.9.3 (protocol 22) on 2026-10-07, by reading its source and probing the live local server. The bar must not assume anything about herdr beyond this list.

Transport and framing

- The API is newline-delimited JSON over a local socket. Request: `{"id":"<string>","method":"<dotted name>","params":{...}}`. Success: `{"id":..,"result":{"type":"<name>",...}}`. Error: `{"id":..,"error":{"code":"<snake_case>","message":"..."}}`.
- A connection serves exactly one normal request; the server closes it after the response. Only `events.subscribe` keeps a connection open. Sending a second request on a used connection fails.
- macOS/Linux: Unix domain socket. Windows: named pipe. Path resolution (highest priority first): `HERDR_SOCKET_PATH`; `HERDR_SESSION=<name>`; default session.
- Config directory: `$XDG_CONFIG_HOME/herdr` if set; else macOS/Linux `~/.config/herdr`; Windows `%APPDATA%\herdr` (fallback `%USERPROFILE%\AppData\Roaming\herdr`). Default session socket: `<config dir>/herdr.sock`. Named session: `<config dir>/sessions/<name>/herdr.sock`.
- On Windows herdr opens the pipe through the `interprocess` crate's `GenericNamespaced` mapping, which prepends `\\.\pipe\` to the path string verbatim. So the pipe name is `\\.\pipe\C:\Users\<u>\AppData\Roaming\herdr\herdr.sock`. herdr also writes a marker file at that path (`<pid>:<nanos>`).
- `herdr status` prints `socket: <path>` for the running server.

Methods the bar uses

| Method | Params | Result type |
|---|---|---|
| `ping` | `{}` | `pong` with `version`, `protocol`, `capabilities` |
| `session.snapshot` | `{}` | `session_snapshot` with `snapshot.{agents, panes, tabs, workspaces, focused_*}` |
| `events.subscribe` | `{"subscriptions":[...]}` | `subscription_started`, then pushed events |
| `agent.read` | `{"target","source":"recent_unwrapped","lines":N,"format":"text","strip_ansi":true}` | `pane_read` with `read.text`, `read.truncated` |
| `agent.send_keys` | `{"target","keys":["1"]}` | `ok` |
| `agent.prompt` | `{"target","text","wait":null}` | `agent_prompted` |

`target` for agent methods is a pane id such as `w1:p1` or a live agent name. The bar always uses the pane id.

Agent and pane records

- `agent_status` is one of `idle`, `working`, `blocked`, `done`, `unknown`. `idle` and `done` both mean ready for input; `done` is a completion the user has not yet looked at, and only an explicit focus in herdr marks it seen. `blocked` means herdr recognised an approval or question UI. `unknown` means an agent is present but unclassified.
- Snapshot `agents[]` contains only panes with a detected agent. Fields used: `pane_id`, `workspace_id`, `tab_id`, `agent` (kind label such as `claude`, `codex`), `agent_status`, `name` (live agent name, may be null), `title` (presentation title, may be null), `display_agent`, `terminal_title_stripped`, `terminal_title`, `focused`, `cwd`. `panes[]` contains every pane (used for the subscription set). `workspaces[]` gives `workspace_id` and `label`. PaneInfo may also carry `label`, `title`, `display_agent`, `state_labels`.
- There are no timestamps in snapshots or events. Elapsed time must be measured by the bar.

Subscriptions

- Lifecycle subscription entries take only `{"type": ...}`: `workspace.created|updated|metadata_updated|renamed|moved|reordered|closed|focused`, `worktree.*`, `tab.created|closed|focused|renamed|moved`, `pane.created|closed|updated|focused|moved|exited|agent_detected`, `layout.updated`.
- Per-pane entries: `{"type":"pane.agent_status_changed","pane_id":"w1:p1"}` with optional `agent_status` filter. There is no server-wide agent-status subscription. If any `pane_id` in the request does not exist, the whole request is rejected and the connection closes.
- Pushed per-pane status event: `{"event":"pane.agent_status_changed","data":{"pane_id","workspace_id","agent_status","agent","title","display_agent","state_labels"}}` (dotted name). Pushed lifecycle events are herdr's raw envelope with snake_case names and a `type` field repeated inside `data`: `{"event":"pane_closed","data":{"type":"pane_closed","pane_id":"w1:p2","workspace_id":"w1"}}`; `pane_created` and `pane_updated` carry `data.pane` as a full PaneInfo. Clients must accept both naming styles.
- `pane.updated` is not emitted on status changes, so per-pane subscriptions are required.
- Without an `agent_status` filter, no initial event is sent; the snapshot provides initial state.
- If a subscriber falls behind, the server sends `{"error":{"code":"events_lost"}}` and closes the connection.
- Verified live: per-pane status events arrived within one second of the agent changing state.

Remote API bridge

- Hidden subcommand: `herdr --session <name> remote-api-bridge` connects to that session's local socket and forwards stdin/stdout to it, one socket connection per process, with no idle timeout. `herdr --session <name> remote-api-bridge --check` prints `herdr-api-bridge-v1` and exits 0.
- herdr's own saved-machine client launches one `ssh` exec per API connection and prints a ready marker line `herdr-remote-output-ready:1` before the stream so SSH banners can be skipped.
- POSIX remote command, exactly as herdr builds it: `/bin/sh -c '<script>'` where the script is `printf '\n%s\n' 'herdr-remote-output-ready:1'` followed by `exec <herdr> --session <session> remote-api-bridge`.
- Windows remote command: `powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand <base64 of UTF-16LE script>` where the script is `[Console]::Out.WriteLine(); [Console]::Out.WriteLine('herdr-remote-output-ready:1'); [Console]::Out.Flush(); $process = Start-Process -FilePath <path> -ArgumentList '--session <session> remote-api-bridge' -NoNewWindow -PassThru -ErrorAction Stop; $null = $process.Handle; $process.WaitForExit(); exit $process.ExitCode`. Start-Process is used so the child inherits raw stdio handles instead of PowerShell's text pipeline.
- Platform detection as herdr does it: `uname -s` and `uname -m` over SSH; if that fails with an exit code other than 255 (255 is SSH itself failing), run a PowerShell probe that prints `herdr-windows:<arch>`.
- Key names accepted by `agent.send_keys`: single characters as themselves, plus `enter`, `tab`, `backspace`, `esc` (also `escape`), `up`, `down`, `left`, `right`, `space`, and modifier combinations such as `ctrl+c`.
- SSH options herdr uses for background bridges: `ServerAliveInterval=15`, and on Unix clients `ControlMaster=auto`, `ControlPersist=600`, a private `ControlPath`.

## 5. Architecture

```
herdr (local socket | wsl.exe bridge | ssh bridge)
        │  newline JSON
        ▼
crates/herdr-client        Transport trait, framing, typed requests/responses/events
        │
        ▼
crates/herdr-status-core   Server + Watcher tasks, reducer, sorted AgentList, config, launchers
        │  AgentList snapshots (serde)        ▲ quick-action calls
        ▼                                     │
src-tauri                  Tauri app: tray, window, autostart, commands, events to UI
        │  Tauri event "agents" / commands    ▲
        ▼                                     │
ui/                        Vite + TypeScript, no framework: rows, drawer, flash, chips
```

Data flows one way from herdr to the UI. UI actions come back as Tauri commands that run a one-shot request on a fresh connection. The core never touches the UI, and the UI never parses herdr JSON.

Everything that talks to herdr lives in `herdr-client` and is testable against an in-process fake server. Everything that decides what to show lives in `herdr-status-core` and is testable with plain data. `src-tauri` is glue and stays thin.

## 6. Protocol crate: `herdr-client`

Public surface

```rust
pub trait Transport: Send + Sync {
    /// Opens a fresh connection. One request per connection, except subscriptions.
    async fn connect(&self) -> Result<Box<dyn Connection>, TransportError>;
    fn describe(&self) -> String; // for logs and chip tooltips
}
pub trait Connection: Send {
    async fn send(&mut self, req: &Request) -> Result<(), TransportError>;
    async fn next_line(&mut self) -> Result<Option<String>, TransportError>; // None = EOF
}
pub struct Client<T: Transport> { .. }
impl Client {
    pub async fn request(&self, method: Method) -> Result<ResponseResult, ApiError>; // connect, send, read one line, close
    pub async fn subscribe(&self, subs: Vec<Subscription>) -> Result<EventStream, ApiError>; // connect, send, read ack, keep reading
}
```

Types: `Request`, `Method` (an enum with one variant per method in section 4, serialising to the dotted name plus params), `ResponseResult` (tagged by `type`, with `#[serde(other)]` style tolerance), `ApiError { code, message }`, `Subscription`, `Event` (tagged by `event`), `SessionSnapshot`, `AgentInfo`, `PaneInfo`, `WorkspaceInfo`, `AgentStatus`. All structs use `#[serde(default)]` on optional fields and ignore unknown fields, as herdr's docs require of JSON clients.

Request ids are `hs-<counter>`; the client checks the echoed id and treats a mismatch as a protocol error.

Transports

- `UnixSocketTransport { path }`: `tokio::net::UnixStream`. macOS and Linux.
- `NamedPipeTransport { pipe_name }`: `tokio::net::windows::named_pipe::ClientOptions::new().open(name)` with the documented `ERROR_PIPE_BUSY` retry (50 ms sleep, up to 2 s). Windows only.
- `BridgeTransport { launcher }`: spawns a child process per connection (`ssh …`, `wsl.exe …`, or a local `herdr.exe … remote-api-bridge` on Windows), discards stdout lines until the ready marker `herdr-remote-output-ready:1`, then treats stdout as the JSON stream and stdin as the request side. If the marker does not arrive within 20 s, or the child exits first, the connection fails with the captured stderr as the message. Children are spawned with `kill_on_drop(true)`, and on Windows with the `CREATE_NO_WINDOW` creation flag.

A `FakeHerdrServer` test helper (behind a `test-support` feature) listens on a temp Unix socket, speaks the protocol with scripted responses, enforces one-request-per-connection, and can push events and `events_lost` on demand. On Windows CI it listens on a named pipe instead so the pipe transport is covered.

## 7. Server kinds and launchers

Each configured server is one of three kinds. All three end up as a `Transport`.

Local (macOS and native Windows)

1. Socket path: `[local].socket_path` if set; else `HERDR_SOCKET_PATH` if set in the bar's environment; else derived from `[local].session` and the config directory rule in section 4.
2. macOS: `UnixSocketTransport`.
3. Windows: `NamedPipeTransport` with `\\.\pipe\` + the path string. If opening fails with `NotFound` while the marker file exists and `herdr.exe` is on PATH and `herdr status` reports a running server, fall back to `BridgeTransport` with a local launcher `herdr.exe --session <s> remote-api-bridge` and log a warning once. This covers the one piece (pipe naming) that cannot be verified on the Mac before the Windows smoke test.
4. "Not running" is detected by connect failure (`NotFound`, `ConnectionRefused`, or missing socket file). Retry every 5 s.

WSL (Windows only)

- Launcher: `wsl.exe [-d <distro>] --exec sh -lc '<posix bridge script>'` with the same POSIX script as remotes (discovery, `--check`, ready marker, `exec`). `--exec` passes the arguments straight to `sh` without the distro's default shell in between, so no extra quoting layer. Login shell so `~/.local/bin` is on PATH.
- On first launch on Windows with no `[[wsl]]` entry, the bar probes the default distro once: `wsl.exe -- sh -lc 'test -S "${XDG_CONFIG_HOME:-$HOME/.config}/herdr/herdr.sock"'`. If it succeeds, a `[[wsl]]` entry with label `wsl` is written to the config.
- On failure the server goes offline with the stderr text as the reason and retries with the same backoff as remotes.

Remote (SSH)

- Launcher: `ssh -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3 [control options] <ssh_target> <remote command>`. The user's own SSH config is honoured (no `-F`). Control options on macOS only: `-o ControlMaster=auto -o ControlPersist=600 -o ControlPath=<cache dir>/cm/%C`. Windows OpenSSH has no connection sharing, so each connection pays a handshake; acceptable because only the subscription stream, the 30 s reconcile and user-triggered actions open connections.
- First contact, in order: platform detection (section 4); herdr discovery; `--check`. Results (`os`, `arch`, `herdr_path`, `checked_at`) are cached in `cache.json` keyed by `ssh_target` and reused until a bridge start fails or `--check` fails, which clears the entry.
- POSIX discovery script, run as one `ssh` command: for each candidate in order, if it is executable and `<candidate> --session <s> remote-api-bridge --check` prints `herdr-api-bridge-v1`, print the candidate and exit 0. Candidates: `command -v herdr` (skipping any `*/mise/shims/herdr`), `$HOME/.local/bin/herdr`, `/opt/homebrew/bin/herdr`, `/usr/local/bin/herdr`, `/home/linuxbrew/.linuxbrew/bin/herdr`, `$HOME/.nix-profile/bin/herdr`. Exit 2 with a message if none pass. `[[remote]].herdr_path` skips discovery.
- Windows discovery, one PowerShell command: `Get-Command herdr.exe` then `$env:LOCALAPPDATA\Programs\Herdr\bin\herdr.exe`; each candidate must pass `--check`.
- Remote command strings are built by `herdr_status_core::launchers` and must equal herdr's own strings byte for byte (tested).

Launcher summary

| Kind | Child process | Stream |
|---|---|---|
| local macOS | none | Unix socket |
| local Windows | none (fallback: `herdr.exe … remote-api-bridge`) | named pipe |
| wsl | `wsl.exe [-d D] -- sh -lc '<posix script>'` | child stdio |
| remote posix | `ssh … <target> /bin/sh -c '<posix script>'` | child stdio |
| remote windows | `ssh … <target> powershell.exe … -EncodedCommand …` | child stdio |

## 8. Watcher

One `Watcher` task per server. It owns one long-lived subscription connection and opens short-lived connections for everything else.

```
state = Connecting
loop:
    S1 = request(session.snapshot)                 # fails → Offline(reason), backoff, continue
    pane_ids = S1.panes[].pane_id
    stream = subscribe(lifecycle_types + [pane.agent_status_changed for each pane_id])
    S2 = request(session.snapshot)                 # authoritative state after subscribing
    apply_snapshot(S2); state = Online
    reconcile = every config.ui.reconcile_seconds
    dirty = debounce(250 ms)
    select:
        event pane.agent_status_changed  → apply_status(pane_id, status, agent, title)
        event pane.created | pane.closed | pane.exited | pane.agent_detected
                                         → dirty.trigger(rebuild = true)
        event pane.updated | workspace.* | tab.* | layout.updated
                                         → dirty.trigger(rebuild = false)
        dirty fired, rebuild = false     → apply_snapshot(request(session.snapshot))
        dirty fired, rebuild = true      → break to top of loop (new subscription set)
        reconcile fired                  → apply_snapshot(request(session.snapshot))
        stream error events_lost | EOF | parse error on the ack
                                         → break to top of loop
        shutdown                         → return
backoff: 1 s, doubling to a 60 s cap, reset after 60 s online. Offline state carries the reason and the next retry time for the UI.
```

Subscribe-then-snapshot is the ordering herdr's docs recommend: events invalidate cached state, and the snapshot after subscribing is authoritative. A status event that arrives before S2 is applied and then overwritten by S2, which is newer.

Lifecycle types requested: `pane.created`, `pane.closed`, `pane.updated`, `pane.exited`, `pane.agent_detected`, `workspace.created`, `workspace.closed`, `workspace.renamed`, `workspace.updated`, `workspace.metadata_updated`, `tab.created`, `tab.closed`, `tab.renamed`.

## 9. State model and reducer

```rust
pub enum AgentStatus { Blocked, Working, Done, Idle, Unknown }   // declaration order = urgency rank

pub struct AgentRow {
    pub server_id: String,          // config label
    pub pane_id: String,
    pub workspace_label: String,
    pub agent_kind: Option<String>, // "claude", "codex", …
    pub name: String,
    pub status: AgentStatus,
    pub since: Option<SystemTime>,  // when the bar observed the current status
    pub flash_seq: u64,             // increments on every observed status change; UI flashes on change
}

pub enum Connection { Connecting, Online, Offline { reason: String, retry_at: SystemTime } }

pub struct ServerState { pub id: String, pub label: String, pub kind: ServerKind, pub connection: Connection, pub rows: Vec<AgentRow> }

pub struct AgentList {                // what the UI receives, already sorted
    pub servers: Vec<ServerChip>,     // id, label, connection
    pub rows: Vec<AgentRow>,          // online servers only, urgency-sorted
    pub offline: Vec<OfflineGroup>,   // label, reason, retry_in_seconds, last known rows
    pub counts: Counts,               // blocked, working, done, idle, unknown
}
```

Rules

- A row exists for every entry in `snapshot.agents[]` (panes with a detected agent). Panes without an agent are not rows.
- `name`, first non-empty of: the agent record's `name` (the live agent name set by `herdr agent start` or `rename`); its `title` (herdr's presentation title); `terminal_title_stripped`; `"<agent_kind> · <workspace_label>"`.
- `workspace_label` comes from `snapshot.workspaces[]` by `workspace_id`; falls back to the id.
- `apply_snapshot`: rows are rebuilt from the snapshot. For a pane that already had a row, `since` and `flash_seq` are kept if the status is unchanged, otherwise `since = now` and `flash_seq += 1`. New rows start with `since = None` and `flash_seq = 0` so the first snapshot never flashes.
- `apply_status(event)`: same rule for the one pane; if the pane has no row yet (agent detected between snapshots) a snapshot is requested instead of inventing a row.
- Sort key for `rows`: `(status rank, since desc with None last, server label, name)`. Stable.
- A server going offline moves its rows into `offline` unchanged and keeps them until the server is back and a fresh snapshot replaces them.
- `counts` are computed over online rows only.

The reducer is a pure function `(ServerState, Input) -> ServerState` plus a pure `assemble(&[ServerState]) -> AgentList`, both unit-tested with sequences.

## 10. Quick actions

Exposed as Tauri commands, each running one one-shot request through the server's transport.

| Command | herdr call | Notes |
|---|---|---|
| `peek(server, pane)` | `agent.read` with `source=recent_unwrapped`, `lines=ui.peek_lines`, `format=text`, `strip_ansi=true` | Returns `text` and `truncated`. UI polls it every 2 s while the Peek tab is open. |
| `send_key(server, pane, key)` | `agent.send_keys` with `keys=[key]` | Keypad keys: `1`, `2`, `3`, `y`, `n`, `up`, `down`, `enter`, `esc`, `ctrl+c`. The command validates against this allow-list. |
| `send_message(server, pane, text)` | `agent.prompt` with `wait=null` | Text is trimmed; empty text is rejected before calling. |

Errors from herdr are returned to the UI as `{ code, message }` and shown inline in the drawer for 5 s. Known codes get friendlier text: `agent_blocked` → "Waiting at a prompt. Answer it with the keypad first."; `not_found` → "That agent is gone."; a transport error → "Could not reach <server>."

No action runs from the row click itself; every action needs a second, explicit click inside the drawer.

## 11. UI

Technology: Vite, TypeScript, plain DOM, one `styles.css`. State arrives as `AgentList` on the Tauri event `agents`; the UI keeps the last list and re-renders with keyed rows so animations work.

Panel anatomy (top to bottom)

- Header: the word `HERDR` (11 px, uppercase, letter-spaced, muted) on the left; on the right one chip per server showing its label and a 6 px dot: green online, grey offline, grey pulsing while connecting. Chip tooltip: transport description and, when offline, the reason and retry countdown.
- Rows (online servers), 40 px tall, 8 px vertical padding, 10 px gap: a 9 px status dot with a soft glow; the name (13 px, 600 weight, ellipsised); the meta line (11 px, muted): `<server> · <workspace> · <agent kind>`; right-aligned state text (11 px, tabular numerals).
- Offline groups: a 10 px uppercase group line `<server label>` with `offline · retrying 30s` (or the reason) on the right, followed by that server's last rows at 40 % opacity.
- Empty states: online with no rows → "No agents running"; local offline and nothing else configured → "herdr is not running".

State text on the right

| Status | Text |
|---|---|
| blocked | `needs you · 2m` (amber, bold) |
| working | `4m` |
| done | `done 12m` (green) |
| idle | `idle 25m` |
| unknown | `—` |

When `since` is `None` the duration part is omitted (`needs you`, `working`, `done`, `idle`). Durations: `<60s` → `just now`; minutes `12m`; hours `3h 05m`; days `2d`. Refreshed every 15 s client-side.

Colours

| Token | Value |
|---|---|
| panel background | `#0f1115` |
| row hover | `#151922` |
| borders | `#262a33` |
| text | `#e6e8ee` |
| muted text | `#8a90a0` |
| dim text | `#6b7180` |
| working | `#3b9dff` |
| blocked | `#ff9f0a`, row background `#2a1a0c`, text `#ffb067` |
| done | `#34c759`, text `#5fe39a` |
| idle | `#5c6270` |
| unknown | hollow dot, border `#5c6270` |
| flash | background from `#1f3a5f` to transparent over 1 s, ease-out |

Font: `system-ui, -apple-system, "Segoe UI", sans-serif`; peek output in `ui-monospace, Menlo, Consolas, monospace` at 10.5 px.

Motion

- Flash: when a row's `flash_seq` changes, replay the flash animation on it.
- Reorder: FLIP transition of 200 ms when a row changes position.
- A new row fades in over 150 ms; a row that disappears is removed immediately.
- Everything respects `prefers-reduced-motion` (no reorder or fade, flash becomes a 1 s static highlight).

Drawer

- Clicking a row toggles its drawer directly beneath it; opening one closes any other. The drawer closes when its row disappears or on Escape.
- Tabs: Peek, Keys, Message. Peek is the default. For a blocked row the keypad is also rendered under the Peek output so answering is two clicks.
- Peek: the last `peek_lines` lines in a monospace box, max height about 120 px, scrolled to the bottom, refreshed every 2 s while visible, with a thin "truncated" note when herdr reports truncation.
- Keys: a 5-column keypad in this order: `1`, `2`, `3`, `y`, `n`, `↑`, `↓`, `Enter`, `Esc`, `^C`. `1` and `Enter` are styled as primary (green tint), `Esc` and `^C` as caution (amber tint).
- Message: a single-line input and a Send button; Enter sends; the field clears on success and keeps the text on error.
- Errors show as one red line at the bottom of the drawer for 5 s.

## 12. Tray, window, autostart

Window

- One main window: title `herdr-status`, default size 300 × 820, minimum 260 × 300, resizable, native title bar, dark background set in both the Tauri config and CSS so there is no white flash.
- Position and size persist via the official window-state plugin.
- The close button hides the window instead of quitting (close-requested event → hide). Quit is only in the tray menu.
- Single-instance plugin: launching a second copy shows and focuses the existing window.

Tray / menu bar

- Built with Tauri's `tray-icon` feature. Icon colour follows the highest-priority state across online servers: amber if any blocked, else blue if any working, else green if any done, else grey. Icons are rendered at runtime into an RGBA buffer (a coloured disc, plus on Windows a small badge disc with the count drawn from a built-in 3×5 bitmap font, `9+` above nine) and handed to Tauri as `Image::new_owned`; no icon assets to generate or commit.
- macOS: `set_title` shows the blocked count as text next to the icon when it is above zero, and clears it otherwise; the badge-less icon is used.
- Windows: the badged icon is used; `set_tooltip` reads `herdr-status: 1 needs you · 2 working · 1 done`.
- Left click toggles the window (`show_menu_on_left_click(false)`). Right click opens the menu: Show / Hide, Reconnect all, Edit config…, Open logs, Launch at login (checkbox, via the autostart plugin), Quit.
- Reconnect all: every watcher drops its current connection, clears its backoff and reconnects immediately; remote discovery caches are kept.

Autostart: the autostart plugin registers a Login Item on macOS and a Run registry entry on Windows, with `--hidden` so the app starts in the tray without raising the window. The checkbox reflects the current registration. When the app is started with `--hidden` (by autostart or by hand) the main window is created but not shown until the tray is clicked.

## 13. Configuration

File: macOS `~/.config/herdr-status/config.toml`; Windows `%APPDATA%\herdr-status\config.toml`. Created with commented defaults on first launch.

```toml
[local]
enabled = true
session = "default"             # named session → <herdr config>/sessions/<name>/herdr.sock
# socket_path = "/custom/herdr.sock"   # overrides session

[[wsl]]                         # Windows only; ignored elsewhere
label = "wsl"
# distro = "Ubuntu"             # default distro when omitted
session = "default"

[[remote]]
label = "winbox"
ssh_target = "winbox"           # any ssh target: alias, user@host, ssh://user@host:2222
session = "default"
# os = "windows"                # "posix" | "windows"; auto-detected and cached when omitted
# herdr_path = "C:\\Users\\me\\AppData\\Local\\Programs\\Herdr\\bin\\herdr.exe"

[ui]
peek_lines = 30
reconcile_seconds = 30
```

Rules

- `label` must be unique across all servers, non-empty, and is used as `server_id`. Validation errors (duplicate label, bad TOML, unknown keys) are shown as a red banner at the top of the panel with the file path and line, logged, and the previous valid config stays active.
- The file is watched (`notify` crate, 500 ms debounce). Added, removed or changed servers are started or stopped individually; unchanged servers are untouched.
- First launch prefill: if `herdr machine list --json` is available and returns entries, each becomes a `[[remote]]` with `label` = machine label, `ssh_target` = its target, `session` = its session. On Windows, the WSL probe in section 7 may add a `[[wsl]]`, and if no `herdr.exe` is on PATH and no socket marker file exists, `[local].enabled` is written as `false` so a WSL-only machine does not show a permanent "not running" chip. Prefill writes the file once and never again.
- "Edit config…" in the tray opens the file with the OS default handler (`open` / `start`).

Other files: macOS `~/Library/Application Support/herdr-status/` and `~/Library/Logs/herdr-status/`; Windows `%LOCALAPPDATA%\herdr-status\` for `cache.json`, the SSH control directory `cm/`, and `logs/`.

## 14. Error handling

| Situation | Behaviour |
|---|---|
| Local herdr not running | Chip grey, rows removed, "herdr is not running" empty state if nothing else is configured, retry every 5 s |
| Local Windows pipe cannot be opened but herdr reports running | Fall back to the local process bridge, warn once in the log |
| `wsl.exe` missing or distro not found | Server offline with the stderr text as reason, retry with backoff |
| SSH auth denied | Reason "SSH authentication failed; run `ssh <target>` once in a terminal" |
| SSH host key problem | Reason "Host key verification failed; connect with `ssh <target>` to accept it" |
| SSH unreachable or timeout | Reason "Unreachable", backoff |
| herdr not found on remote or `--check` fails | Reason "herdr not found or too old on <target>", cache entry cleared, backoff |
| Ready marker not seen within 20 s | Reason "bridge did not start" with stderr tail |
| `events_lost` or stream EOF | Rebuild subscription, no UI change unless it keeps failing |
| Malformed JSON line | Logged with the line, skipped |
| Protocol number differs from 22 | Logged once at info level; the bar continues (unknown fields ignored) |
| Quick action error | Inline drawer message for 5 s (section 10) |
| Config invalid | Banner, previous config kept |
| Second instance launched | Existing window shown |

Passwords are never prompted (`BatchMode=yes`). SSH agent and keys are the user's responsibility, as with herdr itself.

## 15. Logging

`tauri-plugin-log` to a rotating file in the logs directory, info level by default, debug with `HERDR_STATUS_LOG=debug`. Every connection attempt, state transition, rebuild and quick action is one line. Peek output and message text are never logged. "Open logs" in the tray opens the directory.

## 16. Testing

Test-first for every unit below.

`herdr-client`

- Framing: encode requests, decode success, error and event lines, tolerate unknown fields and unknown `type` values, reject id mismatches.
- One-shot request against `FakeHerdrServer`: success, error response, server closing early, malformed line.
- Subscription: ack, pushed events, `events_lost`, EOF.
- `BridgeTransport` with a fake child (a small test binary or `sh -c` script): marker skipping, banner noise before the marker, marker timeout, child exit before marker, stderr capture.
- Named pipe transport: same suite on Windows CI with the fake server on a pipe.

`herdr-status-core`

- Launchers: the POSIX and Windows remote command strings equal herdr's byte for byte (fixtures copied from herdr's tests), `wsl.exe` argument list, SSH option list per client OS, PowerShell `-EncodedCommand` encoding round-trip.
- Platform detection parser: `uname` outputs for Linux and Darwin, failure exit codes, the Windows probe line.
- Discovery script output parsing.
- Reducer sequences: first snapshot sets no `since`; status event sets `since` and bumps `flash_seq`; snapshot with same status keeps them; pane closed removes the row; server offline moves rows to the offline group and back; sort order across mixed statuses and servers; counts.
- Watcher against `FakeHerdrServer`: pane created triggers a rebuild with the new pane in the subscription; `events_lost` triggers a rebuild; reconcile timer; backoff timings with a mocked clock.
- Config: parse, defaults, duplicate labels, unknown keys, prefill from a canned `herdr machine list --json`.

`ui`

- `tsc --noEmit` and vitest with jsdom: render a given `AgentList` and assert row order, texts, chip states, offline groups; drawer open/close/exclusive rules; keypad allow-list; duration formatting table.

Integration and manual

- Opt-in Rust test (`HERDR_STATUS_LIVE=1`) that pings the real local herdr, takes a snapshot, and holds a subscription for 3 s.
- `cargo tauri dev` against the live local herdr on this Mac, checking every row behaviour and action by hand.
- Windows smoke checklist run by the user on a Windows machine (section 17).

## 17. Build, CI, distribution

Toolchain: Rust stable (1.99 on this Mac), Tauri 2.12.x and `tauri-cli` 2.x, bun 1.4 for the UI (Node 18 is too old for current Vite). Xcode command line tools on macOS; on Windows the MSVC build tools and WebView2 runtime (present on Windows 10 and 11).

Commands

```
bun install                  # ui dependencies
cargo test --workspace       # core and client tests
bun run test                 # ui tests (vitest) and tsc --noEmit
cargo tauri dev              # run against the live herdr
cargo tauri build            # .app + .dmg on macOS, .msi + NSIS .exe on Windows
```

CI: `.github/workflows/build.yml` with a matrix of `macos-latest` and `windows-latest`: install Rust and bun, run all tests, `cargo tauri build`, upload the bundles as artifacts. The Windows job is the only way to produce and sanity-check the Windows build from this project until the user runs it.

macOS: ad-hoc signed `.app`; first launch needs right-click Open. Windows: unsigned installer; SmartScreen will ask once.

Windows smoke checklist (run once on a Windows machine, results pasted back):

1. Install, launch. Tray icon appears; window opens dark with no white flash.
2. With native herdr running: `local` chip green within 5 s, rows present. If the chip stays grey, check the log for the pipe fallback line.
3. With herdr in WSL: a `[[wsl]]` entry was prefilled, chip green, rows present.
4. Open a row: Peek shows output, a key reaches the agent, a message reaches the agent.
5. Close the window: it hides; left-click tray shows it; Quit exits.
6. Toggle Launch at login, sign out and in, the app is in the tray.
7. Edit config, save, chip set updates without a restart.

## 18. Project layout

```
herdr-status/
├── Cargo.toml                     # workspace: crates/*, src-tauri
├── crates/
│   ├── herdr-client/              # protocol, transports, FakeHerdrServer (feature test-support)
│   └── herdr-status-core/         # config, launchers, watcher, reducer, AgentList
├── src-tauri/                     # Tauri app: main.rs, tray.rs, commands.rs, tauri.conf.json, icons/
├── ui/                            # index.html, src/{main,render,drawer,format}.ts, styles.css, vite.config.ts, package.json
├── scripts/gen-tray-icons.ts
├── .github/workflows/build.yml
├── docs/superpowers/specs/        # this document
├── docs/superpowers/plans/        # implementation plan
├── README.md
└── .gitignore                     # target/, node_modules/, ui/dist/, .superpowers/
```

Product name `herdr-status`, bundle identifier `com.maximeverreault.herdrstatus` (changeable in `tauri.conf.json`).

## 19. Out of scope for this version

- Remote machines where herdr runs inside WSL (would need an SSH-then-`wsl.exe` launcher; the design leaves room for it as a third launcher variant).
- Any form of "jump to pane" or focusing in herdr.
- Watching more than one local session; one `[local]` entry only (a second local session can be reached as a `[[remote]]` pointing at `localhost` if ever needed).
- Notifications, sounds, light theme, settings UI, Linux packaging, auto-update, code signing and notarization.
- Showing panes that have no detected agent.

## 20. Risks and how they are retired

| Risk | Mitigation |
|---|---|
| Windows named pipe name derived incorrectly | Runtime fallback to the local process bridge; smoke test item 2 |
| `wsl.exe` stdio passthrough mangles the stream | Marker-based framing tolerates noise; smoke test item 3; no further fallback |
| PowerShell wrapper for Windows remotes mis-quoted | Copied from herdr and byte-compared in tests |
| herdr protocol changes after 0.9.3 | Unknown fields ignored, unknown methods surface as errors, protocol number logged |
| `ControlPath` socket path too long (104-byte limit) | Short cache directory plus `%C` hash |
| Elapsed times reset when the bar restarts | Accepted; shown as the state word without a duration |
