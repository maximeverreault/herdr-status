//! Main window helpers.

use tauri::{AppHandle, Manager};

pub fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// On Windows a tray click moves focus to the taskbar before the click-Up
/// event, so focus is meaningless there and visibility alone decides.
fn should_hide(windows: bool, visible: bool, focused: bool) -> bool {
    if windows {
        visible
    } else {
        visible && focused
    }
}

/// Left tray click: hide when visible and focused, otherwise show and focus.
pub fn toggle(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let visible = w.is_visible().unwrap_or(false);
    let focused = w.is_focused().unwrap_or(false);
    if should_hide(cfg!(windows), visible, focused) {
        let _ = w.hide();
    } else {
        show(app);
    }
}

/// The close button hides the window; the app lives in the tray.
pub fn install_close_to_hide(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let target = w.clone();
    w.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = target.hide();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::should_hide;

    #[test]
    fn windows_hides_whenever_visible() {
        assert!(should_hide(true, true, false));
        assert!(should_hide(true, true, true));
        assert!(!should_hide(true, false, false));
    }

    #[test]
    fn other_platforms_need_visible_and_focused() {
        assert!(should_hide(false, true, true));
        assert!(!should_hide(false, true, false));
        assert!(!should_hide(false, false, true));
    }
}
