# macOS manual checklist

Run with `cd ui && bun tauri dev` (or the bundled `.app`) and herdr running. Fix and commit anything that fails.

## Core flow

- [ ] Launch with herdr running: `local` chip green within 1 s; one row per agent pane; names match herdr's sidebar titles.
- [ ] In an agent pane, make the agent go `working` then `idle`: the row flashes and its state text changes within 1 s; the right-hand time counts from that moment.
- [ ] Trigger a permission prompt (Claude Code: ask it to run a shell command without auto-approval): row turns amber, moves to the top, tray icon turns amber and the menu bar shows `1`.
- [ ] Open that row: Peek shows the prompt text; the keypad is visible; press `1` or `Enter`: the agent continues and the row leaves the blocked state.
- [ ] Message tab: type `summarize what you did in one line`, press Enter; the agent receives it.
- [ ] (Optional, stops your session) `herdr server stop`: chip grey, rows vanish, "herdr is not running"; restart herdr: chip green, rows back within 5 s.
- [ ] Add a bogus `[[remote]]` with `ssh_target = "nohost.invalid"`: within a second a `nohost...` chip appears grey with an offline group reading `Unreachable · retrying ...`; remove it: the chip disappears.
- [ ] Real POSIX remote: chip green, peek/keys/message work, `ls /tmp/herdr-status-<user>/` shows a control socket, and the master survives app quit for up to 10 minutes.
- [ ] Windows SSH host remote: connects through the PowerShell bridge; `os` and `herdr_path` detection is cached in `cache.json`.
- [ ] Resize the window narrow (260 px): names ellipsise, nothing wraps.
- [ ] Close the window, click the tray: it comes back where it was.

## Tray, window and lifecycle

- [ ] Quit via the tray with the window open, relaunch with `--hidden`: no flash, no activation.
- [ ] Start while an agent is already blocked: amber icon and count title appear immediately.
- [ ] Dock click shows the hidden window.
- [ ] Cmd+Q and tray Quit both log "hub shut down" within 2 s.
- [ ] The red close button only hides the window.
- [ ] Save config with vim (`:w`) and with VS Code: exactly one "config reloaded" per save.
- [ ] Window position and size persist across restarts.
- [ ] Launch at login (bundled build only): `~/Library/LaunchAgents/*.plist` contains `--hidden`; then turn it off again.

## Drawer and UI

- [ ] Type slowly in the Message tab while another agent is active: input keeps focus and text; the peek scroll position does not jump on events.
- [ ] A flash plays once (~1 s) during a burst of events.
- [ ] With Reduce Motion on, the flash is a ~1 s static highlight that clears.
- [ ] Type a draft, then make that agent change status: the draft survives.
- [ ] Switch quickly between two rows: peek never shows the other pane.
- [ ] Hover a chip during activity: the tooltip stays steady.
- [ ] Double-click Enter on the keypad: only one key reaches the agent (buttons disabled in flight).
- [ ] herdr stopped, only local configured: "herdr is not running". With a remote also configured: "No servers reachable" / "Connecting...".
- [ ] Break `config.toml`, then fix it: the banner appears, then clears and stays cleared.
- [ ] Escape with the message input focused closes the drawer.
