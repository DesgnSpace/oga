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
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};

use crate::AppState;

const POINTER_QUEUE_CAPACITY: usize = 1_024;
const MAX_BATCH: usize = 100;
const BATCH_INTERVAL: Duration = Duration::from_millis(16);

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
        let (sender, mut receiver) = mpsc::channel(POINTER_QUEUE_CAPACITY);
        let overflow_cursor = Arc::new(AtomicI64::new(0));
        let sender_overflow = overflow_cursor.clone();
        let batch_app = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut batch = Vec::with_capacity(MAX_BATCH);
            let mut cursor = 0;
            let mut flush_deadline = None;
            loop {
                if let Some(overflow_cursor) = take_overflow(&overflow_cursor) {
                    while receiver.try_recv().is_ok() {}
                    let stale_cursor = cursor.max(overflow_cursor);
                    emit_batch(&batch_app, stale_cursor, true, Vec::new());
                    batch.clear();
                    cursor = stale_cursor;
                    flush_deadline = None;
                    continue;
                }
                let item = match flush_deadline {
                    None => receiver.recv().await,
                    Some(deadline) => {
                        match tokio::time::timeout_at(deadline, receiver.recv()).await {
                            Ok(item) => item,
                            Err(_) => {
                                emit_batch(&batch_app, cursor, false, std::mem::take(&mut batch));
                                flush_deadline = None;
                                tokio::time::sleep(BATCH_INTERVAL).await;
                                continue;
                            }
                        }
                    }
                };
                let Some((pointer, next_cursor)) = item else {
                    if !batch.is_empty() {
                        emit_batch(&batch_app, cursor, false, std::mem::take(&mut batch));
                        tokio::time::sleep(Duration::from_millis(16)).await;
                    }
                    break;
                };
                if batch.is_empty() {
                    flush_deadline = Some(Instant::now() + BATCH_INTERVAL);
                }
                batch.push(pointer);
                cursor = cursor.max(next_cursor);
                if batch.len() < MAX_BATCH {
                    continue;
                }
                emit_batch(&batch_app, cursor, false, std::mem::take(&mut batch));
                flush_deadline = None;
                tokio::time::sleep(BATCH_INTERVAL).await;
            }
        });
        pump.run(
            move |frame| {
                if let EventFrame::Task(pointer) = frame {
                    follower.note(pointer);
                    let cursor = pointer.cursor;
                    if matches!(
                        sender.try_send((pointer.clone(), cursor)),
                        Err(mpsc::error::TrySendError::Full(_))
                    ) {
                        record_overflow(&sender_overflow, cursor);
                    }
                }
            },
            move |status| {
                let _ = status_app.emit(STATUS_EVENT, status);
            },
        )
        .await
    });
}

fn record_overflow(cursor: &AtomicI64, value: i64) {
    let mut current = cursor.load(Ordering::Acquire);
    while current < value {
        match cursor.compare_exchange_weak(current, value, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
    }
}

fn take_overflow(cursor: &AtomicI64) -> Option<i64> {
    let cursor = cursor.swap(0, Ordering::AcqRel);
    (cursor > 0).then_some(cursor)
}

fn emit_batch<R: Runtime>(
    app: &AppHandle<R>,
    cursor: i64,
    stale: bool,
    pointers: Vec<oga_domain::EventPointer>,
) {
    let _ = app.emit(
        EVENT_BATCH_EVENT,
        oga_client::bridge::EventBatch {
            cursor,
            stream_floor: 0,
            stale,
            pointers,
        },
    );
}
