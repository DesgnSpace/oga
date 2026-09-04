//! The shell-side follower: what one broker frame costs the web view.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::{RawQuery, State},
    http::StatusCode,
    response::Response,
    routing::any,
};
use oga_client::{LoopbackClient, bridge::TaskFollower};
use oga_domain::{EventKind, EventPointer, Task, TaskState};
use serde_json::json;
use tokio::net::TcpListener;

const WATCH_EVENTS: u64 = 150;

#[derive(Clone)]
struct Broker {
    events: Arc<Mutex<Vec<i64>>>,
    reads: Arc<AtomicUsize>,
    task: Arc<Mutex<Task>>,
}

impl Broker {
    /// How many times the broker was asked for anything.
    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn append(&self, ids: impl IntoIterator<Item = i64>) {
        self.events.lock().expect("events lock").extend(ids);
    }
}

async fn start(events: impl IntoIterator<Item = i64>) -> (LoopbackClient, Broker) {
    let broker = Broker {
        events: Arc::new(Mutex::new(events.into_iter().collect())),
        reads: Arc::new(AtomicUsize::new(0)),
        task: Arc::new(Mutex::new(Task {
            id: "task".to_owned(),
            state: TaskState::Running,
            title: Some("Working".to_owned()),
            ..Task::default()
        })),
    };
    let router = Router::new()
        .fallback(any(respond))
        .with_state(broker.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("broker");
    });
    (
        LoopbackClient::new(format!("http://{address}")).expect("client"),
        broker,
    )
}

async fn respond(
    State(broker): State<Broker>,
    RawQuery(query): RawQuery,
    request: axum::http::Request<Body>,
) -> Response<Body> {
    broker.reads.fetch_add(1, Ordering::SeqCst);
    if request.uri().path().ends_with("/events") {
        return events(&broker, query.unwrap_or_default()).await;
    }
    json(serde_json::to_value(&*broker.task.lock().expect("task lock")).expect("task json"))
}

async fn events(broker: &Broker, query: String) -> Response<Body> {
    let after = number(&query, "after");
    let held = broker.events.lock().expect("events lock").clone();
    let page = held
        .iter()
        .filter(|id| after.is_none_or(|after| **id > after))
        .map(|id| event(*id))
        .collect::<Vec<_>>();
    // The broker holds a read open until the task's log moves. Standing in
    // for that is what keeps the follower's own wait out of the read counts.
    if page.is_empty()
        && let Some(wait_ms) = number(&query, "waitMs")
    {
        tokio::time::sleep(Duration::from_millis(wait_ms.max(0) as u64)).await;
    }
    json(json!({
        "events": page,
        "cursor": held.last().copied().unwrap_or(0),
        "hasMore": false,
        "oldestId": held.first().copied().unwrap_or(0),
        "hasEarlier": false,
    }))
}

fn number(query: &str, key: &str) -> Option<i64> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| *name == key)
        .and_then(|(_, value)| value.parse().ok())
}

fn event(id: i64) -> serde_json::Value {
    json!({
        "id": id,
        "taskId": "task",
        "source": "claude",
        "type": "agent.tool",
        "kind": "tool",
        "phase": "completed",
        "title": format!("Step {id}"),
        "createdAt": "2026-01-01T00:00:00.000Z",
    })
}

fn json(value: serde_json::Value) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from(value.to_string()))
        .expect("response")
}

fn pointer(task_id: &str, cursor: i64) -> EventPointer {
    EventPointer {
        id: cursor,
        cursor,
        task_id: task_id.to_owned(),
        event_type: "agent.tool".to_owned(),
        kind: EventKind::Tool,
        state: TaskState::Running,
        at: "2026-01-01T00:00:00.000Z".to_owned(),
        title: "Step".to_owned(),
        summary: "step".to_owned(),
        turn_id: None,
    }
}

/// Nothing the follower cares about happened, so no update may come out.
async fn stays_quiet(follower: &TaskFollower) {
    assert!(
        tokio::time::timeout(Duration::from_millis(150), follower.next_delta())
            .await
            .is_err(),
        "the follower sent an update for something it was not watching"
    );
}

