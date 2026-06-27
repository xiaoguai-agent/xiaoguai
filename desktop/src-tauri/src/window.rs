//! Show/hide/toggle helpers for the single floating window.
//!
//! The window is labelled `main` (see `tauri.conf.json`). It starts hidden and
//! is summoned by the global shortcut. On show we focus it and tell the
//! frontend to focus the input box; on hide we just hide it (state is kept so
//! the next summon is instant).

use tauri::{AppHandle, Emitter as _, Manager as _, WebviewWindow};

/// Label of the floating window, mirrored in `tauri.conf.json`.
pub const MAIN_WINDOW: &str = "main";

/// Tauri event telling the frontend the window just became visible/focused so
/// it can focus the composer. The frontend listens for this.
pub const FOCUS_INPUT_EVENT: &str = "floater://focus-input";

/// Look up the main window, if it exists.
fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(MAIN_WINDOW)
}

/// Reveal + focus the window and ask the frontend to focus the input.
pub fn show(app: &AppHandle) {
    if let Some(win) = main_window(app) {
        let _ = win.show();
        let _ = win.set_focus();
        // Let the webview focus its <textarea>; emitting is best-effort.
        let _ = app.emit(FOCUS_INPUT_EVENT, ());
    }
}

/// Hide the window (keeps it alive in the background for an instant re-summon).
pub fn hide(app: &AppHandle) {
    if let Some(win) = main_window(app) {
        let _ = win.hide();
    }
}

/// Toggle visibility. If currently visible AND focused, hide; otherwise show
/// (this makes the global shortcut feel right whether the window is hidden,
/// or visible-but-behind another app).
pub fn toggle(app: &AppHandle) {
    if let Some(win) = main_window(app) {
        let visible = win.is_visible().unwrap_or(false);
        let focused = win.is_focused().unwrap_or(false);
        if visible && focused {
            hide(app);
        } else {
            show(app);
        }
    }
}
