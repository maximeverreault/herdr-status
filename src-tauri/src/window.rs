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
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let visible = w.is_visible().unwrap_or(false);
    let focused = w.is_focused().unwrap_or(false);
    if visible && focused {
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
