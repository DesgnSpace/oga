//! The contract the desktop shell speaks on the web view's behalf.
//!
//! The web view holds no socket to the broker. It sends a [`BrokerCall`] to the
//! shell and gets JSON back; it sees the broker's stream as [`EventBatch`]
//! values the shell pumps out of the one connection it owns; and for the task
//! it is showing it gets a [`TaskDelta`], because working out what changed is
//! the shell's job rather than the web view's.

use oga_domain::{CleanupSettings, EventPointer, Task, TaskEventView, WaitSettings};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    CompletionRequest, HandoffRequest, ModelSettingsUpdate, ProfileCreate, ProfilePatch,
    PromptWrite, ReplyRequest, ResumeRequest, StateQuery, SteerRequest, TaskEventsQuery,
};

/// One read or write the web view asks the shell to make.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "call",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BrokerCall {
    Health,
    Summary {
        query: StateQuery,
    },
    Usage {
        tz_offset: Option<i32>,
    },
    Task {
        task_id: String,
    },
    TaskEvents {
        task_id: String,
        query: TaskEventsQuery,
    },
    TaskDiff {
        task_id: String,
    },
    ConsumerInbox {
        consumer_id: String,
        channel: Option<String>,
        limit: Option<u64>,
    },
    Projects,
    Memories {
        cwd: String,
    },
    Prompt {
        cwd: Option<String>,
    },
    ModelSettings {
        cwd: Option<String>,
        refresh: Option<bool>,
    },
    Cleanup,
    PutCleanup {
        settings: CleanupSettings,
    },
    RunCleanup,
    Waiting,
    PutWaiting {
        settings: WaitSettings,
    },
    ArchiveTask {
        task_id: String,
        archived: bool,
        #[serde(default)]
        delete_branch: bool,
    },
    CancelTask {
        task_id: String,
    },
    ResumeTask {
        task_id: String,
        request: ResumeRequest,
    },
    ReplyTask {
        task_id: String,
        request: ReplyRequest,
    },
    SteerTask {
        task_id: String,
        request: SteerRequest,
    },
    HandoffTask {
        task_id: String,
        request: HandoffRequest,
    },
    CompleteTask {
        task_id: String,
        request: CompletionRequest,
    },
    RemoveFollowUp {
        task_id: String,
        index: usize,
    },
    PutPrompt {
        request: PromptWrite,
    },
    PutModelSettings {
        request: ModelSettingsUpdate,
    },
    ResetModelSettings {
        cwd: Option<String>,
        revision: Option<String>,
    },
    CreateProfile {
        profile: ProfileCreate,
    },
    UpdateProfile {
        profile_id: String,
        patch: ProfilePatch,
    },
    DeleteProfile {
        profile_id: String,
    },
}

/// Why a call failed, in the shape the web view can act on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeError {
    pub message: String,
    /// The broker's HTTP status, when the broker itself answered with one.
    pub status: Option<u16>,
}

impl BridgeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
        }
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BridgeError {}

/// One paced batch of changes from the broker's global event stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventBatch {
    pub cursor: i64,
    pub stream_floor: i64,
    pub stale: bool,
    pub pointers: Vec<EventPointer>,
}

/// Emitted once per paced batch from the global event stream.
pub const EVENT_BATCH_EVENT: &str = "oga-broker-batch";
/// Emitted whenever the shell's broker connection changes state.
pub const STATUS_EVENT: &str = "oga-broker-status";
/// Emitted whenever the task the web view is watching moves on.
pub const TASK_DELTA_EVENT: &str = "oga-task-delta";

/// Everything the web view needs to show one task, read in a single call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub task: Task,
    pub events: Vec<TaskEventView>,
    pub cursor: i64,
    pub oldest_id: Option<i64>,
    pub has_earlier: bool,
}

/// What moved on a watched task, and nothing else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDelta {
    pub task_id: String,
    /// The cursor this continues from. A reader holding a different cursor
    /// has missed something and has to ask for the task again.
    pub from_cursor: i64,
    pub cursor: i64,
    /// Carried only when the task row itself differs from the one last sent.
    pub task: Option<Task>,
    /// Only the activity the reader does not already hold.
    pub events: Vec<TaskEventView>,
}

/// One broker stream frame, forwarded to the web view exactly as it arrived.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamFrame {
    pub event: String,
    pub data: Value,
}

