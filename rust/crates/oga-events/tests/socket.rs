use std::{collections::BTreeMap, sync::Arc, time::Duration};

use oga_domain::{Profile, Provider, Task, TaskEvent, TaskKind, TaskScope, TaskState};
use oga_events::{EventSocketOptions, event_socket_path, start_event_socket};
use oga_store::Store;
use rusqlite::params;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    net::UnixStream,
    time::timeout,
};

struct Fixture {
    directory: TempDir,
    store: Arc<Store>,
    task: Task,
}

impl Fixture {
    fn new(state: TaskState) -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = directory.path().join("oga.db");
        let store = Arc::new(Store::open_writable(database).expect("store"));
        store
            .repositories()
            .profiles()
            .insert(
                &Profile {
                    id: "profile".into(),
                    label: "Test Profile".into(),
                    provider: Provider::Antigravity,
                    default_model: "fake".into(),
                    enabled: true,
                    env: BTreeMap::new(),
                    capabilities: Vec::new(),
                    command: None,
                },
                "2026-01-01T00:00:00.000Z",
            )
            .expect("profile");
        let task = Task {
            id: "task".into(),
            kind: Some(TaskKind::Delegated),
            profile_id: "profile".into(),
            model: "fake".into(),
            prompt: "watch the fixture".into(),
            cwd: directory.path().display().to_string(),
            state,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            output: String::new(),
            scope: TaskScope {
                read: vec!["**".into()],
                write: vec!["**".into()],
            },
            allow_questions: true,
            can_delegate: false,
            ..Task::default()
        };
        store.repositories().tasks().insert(&task).expect("task");
        Self {
            directory,
            store,
            task,
        }
    }

    fn append(&self, kind: &str, state: TaskState, payload: Value) -> i64 {
        self.store
            .repositories()
            .events()
            .append(&TaskEvent {
                id: 0,
                task_id: self.task.id.clone(),
                kind: kind.into(),
                state,
                payload: payload
                    .as_object()
                    .expect("object payload")
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
                created_at: "2026-01-01T00:00:01.000Z".into(),
                turn_id: None,
            })
            .expect("event")
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.directory.path().join(name)
    }

    fn update_task(&self, state: TaskState, title: &str, tldr: Option<&str>) {
        self.store
            .with_transaction(|connection| {
                connection.execute(
                    "UPDATE tasks SET state=?,title=?,tldr=? WHERE id=?",
                    params![state.as_str(), title, tldr, self.task.id],
                )?;
                Ok(())
            })
            .expect("task update");
    }
}

fn options(path: std::path::PathBuf, keepalive: Duration) -> EventSocketOptions {
    EventSocketOptions::new(
        path,
        oga_domain::HelloPayload {
            version: "test".into(),
            mcp_contract_version: 32,
            initial_cursor: None,
            stream_floor: None,
            stale: None,
        },
    )
    .with_keepalive(keepalive)
    .with_poll_interval(Duration::from_millis(5))
}

async fn next_json<R>(reader: &mut BufReader<R>) -> Value
where
    R: AsyncRead + Unpin,
{
    let mut line = String::new();
    let bytes = timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("socket read timeout")
        .expect("socket read");
    assert!(bytes > 0, "socket closed before a frame");
    serde_json::from_str(line.trim()).expect("JSON frame")
}

async fn subscribe(
    path: &std::path::Path,
    task_ids: &[&str],
    after_cursor: i64,
) -> (
    BufReader<tokio::net::unix::OwnedReadHalf>,
    tokio::net::unix::OwnedWriteHalf,
) {
    let stream = UnixStream::connect(path).await.expect("socket connect");
    let (reader, mut writer) = stream.into_split();
    writer
        .write_all(
            format!(
                "{}\n",
                json!({
                    "v": 1,
                    "watch": task_ids,
                    "afterCursor": after_cursor,
                })
            )
            .as_bytes(),
        )
        .await
        .expect("subscribe");
    (BufReader::new(reader), writer)
}

#[tokio::test]
async fn socket_replays_events_and_delivers_new_batches() {
    let fixture = Fixture::new(TaskState::Running);
    let replay_id = fixture.append("worker_spawned", TaskState::Running, json!({}));
    let path = fixture.path("events.sock");
    let handle = start_event_socket(
        fixture.store.clone(),
        options(path.clone(), Duration::from_secs(5)),
    )
    .expect("socket start");

    let (mut reader, _writer) = subscribe(&path, &["task"], 0).await;
    let hello: oga_domain::HelloFrame =
        serde_json::from_value(next_json(&mut reader).await).expect("hello");
    assert_eq!(hello.hello.version, "test");
    assert_eq!(hello.hello.initial_cursor, Some(replay_id));
    let first: oga_domain::BatchFrame =
        serde_json::from_value(next_json(&mut reader).await).expect("replay batch");
    assert_eq!(first.cursor, replay_id);
    assert_eq!(first.events[0].id, replay_id);
    assert_eq!(first.events[0].task_id, "task");

    let next_id = fixture.append("failed", TaskState::Failed, json!({ "detail": "stop" }));
    let second: oga_domain::BatchFrame =
        serde_json::from_value(next_json(&mut reader).await).expect("live batch");
    assert_eq!(second.cursor, next_id);
    assert_eq!(second.events[0].id, next_id);
    assert_eq!(second.events[0].kind, oga_domain::EventKind::Error);

    let (mut reader, _writer) = subscribe(&path, &["task"], next_id).await;
    let _: oga_domain::HelloFrame =
        serde_json::from_value(next_json(&mut reader).await).expect("reconnect hello");
    let later_id = fixture.append("completed", TaskState::Completed, json!({}));
    let reconnect: oga_domain::BatchFrame =
        serde_json::from_value(next_json(&mut reader).await).expect("reconnect batch");
    assert_eq!(
        reconnect
            .events
            .iter()
            .map(|event| event.id)
            .collect::<Vec<_>>(),
        [later_id]
    );
    handle.stop();
}

