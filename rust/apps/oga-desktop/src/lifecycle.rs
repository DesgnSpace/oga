use std::time::Duration;

use tauri::{App, AppHandle, Emitter, Manager, Runtime, WebviewWindow, Window, WindowEvent};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_window_state::{AppHandleExt, StateFlags};
use tokio::time::timeout;

pub const MAIN_WINDOW_LABEL: &str = "main";
pub const WINDOW_STATE_FILE: &str = "oga-window-state.json";
pub const VISIBILITY_EVENT: &str = "oga-window-visibility";

/// The shell keeps work going while the window is hidden, so the page needs
/// to know when to stop drawing and when to catch up.
#[derive(Clone, Copy, serde::Serialize)]
pub struct WindowVisibility {
    pub visible: bool,
}

fn announce_visibility<R: Runtime>(app: &AppHandle<R>, visible: bool) {
    let _ = app.emit(VISIBILITY_EVENT, WindowVisibility { visible });
}

const WINDOW_STATE_FLAGS: StateFlags = StateFlags::from_bits_retain(
    StateFlags::SIZE.bits()
        | StateFlags::POSITION.bits()
        | StateFlags::MAXIMIZED.bits()
        | StateFlags::FULLSCREEN.bits(),
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseBehavior {
    HideToTray,
    Exit,
}

pub const fn close_behavior() -> CloseBehavior {
    if cfg!(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    )) {
        CloseBehavior::HideToTray
    } else {
        CloseBehavior::Exit
    }
}

pub fn window_state_plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_plugin_window_state::Builder::default()
        .with_state_flags(WINDOW_STATE_FLAGS)
        .with_filename(WINDOW_STATE_FILE)
        .build()
}

pub fn configure_window<R: Runtime>(app: &App<R>) -> tauri::Result<()> {
    app.get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    Ok(())
}

pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = reveal_window(&window);
    }
}

pub fn toggle_main_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return;
    };
    let should_hide = window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false);
    if should_hide {
        let _ = window.hide();
        announce_visibility(app, false);
    } else {
        let _ = reveal_window(&window);
    }
}

/// Closing the window keeps Oga in the tray with its runs going. Quitting
/// stops the broker, so the runs it was driving stop too — they are picked up
/// from their captured sessions the next time Oga opens. That is worth a word
/// before the app disappears.
pub fn paused_runs_notice(running: usize) -> Option<(String, String)> {
    if running == 0 {
        return None;
    }
    let subject = if running == 1 {
        "1 run is paused".to_owned()
    } else {
        format!("{running} runs are paused")
    };
    Some((
        "Runs paused".to_owned(),
        format!("{subject}. They continue when you open Oga again."),
    ))
}

/// The paused-runs count comes from the broker; a broker that is down or
/// slow must not hold the quit hostage.
const QUIT_NOTICE_WAIT: Duration = Duration::from_secs(2);

pub fn quit<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(state) = app.try_state::<crate::AppState>()
            && let Ok(Ok(counts)) = timeout(QUIT_NOTICE_WAIT, state.client.get_activity()).await
            && let Some((title, body)) = paused_runs_notice(counts.running)
        {
            let _ = app.notification().builder().title(title).body(body).show();
        }
        app.exit(0);
    });
}

pub fn handle_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW_LABEL {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event
        && close_behavior() == CloseBehavior::HideToTray
    {
        api.prevent_close();
        let _ = window.hide();
        announce_visibility(window.app_handle(), false);
    }
}

#[tauri::command]
pub fn main_window_visible<R: Runtime>(app: AppHandle<R>) -> bool {
    app.get_webview_window(MAIN_WINDOW_LABEL)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(true)
}

pub fn handle_exit<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.save_window_state(WINDOW_STATE_FLAGS);
    if let Some(state) = app.try_state::<crate::AppState>()
        && let Ok(mut broker) = state.broker.try_lock()
        && let Some(broker) = broker.as_mut()
    {
        let _ = broker.shutdown();
    }
}

fn reveal_window<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    announce_visibility(window.app_handle(), true);
    Ok(())
}