/// What the shell's single broker connection is doing right now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamStatus {
    pub connected: bool,
    pub cursor: i64,
    /// The oldest event the broker still retains, so a view can tell whether
    /// its cursor fell off the end of the log.
    pub stream_floor: i64,
    /// Set when the requested cursor was older than the retained history.
    pub stale: bool,
    pub error: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{StreamPump, TaskFollower};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use oga_domain::{EventPointer, Task};
    use serde::Serialize;
    use serde_json::Value;
    use tokio::sync::Notify;

    use super::{BridgeError, BrokerCall, StreamStatus, TaskDelta, TaskSnapshot};
    use crate::{
        ClientError, EventFrame, EventStreamOptions, EventStreamQuery, LoopbackClient,
        TaskEventsQuery,
    };

    impl From<&ClientError> for BridgeError {
        fn from(error: &ClientError) -> Self {
            Self {
                message: error.to_string(),
                status: error.status().map(|status| status.as_u16()),
            }
        }
    }

    fn encode<T: Serialize>(result: Result<T, ClientError>) -> Result<Value, BridgeError> {
        let value = result.map_err(|error| BridgeError::from(&error))?;
        serde_json::to_value(value).map_err(|error| {
            BridgeError::new(format!("could not encode the broker reply: {error}"))
        })
    }

    impl LoopbackClient {
        /// Answers one [`BrokerCall`] from the broker.
        pub async fn call(&self, call: BrokerCall) -> Result<Value, BridgeError> {
            match call {
                BrokerCall::Health => encode(self.health().await),
                BrokerCall::Summary { query } => encode(self.get_summary(&query).await),
                BrokerCall::Usage { tz_offset } => encode(self.spend_usage(tz_offset).await),
                BrokerCall::Task { task_id } => encode(self.get_task(&task_id).await),
                BrokerCall::TaskEvents { task_id, query } => {
                    encode(self.get_task_events(&task_id, &query).await)
                }
                BrokerCall::TaskDiff { task_id } => encode(self.get_task_diff(&task_id).await),
                BrokerCall::ConsumerInbox {
                    consumer_id,
                    channel,
                    limit,
                } => encode(
                    self.consumer_inbox(&consumer_id, channel.as_deref(), limit)
                        .await,
                ),
                BrokerCall::Projects => encode(self.projects().await),
                BrokerCall::Memories { cwd } => encode(self.memories(&cwd).await),
                BrokerCall::Prompt { cwd } => encode(self.prompt(cwd.as_deref()).await),
                BrokerCall::ModelSettings { cwd, refresh } => encode(
                    self.model_settings(cwd.as_deref(), refresh == Some(true))
                        .await,
                ),
                BrokerCall::Cleanup => encode(self.cleanup().await),
                BrokerCall::PutCleanup { settings } => encode(self.put_cleanup(&settings).await),
                BrokerCall::RunCleanup => encode(self.run_cleanup().await),
                BrokerCall::Waiting => encode(self.waiting().await),
                BrokerCall::PutWaiting { settings } => encode(self.put_waiting(&settings).await),
                BrokerCall::ArchiveTask {
                    task_id,
                    archived,
                    delete_branch,
                } => encode(
                    self.archive_task_with_branch_deletion(&task_id, archived, delete_branch)
                        .await,
                ),
                BrokerCall::CancelTask { task_id } => {
                    encode(self.cancel_task(&task_id, None).await)
                }
                BrokerCall::ResumeTask { task_id, request } => {
                    encode(self.resume_task(&task_id, &request).await)
                }
                BrokerCall::ReplyTask { task_id, request } => {
                    encode(self.reply_task(&task_id, &request).await)
                }
                BrokerCall::SteerTask { task_id, request } => {
                    encode(self.steer_task(&task_id, &request).await)
                }
                BrokerCall::HandoffTask { task_id, request } => {
                    encode(self.handoff_task(&task_id, &request).await)
                }
                BrokerCall::CompleteTask { task_id, request } => {
                    encode(self.complete_task(&task_id, &request).await)
                }
                BrokerCall::RemoveFollowUp { task_id, index } => {
                    encode(self.remove_follow_up(&task_id, index).await)
                }
                BrokerCall::PutPrompt { request } => encode(self.put_prompt(&request).await),
                BrokerCall::PutModelSettings { request } => {
                    encode(self.put_model_settings(&request).await)
                }
                BrokerCall::ResetModelSettings { cwd, revision } => encode(
                    self.delete_model_settings(cwd.as_deref(), revision.as_deref())
                        .await,
                ),
                BrokerCall::CreateProfile { profile } => {
                    encode(self.create_profile(&profile).await)
                }
                BrokerCall::UpdateProfile { profile_id, patch } => {
                    encode(self.update_profile(&profile_id, &patch).await)
                }
                BrokerCall::DeleteProfile { profile_id } => {
                    encode(self.delete_profile(&profile_id).await)
                }
            }
        }
    }

    /// The shell's single consumer of the broker event stream.
    ///
    /// It holds the cursor across reconnects, so a stream that drops resumes
    /// from the last frame it delivered rather than replaying from the start or
    /// skipping what arrived while it was away.
    #[derive(Clone)]
    pub struct StreamPump {
        client: LoopbackClient,
        options: EventStreamOptions,
        status: Arc<Mutex<StreamStatus>>,
    }

    impl StreamPump {
        pub fn new(client: LoopbackClient, options: EventStreamOptions) -> Self {
            Self {
                client,
                options,
                status: Arc::new(Mutex::new(StreamStatus::default())),
            }
        }

        pub fn status(&self) -> StreamStatus {
            self.status.lock().expect("stream status lock").clone()
        }

        /// Consumes the broker stream forever, handing each frame and every
        /// change of connection to the shell to forward.
        ///
        /// The cursor moves with every frame and nothing downstream reads it,
        /// so a status only goes out when what a view reacts to has changed.
        pub async fn run(
            &self,
            mut on_frame: impl FnMut(&EventFrame),
            mut on_status: impl FnMut(StreamStatus),
        ) -> ! {
            let mut announced: Option<StreamStatus> = None;
            let mut announce =
                move |status: StreamStatus, on_status: &mut dyn FnMut(StreamStatus)| {
                    if announced.as_ref().map(announcement) == Some(announcement(&status)) {
                        return;
                    }
                    announced = Some(status.clone());
                    on_status(status);
                };
            let mut cold_start = true;
            loop {
                let after = if cold_start {
                    cold_start = false;
                    let cursor = self.client.event_head().await.unwrap_or(0);
                    self.update(|status| status.cursor = cursor);
                    cursor
                } else {
                    self.status().cursor
                };
                match self
                    .client
                    .event_stream_with_options(EventStreamQuery::new(after), self.options)
                    .await
                {
                    Ok(mut stream) => loop {
                        match stream.next().await {
                            Ok(frame) => {
                                announce(self.record(&frame), &mut on_status);
                                on_frame(&frame);
                            }
                            Err(error) => {
                                announce(self.disconnect(&error), &mut on_status);
                                break;
                            }
                        }
                    },
                    Err(error) => announce(self.disconnect(&error), &mut on_status),
                }
            }
        }

        fn record(&self, frame: &EventFrame) -> StreamStatus {
            self.update(|status| {
                status.connected = true;
                status.error = None;
                if let Some(cursor) = frame.cursor() {
                    status.cursor = status.cursor.max(cursor);
                }
                if let EventFrame::Ready(ready) = frame {
                    status.stream_floor = ready.stream_floor;
                    status.stale = ready.stale;
                }
            })
        }

        fn disconnect(&self, error: &ClientError) -> StreamStatus {
            self.update(|status| {
                status.connected = false;
                status.error = Some(error.to_string());
            })
        }

        fn update(&self, update: impl FnOnce(&mut StreamStatus)) -> StreamStatus {
            let mut status = self.status.lock().expect("stream status lock");
            update(&mut status);
            status.clone()
        }
    }

    /// The part of a status a view reacts to.
    fn announcement(status: &StreamStatus) -> (bool, Option<&str>, i64, bool) {
        (
            status.connected,
            status.error.as_deref(),
            status.stream_floor,
            status.stale,
        )
    }

    /// How much activity one delta carries before it is split across two.
    const DELTA_PAGE_SIZE: u64 = 150;
    /// How long the broker holds a read open waiting for the task to move.
    const WAIT_MS: u64 = 25_000;
    /// How often a task that cannot move is looked at anyway, in case it was
    /// resumed.
    const SETTLED_INTERVAL: Duration = Duration::from_secs(5);

    /// The task the web view is looking at, followed on the shell's side.
    ///
    /// The shell already holds the broker connection, so it is the side that
    /// can turn a pointer frame into the activity behind it. It keeps the
    /// task's cursor and its last known row, and hands the web view only the
    /// difference. Frames that arrive while a read is in flight collapse into
    /// the next one, so a burst costs one read rather than one read each.
    ///
    /// It also waits on the task's own log rather than only on the shell's
    /// stream. The stream carries every task from wherever its cursor happens
    /// to be, so a shell working through a backlog would otherwise leave the
    /// task on screen frozen until it caught up.
    #[derive(Clone)]
    pub struct TaskFollower {
        client: LoopbackClient,
        watched: Arc<Mutex<Option<Watched>>>,
        moved: Arc<Notify>,
    }

    struct Watched {
        task_id: String,
        cursor: i64,
        task: Task,
    }

    impl TaskFollower {
        pub fn new(client: LoopbackClient) -> Self {
            Self {
                client,
                watched: Arc::new(Mutex::new(None)),
                moved: Arc::new(Notify::new()),
            }
        }

        /// Starts following `task_id` and answers with everything the web view
        /// needs to draw it. Calling it again resynchronises.
        pub async fn watch(&self, task_id: &str, events: u64) -> Result<TaskSnapshot, BridgeError> {
            self.client
                .mark_task_viewed(task_id)
                .await
                .map_err(|error| BridgeError::from(&error))?;
            let query = TaskEventsQuery::default().last(events).limit(events);
            let (task_result, page_result) = tokio::join!(
                self.client.get_task(task_id),
                self.client.get_task_events(task_id, &query),
            );
            let task = task_result.map_err(|error| BridgeError::from(&error))?;
            let page = page_result.map_err(|error| BridgeError::from(&error))?;
            let cursor = page.cursor.unwrap_or_default();
            *self.watched.lock().expect("watched task lock") = Some(Watched {
                task_id: task_id.to_owned(),
                cursor,
                task: task.clone(),
            });
            Ok(TaskSnapshot {
                task,
                events: page.events,
                cursor,
                oldest_id: page.oldest_id,
                has_earlier: page.has_earlier.unwrap_or(false),
            })
        }

        /// Stops following `task_id`, so its frames cost nothing again.
        pub fn unwatch(&self, task_id: &str) {
            let mut watched = self.watched.lock().expect("watched task lock");
            if watched
                .as_ref()
                .is_some_and(|watched| watched.task_id == task_id)
            {
                *watched = None;
            }
            let client = self.client.clone();
            let task_id = task_id.to_owned();
            tokio::spawn(async move {
                let _ = client.unmark_task_viewed(&task_id).await;
            });
        }

        /// The task the web view has open, if any.
        pub fn watching(&self) -> Option<String> {
            self.watched
                .lock()
                .expect("watched task lock")
                .as_ref()
                .map(|watched| watched.task_id.clone())
        }

        /// Wakes the follower when this pointer is for the task being watched.
        pub fn note(&self, pointer: &EventPointer) {
            let watching = self
                .watched
                .lock()
                .expect("watched task lock")
                .as_ref()
                .is_some_and(|watched| watched.task_id == pointer.task_id);
            if watching {
                self.moved.notify_one();
            }
        }

        /// Waits for the watched task to move, then reads what changed.
        ///
        /// Answers `None` when the read found nothing new, the task was
        /// released while the read was in flight, or the broker refused; the
        /// cursor stays put in every case, so the next wake retries.
        pub async fn next_delta(&self) -> Option<TaskDelta> {
            tokio::select! {
                // A frame already in hand means there is nothing to hold a
                // read open for, so this branch is tried first.
                biased;
                () = self.moved.notified() => {}
                () = self.waiting() => {}
            }
            self.pull().await
        }

        /// Holds until the broker says the watched task's log has moved.
        async fn waiting(&self) {
            let Some((task_id, from_cursor, settled, _)) = self.watched() else {
                return std::future::pending().await;
            };
            // The broker only holds a read open for a task that can still
            // write, and only past a cursor it can compare against.
            if settled || from_cursor <= 0 {
                tokio::time::sleep(SETTLED_INTERVAL).await;
                return;
            }
            let _ = self
                .client
                .get_task_events(
                    &task_id,
                    &TaskEventsQuery::default()
                        .after(from_cursor)
                        .limit(1)
                        .wait_ms(WAIT_MS),
                )
                .await;
        }

        fn watched(&self) -> Option<(String, i64, bool, String)> {
            let watched = self.watched.lock().expect("watched task lock");
            let watched = watched.as_ref()?;
            Some((
                watched.task_id.clone(),
                watched.cursor,
                watched.task.state.settled(),
                watched.task.updated_at.clone(),
            ))
        }

        async fn pull(&self) -> Option<TaskDelta> {
            let (task_id, from_cursor, _, task_updated_at) = self.watched()?;

            let mut events = Vec::new();
            let mut cursor = from_cursor;
            let mut returned_task = None;
            let mut returned_task_updated_at = None;
            loop {
                let page = self
                    .client
                    .get_task_events(
                        &task_id,
                        &TaskEventsQuery::default()
                            .after(cursor)
                            .limit(DELTA_PAGE_SIZE)
                            .include_task(true)
                            .task_updated_at(task_updated_at.clone()),
                    )
                    .await
                    .ok()?;
                if page.task_updated_at.is_some() {
                    returned_task_updated_at = page.task_updated_at;
                    if page.task.is_some() {
                        returned_task = page.task;
                    }
                }
                let has_more = page.has_more == Some(true) && !page.events.is_empty();
                if let Some(next) = page.cursor {
                    cursor = cursor.max(next);
                }
                events.extend(page.events);
                if !has_more {
                    break;
                }
            }
            let task = match returned_task {
                Some(task) => task,
                None if returned_task_updated_at.as_deref() == Some(task_updated_at.as_str()) => {
                    self.watched
                        .lock()
                        .expect("watched task lock")
                        .as_ref()
                        .filter(|watched| watched.task_id == task_id)
                        .map(|watched| watched.task.clone())?
                }
                None => self.client.get_task(&task_id).await.ok()?,
            };

            let mut watched = self.watched.lock().expect("watched task lock");
            let watched = watched
                .as_mut()
                .filter(|watched| watched.task_id == task_id && watched.cursor == from_cursor)?;
            let task_moved = watched.task != task;
            watched.cursor = cursor;
            if task_moved {
                watched.task = task.clone();
            }
            if events.is_empty() && !task_moved {
                return None;
            }
            Some(TaskDelta {
                task_id,
                from_cursor,
                cursor,
                task: task_moved.then_some(task),
                events,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_round_trip_through_json() {
        let call = BrokerCall::ResumeTask {
            task_id: "task".to_owned(),
            request: ResumeRequest {
                instruction: Some("keep going".to_owned()),
                ..ResumeRequest::default()
            },
        };

        let encoded = serde_json::to_value(&call).expect("encode call");
        assert_eq!(encoded["call"], "resumeTask");
        assert_eq!(
            serde_json::from_value::<BrokerCall>(encoded).expect("decode call"),
            call
        );
    }

    #[test]
    fn handoff_calls_round_trip_with_a_destination() {
        let call = BrokerCall::HandoffTask {
            task_id: "task".to_owned(),
            request: HandoffRequest {
                profile: Some("worker-2".to_owned()),
                model: Some("sonnet".to_owned()),
                ..HandoffRequest::default()
            },
        };
        let encoded = serde_json::to_value(&call).expect("encode call");
        assert_eq!(encoded["call"], "handoffTask");
        assert_eq!(encoded["taskId"], "task");
        assert_eq!(encoded["request"]["profile"], "worker-2");
        assert_eq!(
            serde_json::from_value::<BrokerCall>(encoded).expect("decode call"),
            call
        );
    }

    #[test]
    fn queries_survive_the_trip_with_their_defaults() {
        let call = BrokerCall::TaskEvents {
            task_id: "task".to_owned(),
            query: TaskEventsQuery::default().last(150).limit(150),
        };

        let encoded = serde_json::to_value(&call).expect("encode call");

        assert_eq!(
            serde_json::from_value::<BrokerCall>(encoded).expect("decode call"),
            call
        );
    }

    #[test]
    fn calls_name_their_fields_the_way_the_web_view_writes_them() {
        let encoded = serde_json::to_value(BrokerCall::TaskEvents {
            task_id: "task".to_owned(),
            query: TaskEventsQuery::default().wait_ms(50),
        })
        .expect("encode call");

        assert_eq!(encoded["taskId"], "task");
        assert_eq!(encoded["query"]["waitMs"], 50);

        let inbox = serde_json::to_value(BrokerCall::ConsumerInbox {
            consumer_id: "consumer".to_owned(),
            channel: None,
            limit: None,
        })
        .expect("encode call");

        assert_eq!(inbox["consumerId"], "consumer");
    }
}
