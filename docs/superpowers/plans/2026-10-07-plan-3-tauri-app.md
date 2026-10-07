# herdr-status Tauri App, UI and CI Implementation Plan (Plan 3 of 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The shippable app: a Tauri 2 shell around `herdr-status-core` with the compact-rows UI, action drawer, tray icon, window behaviour, autostart, live config reload, first-launch prefill, a GitHub Actions build for both OSes, and the Windows smoke checklist.

**Architecture:** `src-tauri` is thin glue: it builds a `Hub`, forwards every `AgentList` to the webview as the `agents` event, exposes the quick actions as commands, owns the tray and window, watches `config.toml`. `ui/` is plain TypeScript rendering the list it receives; it never parses herdr JSON. Tray icons are rendered at runtime from an RGBA buffer. Logging from the core's `tracing` flows into `tauri-plugin-log` through tracing's `log` feature.

**Tech Stack:** Tauri 2.12 (`tray-icon`, `image-png` features), plugins `log`, `opener`, `autostart`, `window-state`, `single-instance`; `notify` for the config watcher. UI: Vite 6, TypeScript 5, vitest 3 + jsdom, `@tauri-apps/api` 2. Package manager: bun 1.4. CI: GitHub Actions, `macos-latest` + `windows-latest`.

**Spec:** `docs/superpowers/specs/2026-10-07-herdr-status-bar-design.md`, sections 10 to 17. Plans 1 and 2 must be complete.

**Conventions:** commands run from the repo root unless a `cd` is shown. Rust unit tests live next to the code; UI tests in `ui/src/__tests__/`. Commit after each task. On this Mac the Windows side is only type-checked by CI; the smoke checklist (Task 14) covers runtime verification there.

---

## File structure

| File | Responsibility |
|---|---|
| `Cargo.toml` | Add `src-tauri` to workspace members; `tracing` gets the `log` feature |
| `src-tauri/Cargo.toml`, `build.rs`, `tauri.conf.json`, `capabilities/default.json`, `icons/*` | Tauri crate, config, permissions, app icon |
| `src-tauri/src/main.rs` | Binary entry, calls `herdr_status_app::run()` |
| `src-tauri/src/lib.rs` | `run()`: plugin registration, setup, command handler |
| `src-tauri/src/state.rs` | `AppState`: hub, config path, platform, env, current config, latest list |
| `src-tauri/src/commands.rs` | `get_agents`, `peek`, `send_key`, `send_message`, `reconnect_all`, `open_config`, `open_logs` |
| `src-tauri/src/tray_icon.rs` | Pure RGBA renderer: disc + badge with 3×5 digits (unit-tested) |
| `src-tauri/src/tray.rs` | Tray icon + menu, `update_tray(list)` |
| `src-tauri/src/window.rs` | show/hide/toggle, close-to-hide, `--hidden` |
| `src-tauri/src/config_watch.rs` | Load or create config, watch file, apply to hub, `config_error` event |
| `src-tauri/src/startup.rs` | First launch: prefill, hub creation, list forwarding loop |
| `ui/package.json`, `vite.config.ts`, `tsconfig.json`, `vitest.config.ts`, `index.html` | Frontend tooling |
| `ui/src/types.ts` | TypeScript mirror of `AgentList` |
| `ui/src/format.ts` | Duration and state text |
| `ui/src/render.ts` | Header chips, rows, offline groups, empty states, FLIP reorder, flash |
| `ui/src/drawer.ts` | Drawer state machine: tabs, peek polling, keypad, message |
| `ui/src/tauri.ts` | Thin wrappers around `invoke`/`listen` so tests can stub them |
| `ui/src/main.ts` | Wires events → render → drawer |
| `ui/src/styles.css` | Spec section 11 tokens and layout |
| `ui/scripts/make-app-icon.ts` | Generates `src-tauri/icons/source.png` (pngjs) for `tauri icon` |
| `.github/workflows/build.yml` | Test + build matrix, artifacts |
| `README.md`, `docs/windows-smoke-checklist.md` | Usage and the manual Windows check |

---

### Task 1: Tauri and UI scaffolding

**Files:**
- Modify: `Cargo.toml`
- Create: `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`
- Create: `ui/package.json`, `ui/vite.config.ts`, `ui/tsconfig.json`, `ui/index.html`, `ui/src/main.ts`, `ui/src/styles.css`, `ui/scripts/make-app-icon.ts`
- Modify: `.gitignore`

- [ ] **Step 1: Workspace changes**

In the root `Cargo.toml`, set members and give `tracing` the `log` feature so core logs reach the Tauri log plugin:

```toml
[workspace]
resolver = "2"
members = ["crates/herdr-client", "crates/herdr-status-core", "src-tauri"]
```

and in `[workspace.dependencies]` replace the tracing line with:

```toml
tracing = { version = "0.1", features = ["log"] }
```

Append to `.gitignore`:

```
src-tauri/target/
src-tauri/gen/schemas/
ui/node_modules/
```

- [ ] **Step 2: UI tooling**

`ui/package.json`:

```json
{
  "name": "herdr-status-ui",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc --noEmit && vite build",
    "test": "tsc --noEmit && vitest run",
    "tauri": "tauri",
    "icon": "bun run scripts/make-app-icon.ts && tauri icon ../src-tauri/icons/source.png -o ../src-tauri/icons"
  },
  "dependencies": {
    "@tauri-apps/api": "^2.6.0"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2.8.0",
    "@types/pngjs": "^6.0.5",
    "jsdom": "^26.1.0",
    "pngjs": "^7.0.0",
    "typescript": "^5.8.0",
    "vite": "^6.3.0",
    "vitest": "^3.2.0"
  }
}
```

`ui/vite.config.ts`:

```ts
import { defineConfig } from "vite";

// Tauri expects a fixed port and no screen clearing while it prints its own logs.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: { target: ["es2022", "safari15"], minify: true, sourcemap: false },
});
```

`ui/vitest.config.ts`:

```ts
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: { environment: "jsdom", include: ["src/__tests__/**/*.test.ts"] },
});
```

`ui/tsconfig.json`:

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "noEmit": true,
    "skipLibCheck": true,
    "types": ["vite/client"]
  },
  "include": ["src", "scripts", "vite.config.ts", "vitest.config.ts"]
}
```

`ui/index.html`:

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <meta name="color-scheme" content="dark" />
    <title>herdr-status</title>
    <link rel="stylesheet" href="/src/styles.css" />
  </head>
  <body>
    <div id="app">
      <header id="header"></header>
      <main id="list"></main>
      <div id="banner" hidden></div>
    </div>
    <script type="module" src="/src/main.ts"></script>
  </body>
</html>
```

`ui/src/styles.css` (just enough to confirm the dark window; the full stylesheet lands in Task 8):

```css
:root { color-scheme: dark; --bg: #0f1115; --text: #e6e8ee; }
html, body { margin: 0; background: var(--bg); color: var(--text); font-family: system-ui, -apple-system, "Segoe UI", sans-serif; }
```

`ui/src/main.ts` (placeholder until Task 9):

```ts
document.querySelector<HTMLElement>("#list")!.textContent = "herdr-status";
```

`ui/scripts/make-app-icon.ts` (1024×1024 dark rounded square with an amber disc; `tauri icon` derives every platform size from it):

```ts
import { PNG } from "pngjs";
import { writeFileSync, mkdirSync } from "node:fs";

const size = 1024;
const png = new PNG({ width: size, height: size });
const bg = [0x0f, 0x11, 0x15];
const disc = [0xff, 0x9f, 0x0a];
const radius = size * 0.22;
const corner = size * 0.2;

function insideRoundedSquare(x: number, y: number): boolean {
  const dx = Math.max(corner - x, 0, x - (size - 1 - corner));
  const dy = Math.max(corner - y, 0, y - (size - 1 - corner));
  return dx * dx + dy * dy <= corner * corner;
}

for (let y = 0; y < size; y++) {
  for (let x = 0; x < size; x++) {
    const i = (y * size + x) * 4;
    if (!insideRoundedSquare(x, y)) {
      png.data[i + 3] = 0;
      continue;
    }
    const d = Math.hypot(x - size / 2, y - size / 2);
    const inDisc = d <= radius;
    const [r, g, b] = inDisc ? disc : bg;
    png.data[i] = r;
    png.data[i + 1] = g;
    png.data[i + 2] = b;
    png.data[i + 3] = 255;
  }
}
mkdirSync("../src-tauri/icons", { recursive: true });
writeFileSync("../src-tauri/icons/source.png", PNG.sync.write(png));
console.log("wrote ../src-tauri/icons/source.png");
```

Run:

```bash
cd ui && bun install && bun run icon
```

Expected: `src-tauri/icons/` now contains `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.icns`, `icon.ico` and more.

- [ ] **Step 3: Tauri crate**

`src-tauri/Cargo.toml`:

```toml
[package]
name = "herdr-status-app"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "Native status bar for AI agents running in herdr"

[lib]
name = "herdr_status_app"
path = "src/lib.rs"

[[bin]]
name = "herdr-status"
path = "src/main.rs"

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
herdr-client = { path = "../crates/herdr-client" }
herdr-status-core = { path = "../crates/herdr-status-core" }
tauri = { version = "2.12", features = ["tray-icon", "image-png"] }
tauri-plugin-log = "2"
tauri-plugin-opener = "2"
tauri-plugin-autostart = "2"
tauri-plugin-window-state = "2"
tauri-plugin-single-instance = "2"
log = "0.4"
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
notify = "7"
```

`src-tauri/build.rs`:

```rust
fn main() {
    tauri_build::build()
}
```

`src-tauri/tauri.conf.json`:

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "herdr-status",
  "version": "0.1.0",
  "identifier": "com.maximeverreault.herdrstatus",
  "build": {
    "frontendDist": "../ui/dist",
    "devUrl": "http://localhost:1420",
    "beforeDevCommand": { "cwd": "../ui", "script": "bun run dev", "wait": false },
    "beforeBuildCommand": { "cwd": "../ui", "script": "bun run build" }
  },
  "app": {
    "withGlobalTauri": false,
    "windows": [
      {
        "label": "main",
        "title": "herdr-status",
        "width": 300,
        "height": 820,
        "minWidth": 260,
        "minHeight": 300,
        "resizable": true,
        "visible": false,
        "theme": "Dark",
        "backgroundColor": "#0f1115"
      }
    ],
    "security": { "csp": null }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": ["icons/32x32.png", "icons/128x128.png", "icons/128x128@2x.png", "icons/icon.icns", "icons/icon.ico"],
    "macOS": { "minimumSystemVersion": "12.0" }
  }
}
```

If the schema rejects `backgroundColor` (it was added in a 2.x minor), delete that line; the CSS background covers it.

`src-tauri/capabilities/default.json`:

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "herdr-status main window: own commands and events only",
  "windows": ["main"],
  "permissions": ["core:default"]
}
```

