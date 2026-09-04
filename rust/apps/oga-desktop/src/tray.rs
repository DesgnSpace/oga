use std::time::Duration;

use oga_client::LoopbackClient;
use oga_domain::ActivityCounts;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::time::sleep;

use crate::{commands, lifecycle};

pub const TRAY_ID: &str = "oga-tray";
pub const ACTIVITY_EVENT: &str = "oga-activity-changed";
const ACTIVITY_POLL_INTERVAL: Duration = Duration::from_secs(2);

fn tray_title(counts: ActivityCounts) -> String {
    (counts.running > 0)
        .then(|| counts.running.to_string())
        .unwrap_or_default()
}

fn accessibility_label(counts: ActivityCounts) -> String {
    if counts.running > 0 {
        format!(
            "Oga, {} {}",
            counts.running,
            plural(counts.running, "task running", "tasks running")
        )
    } else {
        "Oga".to_owned()
    }
}

#[cfg(target_os = "macos")]
fn dock_badge(counts: ActivityCounts) -> Option<String> {
    let _ = counts;
    None
}

pub fn setup<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = commands::tray_menu(app)?;
    let icon =
        tauri::image::Image::from_bytes(include_bytes!("../../../../logos/export/logo-512.png"))?;
    tauri::tray::TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("Oga")
        .build(app)?;
    Ok(())
}

pub fn handle_event<R: Runtime>(app: &AppHandle<R>, event: tauri::tray::TrayIconEvent) {
    if let tauri::tray::TrayIconEvent::Click {
        button: tauri::tray::MouseButton::Left,
        button_state: tauri::tray::MouseButtonState::Up,
        ..
    } = event
    {
        lifecycle::toggle_main_window(app);
    }
}

pub fn apply_counts<R: Runtime>(app: &AppHandle<R>, counts: ActivityCounts) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_title(Some(&tray_title(counts)));
        let _ = tray.set_tooltip(Some(&accessibility_label(counts)));
    }
    #[cfg(target_os = "macos")]
    if let Some(window) = app.get_webview_window(lifecycle::MAIN_WINDOW_LABEL) {
        let _ = window.set_badge_label(dock_badge(counts));
    }
    let _ = app.emit(ACTIVITY_EVENT, counts);
}

pub fn spawn_activity_poll<R: Runtime>(app: AppHandle<R>, client: LoopbackClient) {
    tauri::async_runtime::spawn(async move {
        let mut previous = ActivityCounts::default();
        loop {
            if let Ok(counts) = client.get_activity().await
                && counts != previous
            {
                apply_counts(&app, counts);
                previous = counts;
            }
            sleep(ACTIVITY_POLL_INTERVAL).await;
        }
    });
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn running_tasks_show_in_tray_title() {
        assert_eq!(tray_title(ActivityCounts { running: 4 }), "4");
    }

    #[test]
    fn empty_counts_have_no_badge_text() {
        let counts = ActivityCounts::default();
        assert_eq!(tray_title(counts), "");
        assert_eq!(accessibility_label(counts), "Oga");
    }
}
