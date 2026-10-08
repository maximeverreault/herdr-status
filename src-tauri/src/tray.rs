//! Tray / menu bar icon: colour by priority, count badge or title, menu.

use std::sync::{Arc, Mutex};

use herdr_status_core::AgentList;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_autostart::ManagerExt;

use crate::state::{lock, AppState};
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

/// What was last pushed to the tray, so identical updates are skipped.
#[derive(Default)]
struct Shown {
    icon: Option<(Rgb, Option<String>)>,
    tooltip: Option<String>,
}

static SHOWN: Mutex<Shown> = Mutex::new(Shown {
    icon: None,
    tooltip: None,
});

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, MENU_SHOW, "Show / Hide", true, None::<&str>)?;
    let reconnect = MenuItem::with_id(app, MENU_RECONNECT, "Reconnect all", true, None::<&str>)?;
    let config = MenuItem::with_id(app, MENU_CONFIG, "Edit config…", true, None::<&str>)?;
    let logs = MenuItem::with_id(app, MENU_LOGS, "Open logs", true, None::<&str>)?;
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(
        app,
        MENU_AUTOSTART,
        "Launch at login",
        true,
        autostart_enabled,
        None::<&str>,
    )?;
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
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                window::toggle(tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(TrayMenuState {
        autostart_item: autostart,
    });
    Ok(())
}

fn handle_menu(app: &AppHandle, id: &str) {
    match id {
        MENU_SHOW => window::toggle(app),
        MENU_RECONNECT => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let state = app.state::<Arc<AppState>>();
                let guard = state.hub.lock().await;
                if let Some(hub) = guard.as_ref() {
                    hub.reconnect_all();
                }
            });
        }
        MENU_CONFIG => {
            if let Err(e) = crate::commands::open_config(app.clone(), app.state::<Arc<AppState>>())
            {
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
            let result = if enabled {
                manager.disable()
            } else {
                manager.enable()
            };
            if let Err(e) = result {
                log::warn!("autostart toggle failed: {e}");
            }
            let now_enabled = manager.is_enabled().unwrap_or(!enabled);
            let _ = app
                .state::<TrayMenuState>()
                .autostart_item
                .set_checked(now_enabled);
        }
        MENU_QUIT => app.exit(0),
        _ => {}
    }
}

/// Called with every published list. Skips work when nothing visible changed.
pub fn update_tray(app: &AppHandle, list: &AgentList) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let colour = colour_for(&list.counts);
    let badge = badge_text(list.counts.blocked);
    let text = tooltip(list);
    let mut shown = lock(&SHOWN);
    let icon_key = (colour, badge);
    if shown.icon.as_ref() != Some(&icon_key) {
        let (colour, badge) = &icon_key;
        if cfg!(target_os = "macos") {
            let _ = tray.set_icon(Some(image(*colour, None)));
            let _ = tray.set_title(badge.as_deref());
        } else {
            let _ = tray.set_icon(Some(image(*colour, badge.as_deref())));
        }
        shown.icon = Some(icon_key);
    }
    if shown.tooltip.as_deref() != Some(text.as_str()) {
        let _ = tray.set_tooltip(Some(text.as_str()));
        shown.tooltip = Some(text);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use herdr_status_core::Counts;

    fn list(blocked: usize, working: usize, done: usize) -> AgentList {
        AgentList {
            counts: Counts {
                blocked,
                working,
                done,
                idle: 0,
                unknown: 0,
            },
            ..Default::default()
        }
    }

    #[test]
    fn tooltip_lists_only_non_zero_counts() {
        assert_eq!(
            tooltip(&list(1, 2, 1)),
            "herdr-status: 1 needs you · 2 working · 1 done"
        );
        assert_eq!(tooltip(&list(0, 3, 0)), "herdr-status: 3 working");
        assert_eq!(tooltip(&list(0, 0, 0)), "herdr-status: no agents");
    }
}