/// A run streaming events sends a frame per event. Reading the task once per
/// frame is what made the app stutter; frames that pile up while a read is in
/// flight have to collapse into that read.
#[tokio::test]
async fn a_burst_of_frames_costs_one_read() {
    let (client, broker) = start(1..=3).await;
    let follower = TaskFollower::new(client);
    follower.watch("task", WATCH_EVENTS).await.expect("watch");
    let after_watch = broker.reads();

    broker.append(4..=103);
    for cursor in 4..=103 {
        follower.note(&pointer("task", cursor));
    }
    let delta = follower.next_delta().await.expect("an update");

    assert_eq!(
        broker.reads() - after_watch,
        2,
        "one events read, one task read"
    );
    assert_eq!(delta.from_cursor, 3);
    assert_eq!(delta.cursor, 103);
    assert_eq!(delta.events.len(), 100);
}

/// The update carries the activity the web view does not have, and the task
/// row only when the row itself moved.
#[tokio::test]
async fn an_update_carries_only_what_is_new() {
    let (client, broker) = start(1..=3).await;
    let follower = TaskFollower::new(client);
    let snapshot = follower.watch("task", WATCH_EVENTS).await.expect("watch");
    assert_eq!(snapshot.events.len(), 3);
    assert_eq!(snapshot.cursor, 3);

    broker.append([4, 5]);
    follower.note(&pointer("task", 5));
    let delta = follower.next_delta().await.expect("an update");

    assert_eq!(
        delta
            .events
            .iter()
            .map(|event| event.id)
            .collect::<Vec<_>>(),
        vec![4, 5]
    );
    assert!(delta.task.is_none(), "the task row did not move");

    broker.task.lock().expect("task lock").title = Some("Finished".to_owned());
    broker.append([6]);
    follower.note(&pointer("task", 6));
    let delta = follower.next_delta().await.expect("an update");

    assert_eq!(delta.from_cursor, 5);
    assert_eq!(delta.events.len(), 1);
    assert_eq!(
        delta.task.and_then(|task| task.title).as_deref(),
        Some("Finished")
    );
}

/// The shell's stream starts wherever its cursor left off and can be a long
/// way behind the head. The task on screen still has to move.
#[tokio::test]
async fn a_task_moves_without_any_frame() {
    let (client, broker) = start(1..=3).await;
    let follower = TaskFollower::new(client);
    follower.watch("task", WATCH_EVENTS).await.expect("watch");

    broker.append([4]);
    let delta = tokio::time::timeout(Duration::from_secs(5), follower.next_delta())
        .await
        .expect("an update within five seconds")
        .expect("an update");

    assert_eq!(delta.from_cursor, 3);
    assert_eq!(delta.events.len(), 1);
}

/// The shell's stream carries every task. Only the one on screen may cost a
/// read.
#[tokio::test]
async fn frames_for_other_tasks_cost_nothing() {
    let (client, broker) = start(1..=3).await;
    let follower = TaskFollower::new(client);
    follower.watch("task", WATCH_EVENTS).await.expect("watch");
    let after_watch = broker.reads();

    for cursor in 4..=103 {
        follower.note(&pointer("elsewhere", cursor));
    }

    stays_quiet(&follower).await;
    assert_eq!(
        broker.reads() - after_watch,
        1,
        "the follower's own held-open read, and nothing per frame"
    );
}

#[tokio::test]
async fn a_released_task_stops_costing_anything() {
    let (client, broker) = start(1..=3).await;
    let follower = TaskFollower::new(client);
    follower.watch("task", WATCH_EVENTS).await.expect("watch");
    follower.unwatch("task");
    let after_watch = broker.reads();

    broker.append([4]);
    follower.note(&pointer("task", 4));

    stays_quiet(&follower).await;
    assert_eq!(
        broker.reads(),
        after_watch + 1,
        "unwatch clears the broker's viewed-task guard once"
    );
}

/// A frame whose activity the follower already read is not worth waking the
/// web view for.
#[tokio::test]
async fn a_frame_that_adds_nothing_sends_nothing() {
    let (client, _) = start(1..=3).await;
    let follower = TaskFollower::new(client);
    follower.watch("task", WATCH_EVENTS).await.expect("watch");

    follower.note(&pointer("task", 3));

    assert!(follower.next_delta().await.is_none());
}
