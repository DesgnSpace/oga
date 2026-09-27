mod bridge;
mod broker;
mod commands;
mod integrations;
mod lifecycle;
mod notifications;
mod tray;

use std::{error::Error, sync::Arc};

use broker::{BrokerSnapshot, BrokerSupervisor};

const BROKER_WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
use integrations::install_mcp_configs;
use notifications::Notifier;
use oga_client::{
    EventStreamOptions, LoopbackClient,
    bridge::{StreamPump, TaskFollower},
};
use tauri::Manager;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Mutex;
use tokio::sync::RwLock;

fn open_url<R: tauri::Runtime>(app: &tauri::AppHandle<R>, url: &str) {
    if let Err(error) = app.opener().open_url(url, None::<&str>) {
        eprintln!("could not open {url}: {error}");
    }
}

#[tauri::command]
fn open_external_link<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    url: String,
) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:")) {
        return Err("Only web and email links can open outside Oga.".to_string());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|error| format!("Could not open the link: {error}"))
}

#[derive(Debug, serde::Serialize)]
struct ImagePreview {
    bytes: Vec<u8>,
    mime: String,
}

#[tauri::command]
fn read_image_preview(path: String) -> Result<ImagePreview, String> {
    let bytes = std::fs::read(&path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => "Image preview unavailable: file not found.".to_string(),
        std::io::ErrorKind::PermissionDenied => {
            "Image preview unavailable: file is unreadable.".to_string()
        }
        _ => format!("Image preview unavailable: file is unreadable ({error})."),
    })?;
    let mime = oga_domain::image_mime(&bytes).ok_or_else(|| {
        "Image preview unavailable: unsupported image type or invalid image data.".to_string()
    })?;
    Ok(ImagePreview {
        bytes,
        mime: mime.to_owned(),
    })
}

/// Read through the broker, which serves only the files attached to the task.
#[tauri::command]
async fn read_task_attachment(
    state: tauri::State<'_, AppState>,
    task_id: String,
    index: usize,
) -> Result<ImagePreview, String> {
    let (bytes, mime) = state
        .client
        .get_task_attachment(&task_id, index)
        .await
        .map_err(|error| {
            if error.status().is_some_and(|status| status.as_u16() == 404) {
                "Image preview unavailable: file not found.".to_string()
            } else {
                "Image preview unavailable: file is unreadable.".to_string()
            }
        })?;
    if !mime.starts_with("image/") {
        return Err(
            "Image preview unavailable: unsupported image type or invalid image data.".to_string(),
        );
    }
    Ok(ImagePreview {
        bytes: bytes.to_vec(),
        mime,
    })
}

/// The web view names the attachment by its place in the task's list, never
/// by path, so it cannot open arbitrary files.
#[tauri::command]
async fn open_attachment<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, AppState>,
    task_id: String,
    index: usize,
) -> Result<(), String> {
    let task = state
        .client
        .get_task(&task_id)
        .await
        .map_err(|_| "Could not open the file: the task is unavailable.".to_string())?;
    let path = task
        .attachment_path(index)
        .ok_or_else(|| "Could not open the file: it is not attached to this task.".to_string())?;
    app.opener()
        .open_path(path.display().to_string(), None::<&str>)
        .map_err(|error| format!("Could not open the file: {error}"))
}

#[derive(Clone)]
struct AppState {
    broker: Arc<Mutex<Option<BrokerSupervisor>>>,
    broker_snapshot: Arc<RwLock<BrokerSnapshot>>,
    client: LoopbackClient,
    stream: StreamPump,
    follower: TaskFollower,
}

#[tauri::command]
async fn broker_status(state: tauri::State<'_, AppState>) -> Result<BrokerSnapshot, String> {
    Ok(state.broker_snapshot.read().await.clone())
}

#[tauri::command]
async fn ensure_broker(state: tauri::State<'_, AppState>) -> Result<BrokerSnapshot, String> {
    ensure_broker_inner(&state).await
}

async fn ensure_broker_inner(state: &AppState) -> Result<BrokerSnapshot, String> {
    let Some(mut broker) = state.broker.lock().await.take() else {
        return Ok(state.broker_snapshot.read().await.clone());
    };
    let result = broker.connect_or_start().await;
    let snapshot = broker.snapshot();
    *state.broker_snapshot.write().await = snapshot.clone();
    *state.broker.lock().await = Some(broker);
    result.map(|_| snapshot).map_err(|error| error.to_string())
}

#[tauri::command]
async fn stop_broker(state: tauri::State<'_, AppState>) -> Result<BrokerSnapshot, String> {
    let Some(mut broker) = state.broker.lock().await.take() else {
        return Ok(state.broker_snapshot.read().await.clone());
    };
    let result = broker.shutdown().map_err(|error| error.to_string());
    let snapshot = broker.snapshot();
    *state.broker_snapshot.write().await = snapshot.clone();
    *state.broker.lock().await = Some(broker);
    result?;
    Ok(snapshot)
}

fn main() -> Result<(), Box<dyn Error>> {
    let app = tauri::Builder::default()
        .plugin(lifecycle::window_state_plugin())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .menu(commands::app_menu)
        .on_menu_event(commands::handle_menu_event)
        .on_tray_icon_event(tray::handle_event)
        .on_window_event(lifecycle::handle_window_event)
        .setup(|app| {
            lifecycle::configure_window(app)?;
            let resource_directory = app.path().resource_dir().ok();
            let supervisor = BrokerSupervisor::new(resource_directory.as_deref())?;
            if let Ok(home) = app.path().home_dir() {
                match supervisor.link_cli(&home) {
                    Ok(Some(link)) => eprintln!("linked {}", link.display()),
                    Ok(None) => {}
                    Err(error) => eprintln!("could not link the oga command: {error}"),
                }
            }
            let client = LoopbackClient::from_env()?;
            let state = AppState {
                broker_snapshot: Arc::new(RwLock::new(supervisor.snapshot())),
                broker: Arc::new(Mutex::new(Some(supervisor))),
                client: client.clone(),
                stream: StreamPump::new(client.clone(), EventStreamOptions::default()),
                follower: TaskFollower::new(client.clone()),
            };
            app.manage(state.clone());
            tray::setup(app.handle())?;
            tray::spawn_activity_poll(app.handle().clone(), client);
            let notifier = Notifier::new(app.handle().clone(), state.follower.clone());
            notifier.start();
            app.manage(notifier.clone());
            bridge::start(
                app.handle().clone(),
                state.stream.clone(),
                state.follower.clone(),
                notifier,
            );

            // The supervisor must reconnect after a broker dies.
            tauri::async_runtime::spawn(async move {
                loop {
                    if let Err(error) = ensure_broker_inner(&state).await {
                        eprintln!("broker unavailable: {error}");
                    }
                    tokio::time::sleep(BROKER_WATCH_INTERVAL).await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            broker_status,
            ensure_broker,
            stop_broker,
            install_mcp_configs,
            bridge::broker_call,
            bridge::broker_stream_status,
            bridge::broker_watch_task,
            bridge::broker_unwatch_task,
            read_image_preview,
            read_task_attachment,
            open_attachment,
            notifications::set_task_notifications,
            commands::set_menu_item_enabled,
            open_external_link
        ])
        .build(tauri::generate_context!())?;
    app.run(|app, event| match event {
        tauri::RunEvent::ExitRequested { .. } => lifecycle::handle_exit(app),
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => lifecycle::show_main_window(app),
        _ => {}
    });
    Ok(())
}
