//! The web view's only route to the broker.
//!
//! The shell owns the HTTP client and the one event stream. Reads and writes
//! arrive as commands; live updates go back out as window events.

use oga_client::{
    EventFrame,
    bridge::{
        BridgeError, BrokerCall, EVENT_BATCH_EVENT, STATUS_EVENT, StreamPump, StreamStatus,
        TASK_DELTA_EVENT, TaskFollower, TaskSnapshot,
    },
};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::mpsc;
use tokio::time::Duration;

use crate::AppState;

#[tauri::command]
pub async fn broker_call(
    state: tauri::State<'_, AppState>,
    call: BrokerCall,
) -> Result<Value, BridgeError> {
    state.client.call(call).await
}

#[tauri::command]
pub async fn broker_stream_status(
    state: tauri::State<'_, AppState>,
) -> Result<StreamStatus, String> {
    let mut status = state.stream.status();
    if state.client.health().await.is_ok() {
        status.connected = true;
        status.error = None;
    }
    Ok(status)
}

#[tauri::command]
pub async fn broker_watch_task(
    state: tauri::State<'_, AppState>,
    task_id: String,
    events: u64,
) -> Result<TaskSnapshot, BridgeError> {
    state.follower.watch(&task_id, events).await
}

#[tauri::command]
pub async fn broker_unwatch_task(
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<(), BridgeError> {
    state.follower.unwatch(&task_id);
    Ok(())
}

/// Starts the shell's single consumer of the broker event stream, and the
/// reader that turns its frames into updates for the task on screen.
pub fn start<R: Runtime>(app: AppHandle<R>, pump: StreamPump, follower: TaskFollower) {
    let delta_app = app.clone();
    let deltas = follower.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            if let Some(delta) = deltas.next_delta().await {
                let _ = delta_app.emit(TASK_DELTA_EVENT, delta);
            }
        }
    });
    tauri::async_runtime::spawn(async move {
        let status_app = app.clone();
        let (sender, mut receiver) = mpsc::unbounded_channel::<(oga_domain::EventPointer, i64)>();
        let batch_app = app.clone();
        tauri::async_runtime::spawn(async move {
            const MAX_BATCH: usize = 100;
            const BATCH_INTERVAL: Duration = Duration::from_millis(16);
            let mut batch = Vec::with_capacity(MAX_BATCH);
            let mut cursor = 0;
            loop {
                let item = if batch.is_empty() {
                    Some(receiver.recv().await)
                } else {
                    match tokio::time::timeout(BATCH_INTERVAL, receiver.recv()).await {
                        Ok(item) => Some(item),
                        Err(_) => {
                            let _ = batch_app.emit(
                                EVENT_BATCH_EVENT,
                                oga_client::bridge::EventBatch {
                                    cursor,
                                    stream_floor: 0,
                                    stale: false,
                                    pointers: std::mem::take(&mut batch),
                                },
                            );
                            tokio::time::sleep(BATCH_INTERVAL).await;
                            continue;
                        }
                    }
                };
                let Some((pointer, next_cursor)) = item.flatten() else {
                    if !batch.is_empty() {
                        let _ = batch_app.emit(
                            EVENT_BATCH_EVENT,
                            oga_client::bridge::EventBatch {
                                cursor,
                                stream_floor: 0,
                                stale: false,
                                pointers: std::mem::take(&mut batch),
                            },
                        );
                        tokio::time::sleep(Duration::from_millis(16)).await;
                    }
                    break;
                };
                batch.push(pointer);
                cursor = cursor.max(next_cursor);
                if batch.len() < MAX_BATCH {
                    continue;
                }
                let _ = batch_app.emit(
                    EVENT_BATCH_EVENT,
                    oga_client::bridge::EventBatch {
                        cursor,
                        stream_floor: 0,
                        stale: false,
                        pointers: std::mem::take(&mut batch),
                    },
                );
                tokio::time::sleep(BATCH_INTERVAL).await;
            }
        });
        pump.run(
            move |frame| {
                if let EventFrame::Task(pointer) = frame {
                    follower.note(pointer);
                    let cursor = pointer.cursor;
                    let _ = sender.send((pointer.clone(), cursor));
                }
            },
            move |status| {
                let _ = status_app.emit(STATUS_EVENT, status);
            },
        )
        .await
    });
}
