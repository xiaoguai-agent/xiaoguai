//! Xiaoguai Floater — a lightweight, always-on-top floating chat window that
//! talks to a local `xiaoguai serve` (default `http://localhost:7600`).
//!
//! Behaviour:
//!   * Global shortcut `Alt+Space` toggles the window (show + focus / hide).
//!   * Losing focus (blur) auto-hides the window.
//!   * `Esc` (handled in the frontend) hides the window.
//!   * The window starts hidden; the shortcut is the primary entry point.
//!
//! All HTTP runs on the Rust side (see [`serve_client`]) to bypass webview
//! CORS and to stream SSE as Tauri events.

mod config;
mod serve_client;
mod window;

use std::sync::Arc;

use config::AppConfig;
use tauri::{AppHandle, Manager as _, WindowEvent};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};

/// Shared, immutable connection config injected into every command.
struct FloaterState {
    config: AppConfig,
}

/// `Alt+Space` — the toggle hotkey.
fn toggle_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::ALT), Code::Space)
}

/// Tauri command: create a new chat session, returning its id. The frontend
/// calls this once per conversation (lazily, on the first message).
#[tauri::command]
async fn create_session(
    state: tauri::State<'_, Arc<FloaterState>>,
) -> Result<String, serve_client::ServeError> {
    serve_client::create_session(&state.config).await
}

/// Tauri command: send a message and stream the reply. Frames arrive on the
/// frontend over the `chat://event` channel (see [`serve_client::CHAT_EVENT`]).
/// Returns once the stream completes; the streamed frames carry the content.
#[tauri::command]
async fn send_message(
    app: AppHandle,
    state: tauri::State<'_, Arc<FloaterState>>,
    session_id: String,
    content: String,
) -> Result<(), serve_client::ServeError> {
    serve_client::stream_message(&app, &state.config, &session_id, &content).await
}

/// Tauri command: hide the window (the frontend calls this on `Esc`).
#[tauri::command]
fn hide_window(app: AppHandle) {
    window::hide(&app);
}

/// Build and run the floater application.
///
/// # Panics
/// Panics if the Tauri runtime fails to start — there is nothing to fall back
/// to for a GUI app, so a hard failure is the correct behaviour.
pub fn run() {
    let config = AppConfig::from_env();
    let state = Arc::new(FloaterState { config });

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_shortcut(toggle_shortcut())
                .expect("register Alt+Space shortcut")
                .with_handler(|app, shortcut, event| {
                    // Fire on key-press only (ignore the release edge) so a
                    // single tap toggles exactly once.
                    if shortcut == &toggle_shortcut() && event.state() == ShortcutState::Pressed {
                        window::toggle(app);
                    }
                })
                .build(),
        )
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            create_session,
            send_message,
            hide_window
        ])
        .on_window_event(|win, event| {
            // Auto-hide on blur: when the window loses focus, tuck it away so the
            // floater behaves like Spotlight / Raycast. `Focused(false)` is the
            // blur edge.
            if let WindowEvent::Focused(false) = event {
                window::hide(win.app_handle());
            }
        })
        .setup(|app| {
            // The window is declared `visible: false` in tauri.conf.json, so it
            // starts hidden. Nothing to do here beyond confirming it exists; the
            // global shortcut summons it.
            debug_assert!(
                app.get_webview_window(window::MAIN_WINDOW).is_some(),
                "main window must be declared in tauri.conf.json"
            );
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Xiaoguai Floater");
}
