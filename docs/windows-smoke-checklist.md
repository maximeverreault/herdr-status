# Windows smoke checklist

Run once on a Windows machine with the installer from CI. Paste results (pass/fail + notes) back
into the PR or issue.

| # | Step | Expected | Result |
|---|------|----------|--------|
| 1 | Install from the `.msi` (or `.exe`), launch from the Start menu | Tray icon appears; window opens dark with no white flash (first launch may pause up to ~10 s while probing) | |
| 2 | With native herdr running (`herdr status` in a terminal shows a running server) | `local` chip green within 5 s; one row per agent pane | |
| 2b | If the chip stays grey: open logs (tray, Open logs; or `%LOCALAPPDATA%\herdr-status\logs`) | Look for `primary transport unavailable; using fallback`; if present the named-pipe name is wrong but the fallback works: report the line and the `socket:` path from `herdr status` | |
| 3 | With herdr running inside WSL and no `[[wsl]]` entry yet: delete `%APPDATA%\herdr-status\config.toml`, relaunch | `[[wsl]]` entry written; `wsl` chip green; its rows present | |
| 3b | WSL probe: the `test -S "..."` check and the multi-line bridge script (with quotes) pass intact through `wsl.exe --exec sh -lc` | `wsl` chip is green, rows listed (no quoting errors in the log) | |
| 3c | Make the named pipe unavailable (e.g. start the bar with a bogus pipe, or while only `herdr.exe` is on PATH) | Log shows `primary transport unavailable; using fallback`; the local `herdr.exe` fallback bridge connects and the chip goes green | |
| 4 | Click a row: Peek | Terminal output appears and refreshes | |
| 5 | Keys tab: press a harmless key such as `↓` in an idle agent | The agent's terminal reacts | |
| 6 | Message tab: send `say hello` | The agent receives it | |
| 7 | Close the window (x) | Window hides, process keeps running, tray click shows it | |
| 7b | Window shown and focused: left-click the tray icon | Window hides (left click toggles by visibility) | |
| 7c | Launch the app a second time | The existing window is focused; no second tray icon | |
| 8 | Tray, Launch at login, sign out and in | App is in the tray after sign-in, window hidden | |
| 8b | `reg query HKCU\Software\Microsoft\Windows\CurrentVersion\Run` | The herdr-status value contains `--hidden` | |
| 9 | Edit config.toml (change `reconcile_seconds`) and save from Notepad, then from VS Code | Log shows exactly one `config reloaded` per save; no restart needed | |
| 10 | Make an agent block (permission prompt); set display scaling to 100 %, 150 %, 200 % | Tray badge digit is readable at each scale | |
| 11 | Add a `[[remote]]` pointing at the Mac (herdr running there) | Chip green, Mac agents listed; actions work | |
| 12 | Quit from the tray, then check Task Manager / `tasklist` | Process exits; no leftover `ssh.exe` or `wsl.exe` children | |

Windows version: ______  herdr version: ______  WSL distro: ______