`src-tauri/src/main.rs`:

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    herdr_status_app::run()
}
```

`src-tauri/src/lib.rs` (minimal; grows in later tasks):

```rust
//! herdr-status desktop app: Tauri glue around `herdr_status_core`.

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().level(log::LevelFilter::Info).build())
        .setup(|app| {
            let window = app.get_webview_window("main").expect("main window exists");
            if std::env::args().any(|a| a == "--hidden") {
                let _ = window.hide();
            } else {
                let _ = window.show();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running herdr-status");
}
```

- [ ] **Step 4: Verify dev build runs**

Run: `cargo build -p herdr-status-app`
Expected: `Finished` (first build compiles Tauri; several minutes).

Run: `cd ui && bun tauri dev`
Expected: a 300×820 dark window titled `herdr-status` showing the text "herdr-status". Quit with Ctrl+C in the terminal.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml .gitignore src-tauri ui
git commit -m "chore: scaffold tauri app and vite ui"
```

---

### Task 2: Tray icon renderer (`tray_icon.rs`)

**Files:**
- Create: `src-tauri/src/tray_icon.rs`
- Modify: `src-tauri/src/lib.rs` (`mod tray_icon;`)

Pure pixels, no Tauri types, so it is unit-tested like any Rust module.

- [ ] **Step 1: Write the failing tests**

`src-tauri/src/tray_icon.rs` (tests first):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn px(buf: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn count_white(buf: &[u8], size: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> usize {
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = px(buf, size, x, y);
                if p[0] > 240 && p[1] > 240 && p[2] > 240 && p[3] > 200 {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn buffer_is_rgba_of_the_requested_size() {
        let buf = render(32, AMBER, None);
        assert_eq!(buf.len(), 32 * 32 * 4);
    }

    #[test]
    fn disc_is_coloured_in_the_middle_and_transparent_in_the_corners() {
        let buf = render(32, BLUE, None);
        assert_eq!(px(&buf, 32, 16, 16), [0x3b, 0x9d, 0xff, 255]);
        assert_eq!(px(&buf, 32, 0, 0)[3], 0);
        assert_eq!(px(&buf, 32, 31, 31)[3], 0);
    }

    #[test]
    fn badge_draws_white_digits_in_the_top_right_quadrant() {
        let plain = render(32, AMBER, None);
        let one = render(32, AMBER, Some("1"));
        assert_eq!(count_white(&plain, 32, 16, 0, 32, 16), 0);
        assert!(count_white(&one, 32, 16, 0, 32, 16) > 4);
        let nine_plus = render(32, AMBER, Some("9+"));
        assert!(count_white(&nine_plus, 32, 16, 0, 32, 16) > count_white(&one, 32, 16, 0, 32, 16));
    }

    #[test]
    fn badge_text_for_counts() {
        assert_eq!(badge_text(0), None);
        assert_eq!(badge_text(1).as_deref(), Some("1"));
        assert_eq!(badge_text(9).as_deref(), Some("9"));
        assert_eq!(badge_text(10).as_deref(), Some("9+"));
    }

    #[test]
    fn colour_follows_priority() {
        use herdr_status_core::Counts;
        let c = |blocked, working, done| Counts { blocked, working, done, idle: 0, unknown: 0 };
        assert_eq!(colour_for(&c(1, 5, 5)), AMBER);
        assert_eq!(colour_for(&c(0, 1, 5)), BLUE);
        assert_eq!(colour_for(&c(0, 0, 1)), GREEN);
        assert_eq!(colour_for(&c(0, 0, 0)), GREY);
    }
}
```

Add to `src-tauri/src/lib.rs` near the top:

```rust
pub mod tray_icon;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-status-app tray_icon`
Expected: compile error.

- [ ] **Step 3: Implement the renderer**

Above the tests in `tray_icon.rs`:

```rust
//! Tray icon pixels: a coloured disc, optionally with a dark badge carrying a
//! count in a 3×5 bitmap font. Pure function of (size, colour, badge text).

use herdr_status_core::Counts;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

pub const AMBER: Rgb = Rgb(0xff, 0x9f, 0x0a);
pub const BLUE: Rgb = Rgb(0x3b, 0x9d, 0xff);
pub const GREEN: Rgb = Rgb(0x34, 0xc7, 0x59);
pub const GREY: Rgb = Rgb(0x5c, 0x62, 0x70);
const BADGE_BG: Rgb = Rgb(0x1b, 0x1f, 0x27);
const WHITE: Rgb = Rgb(0xff, 0xff, 0xff);

/// Amber if anything needs the user, else blue while working, else green when
/// something finished, else grey.
pub fn colour_for(counts: &Counts) -> Rgb {
    if counts.blocked > 0 {
        AMBER
    } else if counts.working > 0 {
        BLUE
    } else if counts.done > 0 {
        GREEN
    } else {
        GREY
    }
}

pub fn badge_text(blocked: usize) -> Option<String> {
    match blocked {
        0 => None,
        1..=9 => Some(blocked.to_string()),
        _ => Some("9+".to_string()),
    }
}

/// RGBA buffer of `size × size` pixels.
pub fn render(size: u32, colour: Rgb, badge: Option<&str>) -> Vec<u8> {
    let mut buf = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    fill_disc(&mut buf, size, s * 0.5, s * 0.5, s * 0.42, colour);
    if let Some(text) = badge {
        let cx = s * 0.70;
        let cy = s * 0.30;
        fill_disc(&mut buf, size, cx, cy, s * 0.27, BADGE_BG);
        let scale = (size / 16).max(1);
        let glyph_w = 3 * scale;
        let glyph_h = 5 * scale;
        let gap = scale;
        let total_w = text.len() as u32 * glyph_w + (text.len().saturating_sub(1) as u32) * gap;
        let mut x = (cx - total_w as f32 / 2.0).round() as i32;
        let y = (cy - glyph_h as f32 / 2.0).round() as i32;
        for ch in text.chars() {
            draw_glyph(&mut buf, size, x, y, scale, ch);
            x += (glyph_w + gap) as i32;
        }
    }
    buf
}

/// Anti-aliased filled circle.
fn fill_disc(buf: &mut [u8], size: u32, cx: f32, cy: f32, r: f32, colour: Rgb) {
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let coverage = (r - d + 0.5).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend(buf, size, x, y, colour, coverage);
            }
        }
    }
}

fn blend(buf: &mut [u8], size: u32, x: u32, y: u32, colour: Rgb, alpha: f32) {
    let i = ((y * size + x) * 4) as usize;
    let a = alpha;
    let dst_a = buf[i + 3] as f32 / 255.0;
    let out_a = a + dst_a * (1.0 - a);
    for (k, c) in [colour.0, colour.1, colour.2].iter().enumerate() {
        let src = *c as f32;
        let dst = buf[i + k] as f32;
        let out = if out_a > 0.0 { (src * a + dst * dst_a * (1.0 - a)) / out_a } else { 0.0 };
        buf[i + k] = out.round() as u8;
    }
    buf[i + 3] = (out_a * 255.0).round() as u8;
}

/// 3×5 glyphs, one bit per pixel, rows top to bottom.
fn glyph(ch: char) -> [u8; 5] {
    match ch {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b001, 0b001],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        _ => [0; 5],
    }
}

fn draw_glyph(buf: &mut [u8], size: u32, x0: i32, y0: i32, scale: u32, ch: char) {
    let rows = glyph(ch);
    for (row, bits) in rows.iter().enumerate() {
        for col in 0..3 {
            if bits & (0b100 >> col) == 0 {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let x = x0 + (col * scale + dx) as i32;
                    let y = y0 + (row as u32 * scale + dy) as i32;
                    if x >= 0 && y >= 0 && (x as u32) < size && (y as u32) < size {
                        blend(buf, size, x as u32, y as u32, WHITE, 1.0);
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-status-app tray_icon`
Expected: `5 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/tray_icon.rs src-tauri/src/lib.rs
git commit -m "feat(app): runtime tray icon renderer with count badge"
```

---

### Task 3: App state, startup wiring and commands

**Files:**
- Create: `src-tauri/src/state.rs`, `src-tauri/src/startup.rs`, `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`

The hub runs on Tauri's async runtime. Startup blocks `setup` only for the config read (and the one-time prefill probes on first launch); the window is not visible yet, so that is fine.

- [ ] **Step 1: Write the failing test (startup config logic)**

`src-tauri/src/startup.rs` (tests first; the rest in step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use herdr_client::transport::bridge::ChildCommand;
    use herdr_status_core::remote_connect::RunOutput;
    use std::time::Duration;

    struct NoHerdr;
    #[async_trait::async_trait]
    impl ProcessRunner for NoHerdr {
        async fn run(&self, _c: &ChildCommand, _t: Duration) -> std::io::Result<RunOutput> {
            Ok(RunOutput { exit_code: Some(127), stdout: String::new(), stderr: "not found".into() })
        }
    }

    #[tokio::test]
    async fn missing_config_is_created_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        let (cfg, error) = load_or_create(&path, Platform::Mac, &NoHerdr, &Env::default()).await;
        assert_eq!(cfg, Config::default());
        assert!(error.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), Config::DEFAULT_TOML);
    }

    #[tokio::test]
    async fn invalid_config_falls_back_to_defaults_with_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\npeek_lines = \"many\"\n").unwrap();
        let (cfg, error) = load_or_create(&path, Platform::Mac, &NoHerdr, &Env::default()).await;
        assert_eq!(cfg, Config::default());
        assert!(error.unwrap().contains("line 2"));
    }

    #[tokio::test]
    async fn valid_config_is_loaded_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\npeek_lines = 12\n").unwrap();
        let (cfg, error) = load_or_create(&path, Platform::Mac, &NoHerdr, &Env::default()).await;
        assert_eq!(cfg.ui.peek_lines, 12);
        assert!(error.is_none());
    }
}
```

Add to `src-tauri/Cargo.toml` under `[dev-dependencies]`:

```toml
[dev-dependencies]
tempfile = "3"
async-trait.workspace = true
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p herdr-status-app startup`
Expected: compile error.

- [ ] **Step 3: Implement state, startup and commands**

`src-tauri/src/state.rs`:

```rust
//! Everything commands and background tasks share.

use std::path::PathBuf;
use std::sync::Mutex;

use herdr_status_core::{AgentList, Config, Env, Hub, Platform};

pub struct AppState {
    pub platform: Platform,
    pub env: Env,
    pub config_path: PathBuf,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub hub: tokio::sync::Mutex<Hub>,
    pub config: Mutex<Config>,
    pub latest: Mutex<AgentList>,
    /// Keeps the config file watcher alive for the life of the app.
    pub config_watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

impl AppState {
    pub fn peek_lines(&self) -> u32 {
        self.config.lock().unwrap().ui.peek_lines
    }

    /// Chip label for a server id (falls back to the id).
    pub fn label_for(&self, server_id: &str) -> String {
        self.latest
            .lock()
            .unwrap()
            .servers
            .iter()
            .find(|s| s.id == server_id)
            .map(|s| s.label.clone())
            .unwrap_or_else(|| server_id.to_string())
    }
}
```

`src-tauri/src/startup.rs` (above the tests):

```rust
//! Boot: paths, config (with first-launch prefill), hub, and the loop that
//! forwards every `AgentList` to the webview and the tray.

use std::path::Path;
use std::sync::{Arc, Mutex};

use herdr_status_core::prefill::{gather_inputs, prefill_config, render_prefilled};
use herdr_status_core::{default_source_factory, Config, Env, Hub, Platform, ProcessRunner, TokioRunner, WatcherConfig};
use herdr_status_core::paths;
use tauri::{AppHandle, Emitter};

use crate::state::AppState;
use crate::tray;

/// Read `config.toml`, creating it on first launch. Returns the config and an
/// optional banner message when the file is invalid (defaults are used then).
pub async fn load_or_create(path: &Path, platform: Platform, runner: &dyn ProcessRunner, env: &Env) -> (Config, Option<String>) {
    if !path.exists() {
        let inputs = gather_inputs(runner, platform, env).await;
        let cfg = prefill_config(platform, &inputs);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(path, render_prefilled(&cfg)) {
            log::warn!("could not write initial config {}: {e}", path.display());
        }
        return (cfg, None);
    }
    match std::fs::read_to_string(path) {
        Ok(text) => match Config::parse(&text) {
            Ok(cfg) => (cfg, None),
            Err(e) => (Config::default(), Some(format!("{}: {e}", path.display()))),
        },
        Err(e) => (Config::default(), Some(format!("{}: {e}", path.display()))),
    }
}

pub async fn boot(app: &AppHandle) -> Result<(Arc<AppState>, Option<String>), String> {
    let platform = Platform::current();
    let env = Env::from_process();
    let config_path = paths::app_config_file(&env, platform);
    let data_dir = paths::app_data_dir(&env, platform);
    let log_dir = paths::app_log_dir(&env, platform);
    let _ = std::fs::create_dir_all(&data_dir);
    let runner: Arc<dyn ProcessRunner> = Arc::new(TokioRunner);

    let (config, config_error) = load_or_create(&config_path, platform, runner.as_ref(), &env).await;
    let factory = default_source_factory(env.clone(), platform, runner, data_dir.clone());
    let mut hub = Hub::new(factory, WatcherConfig::default());
    hub.apply_config(&config, platform).await;
    let mut list_rx = hub.subscribe();

    let state = Arc::new(AppState {
        platform,
        env,
        config_path,
        data_dir,
        log_dir,
        hub: tokio::sync::Mutex::new(hub),
        config: Mutex::new(config),
        latest: Mutex::new(Default::default()),
        config_watcher: Mutex::new(None),
    });

    let forward_app = app.clone();
    let forward_state = state.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let list = list_rx.borrow_and_update().clone();
            *forward_state.latest.lock().unwrap() = list.clone();
            if let Err(e) = forward_app.emit("agents", &list) {
                log::warn!("emit agents failed: {e}");
            }
            tray::update_tray(&forward_app, &list);
            if list_rx.changed().await.is_err() {
                break;
            }
        }
    });

    Ok((state, config_error))
}
```

`src-tauri/src/commands.rs`:

```rust
//! Commands the webview invokes. Thin wrappers over `herdr_status_core`.

use std::sync::Arc;

use herdr_status_core::{actions, ActionError, AgentList};
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::state::AppState;

#[derive(Serialize)]
pub struct PeekResult {
    pub text: String,
    pub truncated: bool,
}

#[tauri::command]
pub fn get_agents(state: State<'_, Arc<AppState>>) -> AgentList {
    state.latest.lock().unwrap().clone()
}

async fn client_for(state: &AppState, server: &str) -> Result<herdr_client::Client, ActionError> {
    let hub = state.hub.lock().await;
    hub.client_for(server).await.ok_or_else(|| ActionError {
        code: "transport".into(),
        message: format!("Could not reach {}.", state.label_for(server)),
    })
}

#[tauri::command]
pub async fn peek(state: State<'_, Arc<AppState>>, server: String, pane: String) -> Result<PeekResult, ActionError> {
    let client = client_for(&state, &server).await?;
    let read = actions::peek(&client, &pane, state.peek_lines(), &state.label_for(&server)).await?;
    Ok(PeekResult { text: read.text, truncated: read.truncated })
}

#[tauri::command]
pub async fn send_key(state: State<'_, Arc<AppState>>, server: String, pane: String, key: String) -> Result<(), ActionError> {
    let client = client_for(&state, &server).await?;
    actions::send_key(&client, &pane, &key, &state.label_for(&server)).await
}

#[tauri::command]
pub async fn send_message(state: State<'_, Arc<AppState>>, server: String, pane: String, text: String) -> Result<(), ActionError> {
    let client = client_for(&state, &server).await?;
    actions::send_message(&client, &pane, &text, &state.label_for(&server)).await
}

#[tauri::command]
pub async fn reconnect_all(state: State<'_, Arc<AppState>>) -> Result<(), ()> {
    state.hub.lock().await.reconnect_all();
    Ok(())
}

#[tauri::command]
pub fn open_config(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    if !state.config_path.exists() {
        if let Some(parent) = state.config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&state.config_path, herdr_status_core::Config::DEFAULT_TOML).map_err(|e| e.to_string())?;
    }
    app.opener().open_path(state.config_path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_logs(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let _ = std::fs::create_dir_all(&state.log_dir);
    app.opener().open_path(state.log_dir.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}
```

Replace `src-tauri/src/lib.rs` with:

```rust
//! herdr-status desktop app: Tauri glue around `herdr_status_core`.

mod commands;
mod startup;
mod state;
pub mod tray_icon;

// `tray` and `window` arrive in Tasks 5 and 6; until then provide no-op stubs.
mod tray {
    pub fn update_tray(_app: &tauri::AppHandle, _list: &herdr_status_core::AgentList) {}
}

use tauri::{Emitter, Manager};

/// `HERDR_STATUS_LOG=debug|trace` raises the log level (spec section 15).
fn log_level() -> log::LevelFilter {
    match std::env::var("HERDR_STATUS_LOG").ok().as_deref() {
        Some("debug") => log::LevelFilter::Debug,
        Some("trace") => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log_level())
                .max_file_size(2_000_000)
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_agents,
            commands::peek,
            commands::send_key,
            commands::send_message,
            commands::reconnect_all,
            commands::open_config,
            commands::open_logs,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let (state, config_error) = tauri::async_runtime::block_on(startup::boot(&handle))?;
            app.manage(state);
            if let Some(message) = config_error {
                let _ = handle.emit("config_error", Some(message));
            }
            let window = app.get_webview_window("main").expect("main window exists");
            if std::env::args().any(|a| a == "--hidden") {
                let _ = window.hide();
            } else {
                let _ = window.show();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running herdr-status");
}
```

`app.manage(state)` stores an `Arc<AppState>`, which is why the commands take `State<'_, Arc<AppState>>`; `client_for(&state, ..)` works through deref coercion.

- [ ] **Step 4: Run the tests and the app**

Run: `cargo test -p herdr-status-app`
Expected: tray_icon 5 + startup 3 pass.

Run: `cd ui && bun tauri dev`, then in another terminal: `cat ~/.config/herdr-status/config.toml`
Expected: the file exists with the default text (or prefilled remotes if you have herdr machines). The log file under `~/Library/Logs/com.maximeverreault.herdrstatus/` shows `server offline` or snapshot lines.

- [ ] **Step 5: Commit**

```bash
git add src-tauri
git commit -m "feat(app): app state, startup wiring, config bootstrap and commands"
```

---

### Task 4: Config file watcher

**Files:**
- Create: `src-tauri/src/config_watch.rs`
- Modify: `src-tauri/src/lib.rs`

Spec section 13: the file is watched with a 500 ms debounce; a valid change is applied server-by-server; an invalid one keeps the previous config and shows a banner (`config_error` event with the message; `null` clears it).

- [ ] **Step 1: Write the failing test**

`src-tauri/src/config_watch.rs` (tests first):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_outcome_distinguishes_valid_and_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\npeek_lines = 7\n").unwrap();
        match read_config(&path) {
            Ok(cfg) => assert_eq!(cfg.ui.peek_lines, 7),
            Err(e) => panic!("unexpected {e}"),
        }
        std::fs::write(&path, "[ui\n").unwrap();
        let err = read_config(&path).unwrap_err();
        assert!(err.contains("config.toml line 1"), "{err}");
        std::fs::remove_file(&path).unwrap();
        assert!(read_config(&path).unwrap_err().contains("No such file") || read_config(&path).is_err());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p herdr-status-app config_watch`
Expected: compile error.

- [ ] **Step 3: Implement the watcher**

`src-tauri/src/config_watch.rs` (above the tests):

```rust
//! Live reload of `config.toml`.

use std::path::Path;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use herdr_status_core::Config;
use notify::{RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

const DEBOUNCE: Duration = Duration::from_millis(500);

pub fn read_config(path: &Path) -> Result<Config, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Config::parse(&text).map_err(|e| e.to_string())
}

/// Watch the config file's directory; on changes to the file, reload after a
/// quiet period and apply or report.
pub fn start(app: &AppHandle) -> Result<(), String> {
    let state: Arc<AppState> = app.state::<Arc<AppState>>().inner().clone();
    let config_path = state.config_path.clone();
    let dir = config_path.parent().ok_or("config path has no parent")?.to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let (tx, rx) = mpsc::channel::<()>();
    let watched = config_path.clone();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        if let Ok(event) = result {
            if event.paths.iter().any(|p| p == &watched) {
                let _ = tx.send(());
            }
        }
    })
    .map_err(|e| e.to_string())?;
    watcher.watch(&dir, RecursiveMode::NonRecursive).map_err(|e| e.to_string())?;
    *state.config_watcher.lock().unwrap() = Some(watcher);

    let app = app.clone();
    std::thread::Builder::new()
        .name("config-watch".into())
        .spawn(move || {
            while rx.recv().is_ok() {
                // Coalesce bursts of events (editors write several times).
                while rx.recv_timeout(DEBOUNCE).is_ok() {}
                let app = app.clone();
                let state = state.clone();
                let path = config_path.clone();
                tauri::async_runtime::spawn(async move {
                    match read_config(&path) {
                        Ok(cfg) => {
                            log::info!("config reloaded from {}", path.display());
                            *state.config.lock().unwrap() = cfg.clone();
                            state.hub.lock().await.apply_config(&cfg, state.platform).await;
                            let _ = app.emit("config_error", Option::<String>::None);
                        }
                        Err(message) => {
                            log::warn!("config invalid, keeping previous: {message}");
                            let _ = app.emit("config_error", Some(message));
                        }
                    }
                });
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}
```

In `src-tauri/src/lib.rs` add `mod config_watch;` and, in `setup` right after `app.manage(state);`:

```rust
            if let Err(e) = config_watch::start(&handle) {
                log::warn!("config watcher not started: {e}");
            }
```

- [ ] **Step 4: Run the test and verify live reload by hand**

Run: `cargo test -p herdr-status-app config_watch`
Expected: `1 passed`.

Run: `cd ui && bun tauri dev`; edit `~/.config/herdr-status/config.toml` to set `reconcile_seconds = 10`, save.
Expected: the log shows `config reloaded`. Break the file (remove a quote), save: the log shows `config invalid, keeping previous`. Fix it again.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/config_watch.rs src-tauri/src/lib.rs
git commit -m "feat(app): live config reload with debounce and error reporting"
```

---

### Task 5: Tray icon, menu and autostart

**Files:**
- Create: `src-tauri/src/tray.rs`
- Modify: `src-tauri/src/lib.rs` (remove the stub `mod tray`, register autostart, build the tray)

- [ ] **Step 1: Write the failing test**

`src-tauri/src/tray.rs` (tests first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use herdr_status_core::Counts;

    fn list(blocked: usize, working: usize, done: usize) -> AgentList {
        AgentList { counts: Counts { blocked, working, done, idle: 0, unknown: 0 }, ..Default::default() }
    }

    #[test]
    fn tooltip_lists_only_non_zero_counts() {
        assert_eq!(tooltip(&list(1, 2, 1)), "herdr-status: 1 needs you · 2 working · 1 done");
        assert_eq!(tooltip(&list(0, 3, 0)), "herdr-status: 3 working");
        assert_eq!(tooltip(&list(0, 0, 0)), "herdr-status: no agents");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p herdr-status-app tray::`
Expected: compile error.

- [ ] **Step 3: Implement the tray**

`src-tauri/src/tray.rs` (above the tests):

```rust
//! Tray / menu bar icon: colour by priority, count badge or title, menu.

use std::sync::Arc;

use herdr_status_core::AgentList;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_autostart::ManagerExt;

use crate::state::AppState;
use crate::tray_icon::{badge_text, colour_for, render, Rgb};
use crate::window;

pub const TRAY_ID: &str = "main-tray";
const ICON_SIZE: u32 = 44;

const MENU_SHOW: &str = "show";
const MENU_RECONNECT: &str = "reconnect";
const MENU_CONFIG: &str = "config";
const MENU_LOGS: &str = "logs";
const MENU_AUTOSTART: &str = "autostart";
const MENU_QUIT: &str = "quit";

/// Kept in app state so the check mark can be refreshed after toggling.
pub struct TrayMenuState {
    autostart_item: CheckMenuItem<Wry>,
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, MENU_SHOW, "Show / Hide", true, None::<&str>)?;
    let reconnect = MenuItem::with_id(app, MENU_RECONNECT, "Reconnect all", true, None::<&str>)?;
    let config = MenuItem::with_id(app, MENU_CONFIG, "Edit config…", true, None::<&str>)?;
    let logs = MenuItem::with_id(app, MENU_LOGS, "Open logs", true, None::<&str>)?;
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(app, MENU_AUTOSTART, "Launch at login", true, autostart_enabled, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &reconnect,
            &PredefinedMenuItem::separator(app)?,
            &config,
            &logs,
            &autostart,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(image(colour_for(&Default::default()), None))
        .tooltip("herdr-status")
        .show_menu_on_left_click(false)
        .menu(&menu)
        .on_menu_event(|app, event| handle_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                window::toggle(tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(TrayMenuState { autostart_item: autostart });
    Ok(())
}

fn handle_menu(app: &AppHandle, id: &str) {
    match id {
        MENU_SHOW => window::toggle(app),
        MENU_RECONNECT => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let state = app.state::<Arc<AppState>>();
                state.hub.lock().await.reconnect_all();
            });
        }
        MENU_CONFIG => {
            if let Err(e) = crate::commands::open_config(app.clone(), app.state::<Arc<AppState>>()) {
                log::warn!("open config failed: {e}");
            }
        }
        MENU_LOGS => {
            if let Err(e) = crate::commands::open_logs(app.clone(), app.state::<Arc<AppState>>()) {
                log::warn!("open logs failed: {e}");
            }
        }
        MENU_AUTOSTART => {
            let manager = app.autolaunch();
            let enabled = manager.is_enabled().unwrap_or(false);
            let result = if enabled { manager.disable() } else { manager.enable() };
            if let Err(e) = result {
                log::warn!("autostart toggle failed: {e}");
            }
            let now_enabled = manager.is_enabled().unwrap_or(!enabled);
            let _ = app.state::<TrayMenuState>().autostart_item.set_checked(now_enabled);
        }
        MENU_QUIT => app.exit(0),
        _ => {}
    }
}

/// Called with every published list.
pub fn update_tray(app: &AppHandle, list: &AgentList) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let colour = colour_for(&list.counts);
    let badge = badge_text(list.counts.blocked);
    if cfg!(target_os = "macos") {
        let _ = tray.set_icon(Some(image(colour, None)));
        let _ = tray.set_title(badge.as_deref());
    } else {
        let _ = tray.set_icon(Some(image(colour, badge.as_deref())));
    }
    let _ = tray.set_tooltip(Some(tooltip(list)));
}

pub fn tooltip(list: &AgentList) -> String {
    let c = &list.counts;
    let mut parts = Vec::new();
    if c.blocked > 0 {
        parts.push(format!("{} needs you", c.blocked));
    }
    if c.working > 0 {
        parts.push(format!("{} working", c.working));
    }
    if c.done > 0 {
        parts.push(format!("{} done", c.done));
    }
    if parts.is_empty() {
        "herdr-status: no agents".to_string()
    } else {
        format!("herdr-status: {}", parts.join(" · "))
    }
}

fn image(colour: Rgb, badge: Option<&str>) -> Image<'static> {
    Image::new_owned(render(ICON_SIZE, colour, badge), ICON_SIZE, ICON_SIZE)
}
```

In `src-tauri/src/lib.rs`: delete the stub `mod tray { … }` block, add `mod tray;` and `mod window;` (the window module arrives in Task 6; create `src-tauri/src/window.rs` now with the two functions below so this task compiles), register the autostart plugin and build the tray in `setup`:

```rust
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
```

and in `setup`, after the config watcher line:

```rust
            tray::build(&handle)?;
```

`src-tauri/src/window.rs` (initial version; Task 6 extends it):

```rust
//! Main window helpers.

use tauri::{AppHandle, Manager};

pub fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Left tray click: hide when visible and focused, otherwise show and focus.
pub fn toggle(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else { return };
    let visible = w.is_visible().unwrap_or(false);
    let focused = w.is_focused().unwrap_or(false);
    if visible && focused {
        let _ = w.hide();
    } else {
        show(app);
    }
}
```

- [ ] **Step 4: Run the test and check the tray by hand**

Run: `cargo test -p herdr-status-app`
Expected: all pass (tray tooltip test included).

Run: `cd ui && bun tauri dev`
Expected: a coloured dot in the macOS menu bar (blue while any agent is working, grey otherwise); left click toggles the window; right click shows the six-item menu; "Launch at login" toggles its check mark; Quit exits.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/tray.rs src-tauri/src/window.rs src-tauri/src/lib.rs
git commit -m "feat(app): tray icon with priority colour, count badge and menu; autostart"
```

---

### Task 6: Window behaviour, single instance, window state

**Files:**
- Modify: `src-tauri/src/window.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Close hides, Dock reopen shows**

Append to `src-tauri/src/window.rs`:

```rust
/// The close button hides the window; the app lives in the tray.
pub fn install_close_to_hide(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else { return };
    let target = w.clone();
    w.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = target.hide();
        }
    });
}
```

- [ ] **Step 2: Register the plugins and the run-event handler**

In `src-tauri/src/lib.rs`, the builder becomes (single-instance first, as its docs require; `run` switches to `build(..).run(..)` so Dock reopen can be handled):

```rust
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| window::show(app)))
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log_level())
                .max_file_size(2_000_000)
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(tauri::generate_handler![
            commands::get_agents,
            commands::peek,
            commands::send_key,
            commands::send_message,
            commands::reconnect_all,
            commands::open_config,
            commands::open_logs,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let (state, config_error) = tauri::async_runtime::block_on(startup::boot(&handle))?;
            app.manage(state);
            if let Err(e) = config_watch::start(&handle) {
                log::warn!("config watcher not started: {e}");
            }
            tray::build(&handle)?;
            window::install_close_to_hide(&handle);
            if let Some(message) = config_error {
                let _ = handle.emit("config_error", Some(message));
            }
            let window = app.get_webview_window("main").expect("main window exists");
            if std::env::args().any(|a| a == "--hidden") {
                let _ = window.hide();
            } else {
                let _ = window.show();
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building herdr-status");

    app.run(|app, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            window::show(app);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (app, event);
    });
}
```

- [ ] **Step 3: Verify by hand**

Run: `cd ui && bun tauri dev`
Expected:
- Close button hides the window; the process keeps running; tray click brings it back.
- Move and resize the window, Quit from the tray, relaunch: same position and size.
- Launch a second copy while one runs (`./src-tauri/target/debug/herdr-status`): the first window is shown and focused, the second exits.
- `./src-tauri/target/debug/herdr-status --hidden`: starts with no window; tray click shows it.
- Click the Dock icon while hidden: window shows.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/window.rs src-tauri/src/lib.rs
git commit -m "feat(app): close-to-hide, single instance, window state, dock reopen"
```

---

### Task 7: UI types and formatting

**Files:**
- Create: `ui/src/types.ts`, `ui/src/format.ts`, `ui/src/__tests__/format.test.ts`

- [ ] **Step 1: Write the failing tests**

`ui/src/__tests__/format.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { formatDuration, retryText, stateText } from "../format";
import type { AgentRow } from "../types";

const row = (status: AgentRow["status"], since: number | null): AgentRow => ({
  server_id: "local",
  server_label: "local",
  pane_id: "w1:p1",
  workspace_label: "ws",
  agent_kind: "claude",
  name: "x",
  status,
  since,
  flash_seq: 0,
});

const NOW = 1_000_000_000_000;
const s = (n: number) => n * 1000;

describe("formatDuration", () => {
  it("buckets by magnitude", () => {
    expect(formatDuration(NOW - s(5), NOW)).toBe("just now");
    expect(formatDuration(NOW - s(59), NOW)).toBe("just now");
    expect(formatDuration(NOW - s(60), NOW)).toBe("1m");
    expect(formatDuration(NOW - s(12 * 60 + 30), NOW)).toBe("12m");
    expect(formatDuration(NOW - s(3 * 3600 + 5 * 60), NOW)).toBe("3h 05m");
    expect(formatDuration(NOW - s(26 * 3600), NOW)).toBe("1d");
    expect(formatDuration(NOW + s(10), NOW)).toBe("just now", "clock skew never goes negative");
  });
});

describe("stateText", () => {
  it("per status, with and without a duration", () => {
    expect(stateText(row("blocked", NOW - s(120)), NOW)).toBe("needs you · 2m");
    expect(stateText(row("blocked", null), NOW)).toBe("needs you");
    expect(stateText(row("working", NOW - s(240)), NOW)).toBe("4m");
    expect(stateText(row("working", null), NOW)).toBe("working");
    expect(stateText(row("done", NOW - s(720)), NOW)).toBe("done 12m");
    expect(stateText(row("done", null), NOW)).toBe("done");
    expect(stateText(row("idle", NOW - s(1500)), NOW)).toBe("idle 25m");
    expect(stateText(row("idle", null), NOW)).toBe("idle");
    expect(stateText(row("unknown", NOW), NOW)).toBe("—");
  });
});

describe("retryText", () => {
  it("counts down to the retry", () => {
    expect(retryText(NOW + s(30), NOW)).toBe("retrying 30s");
    expect(retryText(NOW - s(1), NOW)).toBe("retrying now");
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd ui && bun run test`
Expected: fails, modules `../format` and `../types` not found.

- [ ] **Step 3: Implement types and format**

`ui/src/types.ts` (mirror of the Rust `AgentList` serialisation; `since` and `retry_at` are Unix milliseconds):

```ts
export type AgentStatus = "blocked" | "working" | "done" | "idle" | "unknown";
export type ServerKind = "local" | "wsl" | "remote";

export type ConnectionState =
  | { state: "connecting" }
  | { state: "online" }
  | { state: "offline"; reason: string; retry_at: number };

export interface AgentRow {
  server_id: string;
  server_label: string;
  pane_id: string;
  workspace_label: string;
  agent_kind: string | null;
  name: string;
  status: AgentStatus;
  since: number | null;
  flash_seq: number;
}

export interface ServerChip {
  id: string;
  label: string;
  kind: ServerKind;
  connection: ConnectionState;
  transport: string;
}

export interface OfflineGroup {
  server_id: string;
  label: string;
  reason: string;
  retry_at: number;
  rows: AgentRow[];
}

export interface Counts {
  blocked: number;
  working: number;
  done: number;
  idle: number;
  unknown: number;
}

export interface AgentList {
  servers: ServerChip[];
  rows: AgentRow[];
  offline: OfflineGroup[];
  counts: Counts;
}

export interface PeekResult {
  text: string;
  truncated: boolean;
}

export interface ActionError {
  code: string;
  message: string;
}

export const EMPTY_LIST: AgentList = {
  servers: [],
  rows: [],
  offline: [],
  counts: { blocked: 0, working: 0, done: 0, idle: 0, unknown: 0 },
};

export function rowKey(row: AgentRow): string {
  return `${row.server_id}|${row.pane_id}`;
}
```

`ui/src/format.ts`:

```ts
import type { AgentRow } from "./types";

/** "just now" under a minute, then 12m, 3h 05m, 2d. */
export function formatDuration(sinceMs: number, nowMs: number): string {
  const seconds = Math.max(0, Math.floor((nowMs - sinceMs) / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
  return `${Math.floor(hours / 24)}d`;
}

/** Right-hand text of a row (spec section 11). */
export function stateText(row: AgentRow, nowMs: number): string {
  const dur = row.since === null ? null : formatDuration(row.since, nowMs);
  switch (row.status) {
    case "blocked":
      return dur ? `needs you · ${dur}` : "needs you";
    case "working":
      return dur ?? "working";
    case "done":
      return dur ? `done ${dur}` : "done";
    case "idle":
      return dur ? `idle ${dur}` : "idle";
    default:
      return "—";
  }
}

export function retryText(retryAtMs: number, nowMs: number): string {
  const seconds = Math.ceil((retryAtMs - nowMs) / 1000);
  return seconds > 0 ? `retrying ${seconds}s` : "retrying now";
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd ui && bun run test`
Expected: 3 test groups pass.

- [ ] **Step 5: Commit**

```bash
git add ui/src/types.ts ui/src/format.ts ui/src/__tests__/format.test.ts
git commit -m "feat(ui): agent list types and state text formatting"
```

---

### Task 8: Rendering and stylesheet

**Files:**
- Create: `ui/src/render.ts`, `ui/src/__tests__/render.test.ts`
- Modify: `ui/src/styles.css` (full version)

Rows are keyed by `server_id|pane_id` so elements survive re-renders, which makes the flash and FLIP reorder possible. The drawer element (Task 9) is passed in and placed right after the open row.

- [ ] **Step 1: Write the failing tests**

`ui/src/__tests__/render.test.ts`:

```ts
import { beforeEach, describe, expect, it } from "vitest";
import { renderHeader, renderList } from "../render";
import { EMPTY_LIST, type AgentList, type AgentRow } from "../types";

const row = (over: Partial<AgentRow>): AgentRow => ({
  server_id: "local",
  server_label: "local",
  pane_id: "w1:p1",
  workspace_label: "swat",
  agent_kind: "claude",
  name: "reviewer",
  status: "working",
  since: null,
  flash_seq: 0,
  ...over,
});

const NOW = 1_000_000_000_000;

function list(over: Partial<AgentList>): AgentList {
  return { ...EMPTY_LIST, servers: [{ id: "local", label: "local", kind: "local", connection: { state: "online" }, transport: "unix socket /x" }], ...over };
}

let header: HTMLElement;
let main: HTMLElement;
let drawer: HTMLElement;

beforeEach(() => {
  document.body.innerHTML = `<header id="h"></header><main id="l"></main>`;
  header = document.getElementById("h")!;
  main = document.getElementById("l")!;
  drawer = document.createElement("div");
  drawer.className = "drawer";
});

describe("renderHeader", () => {
  it("shows one chip per server with its connection class and tooltip", () => {
    const l = list({
      servers: [
        { id: "local", label: "local", kind: "local", connection: { state: "online" }, transport: "unix socket /x" },
        { id: "box", label: "box", kind: "remote", connection: { state: "offline", reason: "Unreachable", retry_at: NOW + 30000 }, transport: "ssh box (posix)" },
        { id: "wsl", label: "wsl", kind: "wsl", connection: { state: "connecting" }, transport: "" },
      ],
    });
    renderHeader(header, l, NOW);
    const chips = header.querySelectorAll(".chip");
    expect(chips.length).toBe(3);
    expect(chips[0]!.className).toContain("online");
    expect(chips[0]!.getAttribute("title")).toBe("unix socket /x");
    expect(chips[1]!.className).toContain("offline");
    expect(chips[1]!.getAttribute("title")).toContain("Unreachable");
    expect(chips[1]!.getAttribute("title")).toContain("retrying 30s");
    expect(chips[2]!.className).toContain("connecting");
    expect(header.querySelector(".brand")!.textContent).toBe("herdr");
  });
});

describe("renderList", () => {
  it("renders rows in list order with dot, name, meta and state text", () => {
    const l = list({
      rows: [
        row({ pane_id: "w1:p2", name: "reviewer", status: "blocked", since: NOW - 120000, agent_kind: "codex", server_label: "winbox", server_id: "winbox" }),
        row({ pane_id: "w1:p1", name: "db migration", status: "working", since: NOW - 240000 }),
      ],
    });
    renderList(main, l, NOW, { openKey: null, drawer, onRowClick: () => {} });
    const rows = main.querySelectorAll(".row");
    expect(rows.length).toBe(2);
    expect(rows[0]!.className).toContain("blocked");
    expect(rows[0]!.querySelector(".name")!.textContent).toBe("reviewer");
    expect(rows[0]!.querySelector(".meta")!.textContent).toBe("winbox · swat · codex");
    expect(rows[0]!.querySelector(".state")!.textContent).toBe("needs you · 2m");
    expect(rows[1]!.querySelector(".state")!.textContent).toBe("4m");
    expect(rows[1]!.getAttribute("data-key")).toBe("local|w1:p1");
  });

  it("reuses elements across renders and flashes when flash_seq changes", () => {
    const first = list({ rows: [row({ flash_seq: 0 })] });
    renderList(main, first, NOW, { openKey: null, drawer, onRowClick: () => {} });
    const el = main.querySelector(".row")!;
    expect(el.classList.contains("flash")).toBe(false);
    const second = list({ rows: [row({ flash_seq: 1, status: "blocked" })] });
    renderList(main, second, NOW, { openKey: null, drawer, onRowClick: () => {} });
    expect(main.querySelector(".row")).toBe(el);
    expect(el.classList.contains("flash")).toBe(true);
    expect(el.classList.contains("blocked")).toBe(true);
  });

  it("places the drawer after the open row and removes rows that are gone", () => {
    const l = list({ rows: [row({ pane_id: "a" }), row({ pane_id: "b" })] });
    renderList(main, l, NOW, { openKey: "local|b", drawer, onRowClick: () => {} });
    const rowB = main.querySelector('[data-key="local|b"]')!;
    expect(rowB.nextElementSibling).toBe(drawer);
    expect(rowB.classList.contains("open")).toBe(true);
    renderList(main, list({ rows: [row({ pane_id: "a" })] }), NOW, { openKey: null, drawer, onRowClick: () => {} });
    expect(main.querySelector('[data-key="local|b"]')).toBeNull();
    expect(drawer.isConnected).toBe(false);
  });

  it("renders offline groups dimmed with reason and retry, after online rows", () => {
    const l = list({
      rows: [row({ pane_id: "a" })],
      offline: [{ server_id: "box", label: "build-box", reason: "Unreachable", retry_at: NOW + 30000, rows: [row({ server_id: "box", server_label: "build-box", pane_id: "z", status: "idle" })] }],
    });
    renderList(main, l, NOW, { openKey: null, drawer, onRowClick: () => {} });
    const group = main.querySelector(".group")!;
    expect(group.querySelector(".group-label")!.textContent).toBe("build-box");
    expect(group.querySelector(".group-status")!.textContent).toBe("Unreachable · retrying 30s");
    const dimmed = main.querySelectorAll(".row.dim");
    expect(dimmed.length).toBe(1);
    expect(main.children[0]!.classList.contains("row")).toBe(true);
    expect(main.children[1]!.classList.contains("group")).toBe(true);
  });

  it("shows empty states", () => {
    renderList(main, list({}), NOW, { openKey: null, drawer, onRowClick: () => {} });
    expect(main.querySelector(".empty")!.textContent).toBe("No agents running");
    const down = list({ servers: [{ id: "local", label: "local", kind: "local", connection: { state: "offline", reason: "herdr is not running", retry_at: NOW }, transport: "" }] });
    renderList(main, down, NOW, { openKey: null, drawer, onRowClick: () => {} });
    expect(main.querySelector(".empty")!.textContent).toBe("herdr is not running");
  });

  it("calls onRowClick with the row", () => {
    let clicked: AgentRow | null = null;
    renderList(main, list({ rows: [row({})] }), NOW, { openKey: null, drawer, onRowClick: (r) => (clicked = r) });
    (main.querySelector(".row") as HTMLElement).click();
    expect(clicked!.pane_id).toBe("w1:p1");
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd ui && bun run test`
Expected: fails, `../render` not found.

- [ ] **Step 3: Implement render.ts and the stylesheet**

`ui/src/render.ts`:

```ts
import { retryText, stateText } from "./format";
import { rowKey, type AgentList, type AgentRow, type OfflineGroup } from "./types";

export interface RenderOptions {
  openKey: string | null;
  drawer: HTMLElement;
  onRowClick: (row: AgentRow) => void;
}

const reducedMotion = (): boolean =>
  typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

export function renderHeader(root: HTMLElement, list: AgentList, nowMs: number): void {
  root.replaceChildren();
  const brand = document.createElement("span");
  brand.className = "brand";
  brand.textContent = "herdr";
  root.appendChild(brand);
  const chips = document.createElement("div");
  chips.className = "chips";
  for (const server of list.servers) {
    const chip = document.createElement("span");
    chip.className = `chip ${server.connection.state}`;
    chip.innerHTML = `<i></i>`;
    chip.appendChild(document.createTextNode(server.label));
    chip.title =
      server.connection.state === "offline"
        ? `${server.connection.reason} · ${retryText(server.connection.retry_at, nowMs)}${server.transport ? ` · ${server.transport}` : ""}`
        : server.transport || server.connection.state;
    chips.appendChild(chip);
  }
  root.appendChild(chips);
}

/** Row elements carry their latest row so click handlers never see stale data. */
interface RowElement extends HTMLElement {
  row?: AgentRow;
}

function buildRow(row: AgentRow, onClick: (row: AgentRow) => void): RowElement {
  const el = document.createElement("div") as RowElement;
  el.className = "row enter";
  el.dataset.key = rowKey(row);
  el.innerHTML = `<div class="dot"></div><div class="text"><div class="name"></div><div class="meta"></div></div><div class="state"></div>`;
  el.addEventListener("click", () => {
    if (el.row) onClick(el.row);
  });
  el.addEventListener("animationend", () => el.classList.remove("flash", "enter"));
  return el;
}

function fillRow(el: RowElement, row: AgentRow, nowMs: number, open: boolean, dim: boolean): void {
  el.row = row;
  const keep = ["flash", "enter"].filter((c) => el.classList.contains(c)).join(" ");
  el.className = `row ${row.status}${open ? " open" : ""}${dim ? " dim" : ""}${keep ? ` ${keep}` : ""}`;
  el.querySelector(".name")!.textContent = row.name;
  const meta = [row.server_label, row.workspace_label, row.agent_kind].filter((p) => p && p.length > 0).join(" · ");
  el.querySelector(".meta")!.textContent = meta;
  el.querySelector(".state")!.textContent = stateText(row, nowMs);
  const previous = el.dataset.flashSeq;
  if (previous !== undefined && Number(previous) !== row.flash_seq) {
    el.classList.remove("flash");
    void (el as HTMLElement).offsetWidth; // restart the animation
    el.classList.add("flash");
  }
  el.dataset.flashSeq = String(row.flash_seq);
}

function buildGroup(group: OfflineGroup, nowMs: number): HTMLElement {
  const el = document.createElement("div");
  el.className = "group";
  el.dataset.server = group.server_id;
  el.innerHTML = `<div class="group-line"><span class="group-label"></span><span class="group-status"></span></div>`;
  el.querySelector(".group-label")!.textContent = group.label;
  el.querySelector(".group-status")!.textContent = `${group.reason} · ${retryText(group.retry_at, nowMs)}`;
  return el;
}

function emptyText(list: AgentList): string | null {
  if (list.rows.length > 0 || list.offline.some((g) => g.rows.length > 0)) return null;
  const anyOnline = list.servers.some((s) => s.connection.state === "online");
  if (anyOnline) return "No agents running";
  const local = list.servers.find((s) => s.id === "local");
  if (local && local.connection.state === "offline") return "herdr is not running";
  return list.servers.length === 0 ? "No servers configured" : "Connecting…";
}

/**
 * Re-render the list. Existing row elements are reused (keyed), the drawer is
 * moved after the open row, offline groups follow the online rows, and rows
 * that moved get a short FLIP transition.
 */
export function renderList(root: HTMLElement, list: AgentList, nowMs: number, opts: RenderOptions): void {
  const existing = new Map<string, RowElement>();
  root.querySelectorAll<RowElement>(".row[data-key]").forEach((el) => existing.set(el.dataset.key!, el));
  const before = new Map<string, DOMRect>();
  if (!reducedMotion()) existing.forEach((el, key) => before.set(key, el.getBoundingClientRect()));

  const ordered: Node[] = [];
  const seen = new Set<string>();
  const place = (row: AgentRow, dim: boolean) => {
    const key = rowKey(row);
    let el = existing.get(key);
    if (!el) el = buildRow(row, opts.onRowClick);
    const open = opts.openKey === key;
    fillRow(el, row, nowMs, open, dim);
    seen.add(key);
    ordered.push(el);
    if (open) ordered.push(opts.drawer);
  };

  for (const row of list.rows) place(row, false);
  for (const group of list.offline) {
    ordered.push(buildGroup(group, nowMs));
    for (const row of group.rows) place(row, true);
  }
  const empty = emptyText(list);
  if (empty) {
    const el = document.createElement("div");
    el.className = "empty";
    el.textContent = empty;
    ordered.push(el);
  }
  if (opts.openKey !== null && !seen.has(opts.openKey) && opts.drawer.isConnected) opts.drawer.remove();
  root.replaceChildren(...ordered);

  if (before.size > 0) {
    seen.forEach((key) => {
      const el = existing.get(key);
      const prev = before.get(key);
      if (!el || !prev) return;
      const next = el.getBoundingClientRect();
      const dy = prev.top - next.top;
      if (dy !== 0 && Number.isFinite(dy) && typeof el.animate === "function") {
        el.animate([{ transform: `translateY(${dy}px)` }, { transform: "translateY(0)" }], { duration: 200, easing: "ease-out" });
      }
    });
  }
}
```

`ui/src/styles.css` (full):

```css
:root {
  color-scheme: dark;
  --bg: #0f1115;
  --row-hover: #151922;
  --border: #262a33;
  --text: #e6e8ee;
  --muted: #8a90a0;
  --dim: #6b7180;
  --working: #3b9dff;
  --blocked: #ff9f0a;
  --blocked-bg: #2a1a0c;
  --blocked-text: #ffb067;
  --done: #34c759;
  --done-text: #5fe39a;
  --idle: #5c6270;
  --flash: #1f3a5f;
  --drawer-bg: #171b23;
  --key-bg: #232936;
  --key-border: #2f3645;
  --mono: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}
* { box-sizing: border-box; }
html, body { margin: 0; height: 100%; background: var(--bg); color: var(--text); font-family: system-ui, -apple-system, "Segoe UI", sans-serif; font-size: 13px; -webkit-font-smoothing: antialiased; user-select: none; }
#app { display: flex; flex-direction: column; height: 100%; padding: 10px; gap: 4px; }
#header { display: flex; align-items: center; justify-content: space-between; padding: 4px 6px 8px; border-bottom: 1px solid #1c2029; }
.brand { font-size: 11px; font-weight: 700; letter-spacing: .08em; text-transform: uppercase; color: var(--muted); }
.chips { display: flex; gap: 6px; flex-wrap: wrap; justify-content: flex-end; }
.chip { font-size: 10.5px; color: #aeb4c2; background: #171b23; border-radius: 999px; padding: 2px 8px; display: inline-flex; align-items: center; gap: 5px; }
.chip i { width: 6px; height: 6px; border-radius: 50%; background: var(--idle); display: inline-block; }
.chip.online i { background: var(--done); }
.chip.offline { color: var(--dim); }
.chip.connecting i { animation: pulse 1.2s ease-in-out infinite; }
#list { flex: 1; overflow-y: auto; overflow-x: hidden; }
#banner { background: #3a1212; color: #ffb3b3; border: 1px solid #6a2020; border-radius: 8px; padding: 8px 10px; font-size: 11.5px; margin-bottom: 6px; white-space: pre-wrap; }
#banner[hidden] { display: none; }
.row { display: flex; align-items: center; gap: 10px; height: 40px; padding: 8px 6px; border-radius: 8px; cursor: pointer; }
.row:hover { background: var(--row-hover); }
.row .dot { width: 9px; height: 9px; border-radius: 50%; flex-shrink: 0; background: var(--idle); }
.row.working .dot { background: var(--working); box-shadow: 0 0 10px #3b9dff99; }
.row.blocked .dot { background: var(--blocked); box-shadow: 0 0 10px #ff9f0a99; }
.row.done .dot { background: var(--done); box-shadow: 0 0 10px #34c75999; }
.row.unknown .dot { background: transparent; border: 2px solid var(--idle); width: 7px; height: 7px; }
.row .text { flex: 1; min-width: 0; }
.row .name { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.row .meta { font-size: 11px; color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.row .state { font-size: 11px; color: var(--muted); font-variant-numeric: tabular-nums; white-space: nowrap; }
.row.blocked { background: var(--blocked-bg); }
.row.blocked .state { color: var(--blocked-text); font-weight: 700; }
.row.done .state { color: var(--done-text); }
.row.open { border-radius: 8px 8px 0 0; }
.row.dim { opacity: .4; }
.row.flash { animation: flash 1s ease-out; }
.row.enter { animation: enter 150ms ease-out; }
@keyframes enter { from { opacity: 0; } to { opacity: 1; } }
@keyframes flash { from { background-color: var(--flash); } to { background-color: transparent; } }
.row.blocked.flash { animation: flash-blocked 1s ease-out; }
@keyframes flash-blocked { from { background-color: var(--flash); } to { background-color: var(--blocked-bg); } }
@keyframes pulse { 0%, 100% { opacity: .35; } 50% { opacity: 1; } }
.group { margin-top: 6px; }
.group-line { display: flex; justify-content: space-between; align-items: center; padding: 8px 6px 2px; font-size: 10px; font-weight: 700; letter-spacing: .08em; text-transform: uppercase; color: #4f5563; }
.group-status { font-weight: 600; letter-spacing: 0; text-transform: none; color: var(--dim); font-size: 11px; }
.empty { color: var(--dim); text-align: center; padding: 40px 10px; font-size: 12px; }
.drawer { background: var(--drawer-bg); border: 1px solid var(--border); border-top: none; border-radius: 0 0 10px 10px; padding: 8px; margin: 0 0 6px; }
.tabs { display: flex; gap: 4px; margin-bottom: 8px; }
.tab { font-size: 10.5px; font-weight: 600; color: var(--muted); padding: 3px 8px; border-radius: 6px; background: #1f242e; border: none; cursor: pointer; font-family: inherit; }
.tab.on { color: var(--text); background: #2b3240; }
.peek { font-family: var(--mono); font-size: 10.5px; line-height: 1.45; color: #aeb4c2; background: #0b0d11; border-radius: 6px; padding: 8px; white-space: pre; overflow: auto; max-height: 120px; margin-bottom: 8px; user-select: text; }
.peek-note { font-size: 10px; color: var(--dim); margin: -4px 0 8px; }
.keys { display: grid; grid-template-columns: repeat(5, 1fr); gap: 4px; margin-bottom: 8px; }
.key { font-size: 11px; font-weight: 600; text-align: center; padding: 5px 0; border-radius: 6px; background: var(--key-bg); color: var(--text); border: 1px solid var(--key-border); cursor: pointer; font-family: inherit; }
.key.primary { background: #2d4a1f; border-color: #3f6b2b; color: #c8f5b0; }
.key.caution { background: #3a2210; border-color: #5a351a; color: var(--blocked-text); }
.key:active { filter: brightness(1.3); }
.send { display: flex; gap: 6px; }
.send input { flex: 1; background: #0b0d11; border: 1px solid var(--border); border-radius: 6px; color: var(--text); font-size: 11.5px; padding: 6px 8px; font-family: inherit; user-select: text; }
.send button { font-size: 11px; font-weight: 700; padding: 6px 10px; border-radius: 6px; background: #0f2a44; color: #6fb8ff; border: none; cursor: pointer; font-family: inherit; }
.error { color: #ff6b6b; font-size: 11px; margin-top: 6px; }
@media (prefers-reduced-motion: reduce) {
  .row.flash, .row.blocked.flash { animation: none; background-color: var(--flash); }
  .row.enter { animation: none; }
  .chip.connecting i { animation: none; }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd ui && bun run test`
Expected: format and render suites pass (jsdom has no layout, so rects are zero and the FLIP branch is a no-op; `el.animate` is guarded because jsdom lacks it).

- [ ] **Step 5: Commit**

```bash
git add ui/src/render.ts ui/src/__tests__/render.test.ts ui/src/styles.css
git commit -m "feat(ui): keyed row rendering with flash, offline groups and full stylesheet"
```

---

### Task 9: Drawer, Tauri API wrapper and entry point

**Files:**
- Create: `ui/src/drawer.ts`, `ui/src/tauri.ts`, `ui/src/__tests__/drawer.test.ts`
- Modify: `ui/src/main.ts`

- [ ] **Step 1: Write the failing tests**

`ui/src/__tests__/drawer.test.ts`:

```ts
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Drawer, KEYPAD, type Api } from "../drawer";
import type { AgentRow } from "../types";

const row = (over: Partial<AgentRow> = {}): AgentRow => ({
  server_id: "local",
  server_label: "local",
  pane_id: "w1:p1",
  workspace_label: "ws",
  agent_kind: "claude",
  name: "reviewer",
  status: "working",
  since: null,
  flash_seq: 0,
  ...over,
});

function fakeApi(): Api & { calls: string[] } {
  const calls: string[] = [];
  return {
    calls,
    peek: vi.fn(async (server, pane) => {
      calls.push(`peek ${server} ${pane}`);
      return { text: "line 1\nline 2", truncated: false };
    }),
    sendKey: vi.fn(async (server, pane, key) => {
      calls.push(`key ${server} ${pane} ${key}`);
    }),
    sendMessage: vi.fn(async (server, pane, text) => {
      calls.push(`msg ${server} ${pane} ${text}`);
    }),
  };
}

const flush = () => Promise.resolve().then(() => Promise.resolve());

let api: ReturnType<typeof fakeApi>;
let drawer: Drawer;

beforeEach(() => {
  vi.useFakeTimers();
  document.body.innerHTML = "";
  api = fakeApi();
  drawer = new Drawer(api, 2000, 5000);
  document.body.appendChild(drawer.el);
});

afterEach(() => {
  drawer.close();
  vi.useRealTimers();
});

describe("Drawer", () => {
  it("opens on peek, polls every 2s, stops on close", async () => {
    drawer.open(row());
    await flush();
    expect(drawer.currentKey).toBe("local|w1:p1");
    expect(drawer.el.querySelector(".tab.on")!.textContent).toBe("Peek");
    expect(drawer.el.querySelector(".peek")!.textContent).toBe("line 1\nline 2");
    expect(api.calls.filter((c) => c.startsWith("peek")).length).toBe(1);
    await vi.advanceTimersByTimeAsync(4100);
    expect(api.calls.filter((c) => c.startsWith("peek")).length).toBe(3);
    drawer.close();
    await vi.advanceTimersByTimeAsync(4000);
    expect(api.calls.filter((c) => c.startsWith("peek")).length).toBe(3);
    expect(drawer.currentKey).toBeNull();
  });

  it("shows the keypad under the peek only for blocked rows, and always on the keys tab", async () => {
    drawer.open(row({ status: "working" }));
    await flush();
    expect(drawer.el.querySelector(".keys")).toBeNull();
    drawer.update(row({ status: "blocked" }));
    await flush();
    expect(drawer.el.querySelectorAll(".keys .key").length).toBe(KEYPAD.length);
    (drawer.el.querySelector('[data-tab="keys"]') as HTMLElement).click();
    expect(drawer.el.querySelectorAll(".keys .key").length).toBe(10);
    (drawer.el.querySelector('.key[data-key="1"]') as HTMLElement).click();
    await flush();
    expect(api.calls).toContain("key local w1:p1 1");
    expect(drawer.el.querySelector('.key[data-key="enter"]')!.className).toContain("primary");
    expect(drawer.el.querySelector('.key[data-key="ctrl+c"]')!.className).toContain("caution");
  });

  it("sends a message on Enter, clears on success, keeps text and shows an error on failure", async () => {
    drawer.open(row());
    (drawer.el.querySelector('[data-tab="message"]') as HTMLElement).click();
    const input = drawer.el.querySelector(".send input") as HTMLInputElement;
    input.value = "continue";
    input.form!.dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    expect(api.calls).toContain("msg local w1:p1 continue");
    expect(input.value).toBe("");

    (api.sendMessage as ReturnType<typeof vi.fn>).mockRejectedValueOnce({ code: "agent_blocked", message: "Waiting at a prompt. Answer it with the keypad first." });
    input.value = "again";
    input.form!.dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    expect(input.value).toBe("again");
    const error = drawer.el.querySelector(".error") as HTMLElement;
    expect(error.hidden).toBe(false);
    expect(error.textContent).toBe("Waiting at a prompt. Answer it with the keypad first.");
    await vi.advanceTimersByTimeAsync(5100);
    expect(error.hidden).toBe(true);
  });

  it("toggle closes the same row and switches to another", async () => {
    drawer.toggle(row({ pane_id: "a" }));
    expect(drawer.currentKey).toBe("local|a");
    drawer.toggle(row({ pane_id: "b" }));
    expect(drawer.currentKey).toBe("local|b");
    drawer.toggle(row({ pane_id: "b" }));
    expect(drawer.currentKey).toBeNull();
  });

  it("peek failure shows the error but keeps polling", async () => {
    (api.peek as ReturnType<typeof vi.fn>).mockRejectedValueOnce({ code: "transport", message: "Could not reach local." });
    drawer.open(row());
    await flush();
    expect((drawer.el.querySelector(".error") as HTMLElement).textContent).toBe("Could not reach local.");
    await vi.advanceTimersByTimeAsync(2100);
    expect(drawer.el.querySelector(".peek")!.textContent).toBe("line 1\nline 2");
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd ui && bun run test`
Expected: fails, `../drawer` not found.

- [ ] **Step 3: Implement drawer, tauri wrapper and main**

`ui/src/drawer.ts`:

```ts
import { rowKey, type ActionError, type AgentRow, type PeekResult } from "./types";

export interface Api {
  peek(server: string, pane: string): Promise<PeekResult>;
  sendKey(server: string, pane: string, key: string): Promise<void>;
  sendMessage(server: string, pane: string, text: string): Promise<void>;
}

export type Tab = "peek" | "keys" | "message";

/** Same order and allow-list as `herdr_status_core::actions::KEYPAD`. */
export const KEYPAD: ReadonlyArray<{ key: string; label: string; cls: string }> = [
  { key: "1", label: "1", cls: "primary" },
  { key: "2", label: "2", cls: "" },
  { key: "3", label: "3", cls: "" },
  { key: "y", label: "y", cls: "" },
  { key: "n", label: "n", cls: "" },
  { key: "up", label: "↑", cls: "" },
  { key: "down", label: "↓", cls: "" },
  { key: "enter", label: "Enter", cls: "primary" },
  { key: "esc", label: "Esc", cls: "caution" },
  { key: "ctrl+c", label: "^C", cls: "caution" },
];

function messageOf(err: unknown): string {
  if (err && typeof err === "object" && "message" in err) return String((err as ActionError).message);
  return String(err);
}

export class Drawer {
  readonly el: HTMLElement;
  private row: AgentRow | null = null;
  private tab: Tab = "peek";
  private pollTimer: ReturnType<typeof setInterval> | null = null;
  private errorTimer: ReturnType<typeof setTimeout> | null = null;
  private peekText = "";
  private peekTruncated = false;
  private draft = "";

  constructor(
    private readonly api: Api,
    private readonly peekIntervalMs = 2000,
    private readonly errorMs = 5000,
  ) {
    this.el = document.createElement("div");
    this.el.className = "drawer";
    this.el.addEventListener("click", (e) => e.stopPropagation());
  }

  get currentKey(): string | null {
    return this.row ? rowKey(this.row) : null;
  }

  isOpen(): boolean {
    return this.row !== null;
  }

  toggle(row: AgentRow): void {
    if (this.currentKey === rowKey(row)) this.close();
    else this.open(row);
  }

  open(row: AgentRow): void {
    this.stopPolling();
    this.row = row;
    this.tab = "peek";
    this.peekText = "";
    this.peekTruncated = false;
    this.render();
    this.startPolling();
  }

  close(): void {
    this.stopPolling();
    this.row = null;
    this.el.replaceChildren();
    this.el.remove();
  }

  /** Called on every list render while open, so status changes update the keypad. */
  update(row: AgentRow): void {
    if (!this.row || rowKey(row) !== rowKey(this.row)) return;
    const statusChanged = row.status !== this.row.status;
    this.row = row;
    if (statusChanged) this.render();
  }

  private startPolling(): void {
    void this.refreshPeek();
    this.pollTimer = setInterval(() => {
      if (this.tab === "peek") void this.refreshPeek();
    }, this.peekIntervalMs);
  }

  private stopPolling(): void {
    if (this.pollTimer !== null) clearInterval(this.pollTimer);
    this.pollTimer = null;
  }

  private async refreshPeek(): Promise<void> {
    if (!this.row) return;
    try {
      const result = await this.api.peek(this.row.server_id, this.row.pane_id);
      this.peekText = result.text;
      this.peekTruncated = result.truncated;
      this.applyPeek();
    } catch (err) {
      this.showError(err);
    }
  }

  private applyPeek(): void {
    const pre = this.el.querySelector<HTMLElement>(".peek");
    if (!pre) return;
    pre.textContent = this.peekText;
    pre.scrollTop = pre.scrollHeight;
    const note = this.el.querySelector<HTMLElement>(".peek-note");
    if (note) note.hidden = !this.peekTruncated;
  }

  private async press(key: string): Promise<void> {
    if (!this.row) return;
    try {
      await this.api.sendKey(this.row.server_id, this.row.pane_id, key);
    } catch (err) {
      this.showError(err);
    }
  }

  private async send(input: HTMLInputElement): Promise<void> {
    if (!this.row) return;
    const text = input.value;
    try {
      await this.api.sendMessage(this.row.server_id, this.row.pane_id, text);
      input.value = "";
      this.draft = "";
    } catch (err) {
      this.draft = text;
      this.showError(err);
    }
  }

  private showError(err: unknown): void {
    const box = this.el.querySelector<HTMLElement>(".error");
    if (!box) return;
    box.textContent = messageOf(err);
    box.hidden = false;
    if (this.errorTimer !== null) clearTimeout(this.errorTimer);
    this.errorTimer = setTimeout(() => {
      box.hidden = true;
    }, this.errorMs);
  }

  private keypad(): HTMLElement {
    const keys = document.createElement("div");
    keys.className = "keys";
    for (const { key, label, cls } of KEYPAD) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = `key ${cls}`.trim();
      b.dataset.key = key;
      b.textContent = label;
      b.addEventListener("click", () => void this.press(key));
      keys.appendChild(b);
    }
    return keys;
  }

  private render(): void {
    if (!this.row) return;
    this.el.replaceChildren();
    const tabs = document.createElement("div");
    tabs.className = "tabs";
    for (const [id, label] of [["peek", "Peek"], ["keys", "Keys"], ["message", "Message"]] as const) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = `tab${this.tab === id ? " on" : ""}`;
      b.dataset.tab = id;
      b.textContent = label;
      b.addEventListener("click", () => {
        this.tab = id;
        this.render();
        if (id === "peek") void this.refreshPeek();
      });
      tabs.appendChild(b);
    }
    this.el.appendChild(tabs);

    const content = document.createElement("div");
    content.className = "content";
    if (this.tab === "peek") {
      const pre = document.createElement("pre");
      pre.className = "peek";
      content.appendChild(pre);
      const note = document.createElement("div");
      note.className = "peek-note";
      note.textContent = "output truncated";
      note.hidden = true;
      content.appendChild(note);
      if (this.row.status === "blocked") content.appendChild(this.keypad());
    } else if (this.tab === "keys") {
      content.appendChild(this.keypad());
    } else {
      const form = document.createElement("form");
      form.className = "send";
      const input = document.createElement("input");
      input.type = "text";
      input.placeholder = "Message to agent…";
      input.value = this.draft;
      const button = document.createElement("button");
      button.type = "submit";
      button.textContent = "Send";
      form.append(input, button);
      form.addEventListener("submit", (e) => {
        e.preventDefault();
        void this.send(input);
      });
      content.appendChild(form);
      queueMicrotask(() => input.focus());
    }
    this.el.appendChild(content);

    const error = document.createElement("div");
    error.className = "error";
    error.hidden = true;
    this.el.appendChild(error);
    this.applyPeek();
  }
}
```

`ui/src/tauri.ts`:

```ts
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Api } from "./drawer";
import type { AgentList, PeekResult } from "./types";

export interface AppApi extends Api {
  getAgents(): Promise<AgentList>;
  onAgents(cb: (list: AgentList) => void): Promise<UnlistenFn>;
  onConfigError(cb: (message: string | null) => void): Promise<UnlistenFn>;
}

export const api: AppApi = {
  getAgents: () => invoke<AgentList>("get_agents"),
  peek: (server, pane) => invoke<PeekResult>("peek", { server, pane }),
  sendKey: (server, pane, key) => invoke<void>("send_key", { server, pane, key }),
  sendMessage: (server, pane, text) => invoke<void>("send_message", { server, pane, text }),
  onAgents: (cb) => listen<AgentList>("agents", (e) => cb(e.payload)),
  onConfigError: (cb) => listen<string | null>("config_error", (e) => cb(e.payload)),
};
```

`ui/src/main.ts`:

```ts
import { Drawer } from "./drawer";
import { renderHeader, renderList } from "./render";
import { api } from "./tauri";
import { EMPTY_LIST, rowKey, type AgentList, type AgentRow } from "./types";

const header = document.querySelector<HTMLElement>("#header")!;
const listEl = document.querySelector<HTMLElement>("#list")!;
const banner = document.querySelector<HTMLElement>("#banner")!;

let list: AgentList = EMPTY_LIST;
const drawer = new Drawer(api);

function findRow(key: string): AgentRow | null {
  return list.rows.find((r) => rowKey(r) === key) ?? list.offline.flatMap((g) => g.rows).find((r) => rowKey(r) === key) ?? null;
}

function render(): void {
  const now = Date.now();
  renderHeader(header, list, now);
  const openKey = drawer.currentKey;
  if (openKey !== null) {
    const current = findRow(openKey);
    if (current) drawer.update(current);
    else drawer.close();
  }
  renderList(listEl, list, now, {
    openKey: drawer.currentKey,
    drawer: drawer.el,
    onRowClick: (row) => {
      drawer.toggle(row);
      render();
    },
  });
}

void api.getAgents().then((initial) => {
  list = initial;
  render();
});
void api.onAgents((next) => {
  list = next;
  render();
});
void api.onConfigError((message) => {
  banner.hidden = message === null;
  banner.textContent = message ?? "";
});
setInterval(render, 15_000);
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && drawer.isOpen()) {
    drawer.close();
    render();
  }
});
```

- [ ] **Step 4: Run the tests, then the app**

Run: `cd ui && bun run test`
Expected: format, render and drawer suites pass.

Run: `cd ui && bun tauri dev` with herdr running and at least one agent.
Expected: real rows appear within a second of launch; clicking a row opens the drawer with live output refreshing; Keys sends to the agent (watch the agent's terminal); Message submits a prompt; Escape closes; a state change in the agent flashes the row and re-sorts.

- [ ] **Step 5: Commit**

```bash
git add ui/src/drawer.ts ui/src/tauri.ts ui/src/main.ts ui/src/__tests__/drawer.test.ts
git commit -m "feat(ui): action drawer with peek polling, keypad and message; app entry"
```

---

### Task 10: Live verification against the real herdr

**Files:**
- Create: `crates/herdr-client/tests/live_herdr.rs`

- [ ] **Step 1: Add the opt-in live test**

`crates/herdr-client/tests/live_herdr.rs`:

```rust
//! Talks to the real local herdr. Opt in with `HERDR_STATUS_LIVE=1`.
#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use herdr_client::transport::unix::UnixSocketTransport;
use herdr_client::{Client, Method, ResponseResult, Subscription};

fn live_socket() -> Option<std::path::PathBuf> {
    if std::env::var("HERDR_STATUS_LIVE").ok().as_deref() != Some("1") {
        return None;
    }
    let home = std::env::var("HOME").ok()?;
    let path = std::path::PathBuf::from(home).join(".config/herdr/herdr.sock");
    path.exists().then_some(path)
}

#[tokio::test]
async fn ping_snapshot_and_short_subscription() {
    let Some(path) = live_socket() else {
        eprintln!("skipped: set HERDR_STATUS_LIVE=1 with herdr running");
        return;
    };
    let client = Client::new(Arc::new(UnixSocketTransport::new(path)));
    match client.request(Method::Ping).await.unwrap() {
        ResponseResult::Pong { protocol, .. } => assert!(protocol >= 22),
        other => panic!("unexpected {other:?}"),
    }
    let snapshot = client.snapshot().await.unwrap();
    assert!(!snapshot.workspaces.is_empty());
    let ids: Vec<String> = snapshot.pane_ids().iter().map(|s| s.to_string()).collect();
    let subs = [Subscription::lifecycle("pane.created")]
        .into_iter()
        .chain(ids.iter().map(|p| Subscription::pane_agent_status(p)))
        .collect();
    let mut stream = client.subscribe(subs).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), stream.next()).await;
}
```

Run: `HERDR_STATUS_LIVE=1 cargo test -p herdr-client --test live_herdr -- --nocapture`
Expected: passes against the running herdr (or prints `skipped` when herdr is down).

- [ ] **Step 2: Walk the macOS checklist with `bun tauri dev`**

Verify every line; fix and commit anything that fails before continuing:

1. Launch with herdr running: `local` chip green within 1 s; one row per agent pane; names match herdr's sidebar titles.
2. In an agent pane, ask the agent something so it goes `working` then `idle`: the row flashes and its state text changes within 1 s; the right-hand time starts counting from that moment.
3. Trigger a permission prompt in an agent (for Claude Code, ask it to run a shell command without auto-approval): the row turns amber, moves to the top, tray icon turns amber and the menu bar shows `1`.
4. Open that row: Peek shows the prompt text; the keypad is visible under it; press `1` or `Enter`: the agent continues; the row leaves the blocked state.
5. Message tab: type `summarize what you did in one line` and press Enter; the agent receives it.
6. `herdr server stop` in another terminal (only if you are fine stopping your session; otherwise skip): chip grey, rows vanish, "herdr is not running"; restart herdr: chip green, rows back within 5 s.
7. Edit `config.toml` to add a bogus `[[remote]]` with `ssh_target = "nohost.invalid"`: within a second a `nohost…` chip appears grey with an offline group reading `Unreachable · retrying …`; remove it: the chip disappears.
8. Resize the window narrow (260 px): names ellipsise, nothing wraps.
9. Close the window, click the tray: it comes back where it was.

- [ ] **Step 3: Commit**

```bash
git add crates/herdr-client/tests/live_herdr.rs
git commit -m "test(client): opt-in live test against the local herdr"
```

---

### Task 11: CI workflow

**Files:**
- Create: `.github/workflows/build.yml`

- [ ] **Step 1: Write the workflow**

`.github/workflows/build.yml`:

```yaml
name: build

on:
  push:
    branches: [main]
  pull_request:
  workflow_dispatch:

jobs:
  test-and-build:
    strategy:
      fail-fast: false
      matrix:
        os: [macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: ". -> target"
      - uses: oven-sh/setup-bun@v2
        with:
          bun-version: latest
      - name: Install UI dependencies
        run: bun install --frozen-lockfile
        working-directory: ui
      - name: Format check
        run: cargo fmt --all --check
      - name: Clippy
        run: cargo clippy --workspace --all-targets --features herdr-client/test-support -- -D warnings
      - name: Rust tests
        run: cargo test --workspace --features herdr-client/test-support
      - name: UI tests
        run: bun run test
        working-directory: ui
      - name: Build bundles
        run: bun tauri build
        working-directory: ui
      - name: Upload bundles
        uses: actions/upload-artifact@v4
        with:
          name: herdr-status-${{ matrix.os }}
          if-no-files-found: ignore
          path: |
            src-tauri/target/release/bundle/dmg/*.dmg
            src-tauri/target/release/bundle/macos/*.app
            src-tauri/target/release/bundle/msi/*.msi
            src-tauri/target/release/bundle/nsis/*.exe
```

On Windows the `test_support` module is `cfg(unix)` and every fake-server test file starts with `#![cfg(unix)]`, so the Windows job runs the protocol, launcher, parser, model, reducer, pipe-name and named-pipe tests, then builds the installer. The lockfile `ui/bun.lock` must be committed for `--frozen-lockfile`.

- [ ] **Step 2: Verify locally what can be verified**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --features herdr-client/test-support -- -D warnings && cargo test --workspace --features herdr-client/test-support && (cd ui && bun run test)`
Expected: all green on macOS.

- [ ] **Step 3: Commit and push, then check the Actions tab**

```bash
git add .github/workflows/build.yml ui/bun.lock
git commit -m "ci: test and build bundles on macOS and Windows"
```

Push to GitHub (create the repository first with `gh repo create herdr-status --private --source . --push` if it does not exist). Expected: both jobs green; the Windows artifact contains an `.msi` and an `.exe` installer. Download the Windows artifact for the smoke test.

---

### Task 12: README and Windows smoke checklist

**Files:**
- Create: `README.md`, `docs/windows-smoke-checklist.md`

- [ ] **Step 1: Write the README**

`README.md`:

```markdown
# herdr-status

A small native status bar for the AI coding agents running under [herdr](https://herdr.dev):
one row per agent, sorted so anything that needs you is on top, with peek / keypad / message
quick actions and a tray icon. macOS and Windows. Built with Tauri 2 and Rust.

## What it watches

- **Local herdr**: the Unix socket (macOS) or named pipe (Windows) of the default session.
- **WSL** (Windows only): herdr running inside a WSL distro, reached through `wsl.exe`.
- **Remote machines over SSH**: Linux, macOS or native Windows hosts running herdr, reached with
  herdr's own `remote-api-bridge`. No `herdr machine add` needed; any SSH target works.

## Requirements

- herdr 0.9.3 or later on every machine you want to watch, with its server running.
- SSH key access to remote hosts (the bar never prompts for passwords; run `ssh host` once first).
- macOS 12+ or Windows 10/11 (WebView2 is preinstalled on both).

## Install

Download the bundle from the GitHub Actions artifacts (`.dmg` for macOS, `.msi` or `.exe` for Windows),
or build it yourself:

    bun install --cwd ui
    cargo tauri build        # or: cd ui && bun tauri build

The macOS app is ad-hoc signed: right-click → Open the first time. The Windows installer is unsigned:
SmartScreen asks once.

## Configure

The config file is created on first launch and reloaded whenever you save it
(tray menu → *Edit config…*):

- macOS: `~/.config/herdr-status/config.toml`
- Windows: `%APPDATA%\herdr-status\config.toml`

    [local]
    enabled = true
    session = "default"

    [[wsl]]                     # Windows only
    label = "wsl"
    # distro = "Ubuntu"

    [[remote]]
    label = "winbox"
    ssh_target = "winbox"       # alias, user@host, ssh://user@host:2222
    session = "default"
    # os = "windows"            # auto-detected when omitted
    # herdr_path = "C:\\...\\herdr.exe"

    [ui]
    peek_lines = 30
    reconcile_seconds = 30

On first launch the bar prefills `[[remote]]` entries from `herdr machine list` and, on Windows,
detects herdr inside the default WSL distro.

## Use

- Rows: amber = needs you, blue = working, green = done, grey = idle. The right-hand text is the
  time in that state since the bar saw it change.
- Click a row for the drawer: **Peek** (last lines of the terminal, refreshed every 2 s),
  **Keys** (1 2 3 y n ↑ ↓ Enter Esc ^C), **Message** (one-line prompt). Escape closes it.
- Tray / menu bar: colour follows the most urgent state; the count of agents needing you is shown
  next to the macOS icon or as a badge on Windows. Left click toggles the window; right click:
  Show / Hide, Reconnect all, Edit config…, Open logs, Launch at login, Quit. Closing the window hides it.

## Troubleshooting

- Chip stays grey with *herdr is not running*: `herdr status` should report a running server.
- Remote offline with an SSH reason: run the suggested `ssh <target>` in a terminal, then
  *Reconnect all*.
- *herdr not found or too old on <host>*: install or update herdr there; it needs
  `remote-api-bridge` (0.9.x).
- Logs: tray menu → *Open logs*. Set `HERDR_STATUS_LOG=debug` before launching for more detail.

## Development

    cargo test --workspace --features herdr-client/test-support
    cd ui && bun run test
    cd ui && bun tauri dev

Design: `docs/superpowers/specs/2026-10-07-herdr-status-bar-design.md`.
Plans: `docs/superpowers/plans/`.
```

- [ ] **Step 2: Write the Windows smoke checklist**

`docs/windows-smoke-checklist.md`:

```markdown
# Windows smoke checklist

Run once on a Windows machine with the installer from CI. Paste results (pass/fail + notes) back
into the PR or issue.

| # | Step | Expected | Result |
|---|------|----------|--------|
| 1 | Install from the `.msi` (or `.exe`), launch from the Start menu | Tray icon appears; window opens dark with no white flash | |
| 2 | With native herdr running (`herdr status` in a terminal shows a running server) | `local` chip green within 5 s; one row per agent pane | |
| 2b | If the chip stays grey: open logs (tray → Open logs) | Look for `primary transport unavailable; using fallback`; if present the named-pipe name is wrong but the fallback works: report the line and the `socket:` path from `herdr status` | |
| 3 | With herdr running inside WSL and no `[[wsl]]` entry yet: delete `%APPDATA%\herdr-status\config.toml`, relaunch | `[[wsl]]` entry written; `wsl` chip green; its rows present | |
| 4 | Click a row: Peek | Terminal output appears and refreshes | |
| 5 | Keys tab: press a harmless key such as `↓` in an idle agent | The agent's terminal reacts | |
| 6 | Message tab: send `say hello` | The agent receives it | |
| 7 | Close the window (×) | Window hides, process keeps running, tray click shows it | |
| 8 | Right-click tray → Launch at login, sign out and in | App is in the tray after sign-in, window hidden | |
| 9 | Edit config.toml (change `reconcile_seconds`), save | Log shows `config reloaded`; no restart needed | |
| 10 | Add a `[[remote]]` pointing at the Mac (herdr running there) | Chip green, Mac agents listed; actions work | |
| 11 | Quit from the tray | Process exits | |

Windows version: ______  herdr version: ______  WSL distro: ______
```

- [ ] **Step 3: Commit**

```bash
git add README.md docs/windows-smoke-checklist.md
git commit -m "docs: readme and windows smoke checklist"
```

---

### Task 13: Final pass and release build

- [ ] **Step 1: Lint everything**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --features herdr-client/test-support -- -D warnings && cargo test --workspace --features herdr-client/test-support && (cd ui && bun run test)`
Expected: all green.

- [ ] **Step 2: Build the macOS bundle**

Run: `cd ui && bun tauri build`
Expected: `src-tauri/target/release/bundle/dmg/herdr-status_0.1.0_aarch64.dmg` and `…/macos/herdr-status.app`. Open the `.app` (right-click → Open): tray icon appears, window shows live agents.

- [ ] **Step 3: Commit any fixes**

```bash
git add -A
git commit -m "chore: final lint pass"
```

---

## Plan 3 done criteria

- `bun tauri dev` shows live agents from your local herdr with all drawer actions working (Task 10 checklist passed).
- `bun tauri build` produces a working macOS `.dmg`.
- CI is green on both runners and the Windows artifact exists.
- The Windows smoke checklist has been run once and any named-pipe or WSL finding is recorded as an issue or fixed.
- Spec sections 10 to 17 each map to a task above; section 19 items are absent by design.