#[tokio::test]
async fn socket_batches_carry_settlement_outcomes() {
    let fixture = Fixture::new(TaskState::Running);
    let path = fixture.path("outcomes.sock");
    let handle = start_event_socket(
        fixture.store.clone(),
        options(path.clone(), Duration::from_secs(5)),
    )
    .expect("socket start");

    let (mut reader, _writer) = subscribe(&path, &["task"], 0).await;
    let _: oga_domain::HelloFrame =
        serde_json::from_value(next_json(&mut reader).await).expect("hello");
    fixture.update_task(TaskState::Completed, "A completed task", Some("the result"));
    fixture.append("completed", TaskState::Completed, json!({}));
    let batch: oga_domain::BatchFrame =
        serde_json::from_value(next_json(&mut reader).await).expect("settlement batch");
    assert_eq!(batch.events[0].outcome.as_deref(), Some("the result"));
    assert_eq!(batch.tasks[0].tldr.as_deref(), Some("the result"));
    assert_eq!(batch.tasks[0].title.as_deref(), Some("A completed task"));
    handle.stop();
}

#[tokio::test]
async fn socket_keepalive_sends_an_empty_batch_for_a_quiet_task() {
    let fixture = Fixture::new(TaskState::Running);
    let path = fixture.path("keepalive.sock");
    let handle = start_event_socket(
        fixture.store.clone(),
        options(path.clone(), Duration::from_millis(25)),
    )
    .expect("socket start");

    let (mut reader, _writer) = subscribe(&path, &["task"], 0).await;
    let _: oga_domain::HelloFrame =
        serde_json::from_value(next_json(&mut reader).await).expect("hello");
    let batch: oga_domain::BatchFrame =
        serde_json::from_value(next_json(&mut reader).await).expect("keepalive batch");
    assert!(batch.events.is_empty());
    assert_eq!(batch.tasks[0].id, "task");
    assert_eq!(batch.cursor, 0);
    handle.stop();
}

#[tokio::test]
async fn socket_rejects_unknown_tasks_and_reports_stale_cursors() {
    let fixture = Fixture::new(TaskState::Running);
    let first_id = fixture.append("worker_spawned", TaskState::Running, json!({}));
    let mut survivor = fixture.task.clone();
    survivor.id = "survivor".into();
    fixture
        .store
        .repositories()
        .tasks()
        .insert(&survivor)
        .expect("survivor task");
    let survivor_floor = fixture
        .store
        .repositories()
        .events()
        .append(&TaskEvent {
            id: 0,
            task_id: survivor.id.clone(),
            kind: "worker_spawned".into(),
            state: TaskState::Running,
            payload: BTreeMap::new(),
            created_at: "2026-01-01T00:00:02.000Z".into(),
            turn_id: None,
        })
        .expect("survivor event");
    let path = fixture.path("errors.sock");
    let handle = start_event_socket(
        fixture.store.clone(),
        options(path.clone(), Duration::from_secs(5)),
    )
    .expect("socket start");

    let (mut reader, _writer) = subscribe(&path, &["missing"], 0).await;
    let error: oga_domain::ErrorFrame =
        serde_json::from_value(next_json(&mut reader).await).expect("error frame");
    assert!(error.error.contains("missing"));

    drop(reader);
    let (mut reader, _writer) = subscribe(&path, &["survivor"], first_id).await;
    let hello: oga_domain::HelloFrame =
        serde_json::from_value(next_json(&mut reader).await).expect("hello");
    assert_eq!(hello.hello.stale, Some(true));
    assert_eq!(hello.hello.stream_floor, Some(survivor_floor));
    handle.stop();
}

#[test]
fn socket_skips_overlong_paths_without_binding() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
    let path = directory.path().join("x".repeat(200));
    let handle = start_event_socket(store, options(path, Duration::from_secs(5)))
        .expect("overlong paths are skipped");
    assert!(handle.path.is_none());
}

#[test]
fn socket_requires_a_runtime_before_binding() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
    let path = directory.path().join("runtime.sock");
    let result = start_event_socket(store, options(path.clone(), Duration::from_secs(5)));
    assert!(matches!(result, Err(oga_events::SocketError::NoRuntime)));
    assert!(!path.exists());
}

#[test]
fn resolves_the_socket_next_to_the_database() {
    let database = std::path::Path::new("/tmp/oga-test.db");
    assert_eq!(
        event_socket_path(database),
        std::path::Path::new("/tmp/oga.sock")
    );
}
