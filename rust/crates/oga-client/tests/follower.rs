//! The shell-side follower: what one broker frame costs the web view.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
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
use tokio::sync::Barrier;

const WATCH_EVENTS: u64 = 150;

#[derive(Clone)]
struct Broker {
    events: Arc<Mutex<Vec<i64>>>,
    reads: Arc<AtomicUsize>,
    reads_before_viewed: Arc<AtomicUsize>,
    viewed: Arc<AtomicBool>,
    read_barrier: Option<Arc<Barrier>>,
    task: Arc<Mutex<Task>>,
    combined_event_page: bool,
    response_bytes: Arc<AtomicUsize>,
}

impl Broker {
    /// How many times the broker was asked for anything.
    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn reads_before_viewed(&self) -> usize {
        self.reads_before_viewed.load(Ordering::SeqCst)
    }

    fn response_bytes(&self) -> usize {
        self.response_bytes.load(Ordering::SeqCst)
    }

    fn append(&self, ids: impl IntoIterator<Item = i64>) {
        self.events.lock().expect("events lock").extend(ids);
    }
}

async fn start(events: impl IntoIterator<Item = i64>) -> (LoopbackClient, Broker) {
    start_config(events, None, false).await
}

async fn start_with_read_barrier(
    events: impl IntoIterator<Item = i64>,
    read_barrier: Option<Arc<Barrier>>,
) -> (LoopbackClient, Broker) {
    start_config(events, read_barrier, false).await
}

async fn start_combined(events: impl IntoIterator<Item = i64>) -> (LoopbackClient, Broker) {
    start_config(events, None, true).await
}

async fn start_config(
    events: impl IntoIterator<Item = i64>,
    read_barrier: Option<Arc<Barrier>>,
    combined_event_page: bool,
) -> (LoopbackClient, Broker) {
    let broker = Broker {
        events: Arc::new(Mutex::new(events.into_iter().collect())),
        reads: Arc::new(AtomicUsize::new(0)),
        reads_before_viewed: Arc::new(AtomicUsize::new(0)),
        viewed: Arc::new(AtomicBool::new(false)),
        read_barrier,
        task: Arc::new(Mutex::new(Task {
            id: "task".to_owned(),
            state: TaskState::Running,
            title: Some("Working".to_owned()),
            ..Task::default()
        })),
        combined_event_page,
        response_bytes: Arc::new(AtomicUsize::new(0)),
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
    let path = request.uri().path();
    if path.ends_with("/view") {
        if request.method().as_str() == "POST" {
            broker.viewed.store(true, Ordering::SeqCst);
        }
        return json(&broker, json!({ "ok": true }));
    }
    if request.method().as_str() == "GET" {
        if !broker.viewed.load(Ordering::SeqCst) {
            broker.reads_before_viewed.fetch_add(1, Ordering::SeqCst);
        }
        if let Some(barrier) = &broker.read_barrier {
            barrier.wait().await;
        }
    }
    if path.ends_with("/events") {
        return events(&broker, query.unwrap_or_default()).await;
    }
    json(
        &broker,
        serde_json::to_value(&*broker.task.lock().expect("task lock")).expect("task json"),
    )
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
    let mut body = json!({
        "events": page,
        "cursor": held.last().copied().unwrap_or(0),
        "hasMore": false,
        "oldestId": held.first().copied().unwrap_or(0),
        "hasEarlier": false,
    });
    if broker.combined_event_page {
        body["task"] =
            serde_json::to_value(&*broker.task.lock().expect("task lock")).expect("task json");
        body["taskUpdatedAt"] = json!(broker.task.lock().expect("task lock").updated_at.clone());
    }
    json(broker, body)
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

fn json(broker: &Broker, value: serde_json::Value) -> Response<Body> {
    let body = value.to_string();
    broker
        .response_bytes
        .fetch_add(body.len(), Ordering::SeqCst);
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from(body))
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

#[tokio::test]
async fn a_combined_event_page_avoids_an_unchanged_task_read() {
    let (client, broker) = start_combined(1..=3).await;
    let follower = TaskFollower::new(client);
    follower.watch("task", WATCH_EVENTS).await.expect("watch");
    let after_watch = broker.reads();

    broker.append([4]);
    follower.note(&pointer("task", 4));
    let delta = follower.next_delta().await.expect("an update");

    assert_eq!(broker.reads() - after_watch, 1, "one combined event read");
    assert_eq!(delta.events.len(), 1);
    assert!(delta.task.is_none());
}

#[tokio::test]
#[ignore]
async fn measured_combined_delivery_fixture() {
    const EVENT_COUNT: i64 = 10_000;
    const POINTER_QUEUE_CAPACITY: usize = 1_024;
    async fn measure(combined: bool) -> (usize, usize, usize, usize, f64, f64) {
        let (client, broker) = if combined {
            start_combined(1..=3).await
        } else {
            start(1..=3).await
        };
        let follower = TaskFollower::new(client);
        follower.watch("task", WATCH_EVENTS).await.expect("watch");
        let before_reads = broker.reads();
        let before_bytes = broker.response_bytes();
        let started = Instant::now();

        broker.append(4..=EVENT_COUNT + 3);
        for cursor in 4..=EVENT_COUNT + 3 {
            follower.note(&pointer("task", cursor));
        }
        let delta = follower.next_delta().await.expect("an update");
        let elapsed = started.elapsed().as_secs_f64();
        let decoded_event_bytes = serde_json::to_vec(&delta.events)
            .expect("decoded events")
            .len();
        (
            delta.events.len(),
            broker.reads() - before_reads,
            broker.response_bytes() - before_bytes,
            decoded_event_bytes,
            elapsed * 1_000.0,
            EVENT_COUNT as f64 / elapsed,
        )
    }

    let baseline = measure(false).await;
    let combined = measure(true).await;
    let pointer_wire_bytes = serde_json::to_vec(&pointer("task", 4))
        .expect("pointer")
        .len();
    println!(
        "baseline events={} broker_reads={} serialized_response_bytes={} decoded_event_bytes={} pointer_wire_bytes={} queue_capacity={} elapsed_ms={:.2} throughput_events_per_sec={:.0}",
        baseline.0,
        baseline.1,
        baseline.2,
        baseline.3,
        pointer_wire_bytes,
        POINTER_QUEUE_CAPACITY,
        baseline.4,
        baseline.5,
    );
    println!(
        "combined events={} broker_reads={} serialized_response_bytes={} decoded_event_bytes={} pointer_wire_bytes={} queue_capacity={} elapsed_ms={:.2} throughput_events_per_sec={:.0}",
        combined.0,
        combined.1,
        combined.2,
        combined.3,
        pointer_wire_bytes,
        POINTER_QUEUE_CAPACITY,
        combined.4,
        combined.5,
    );
    assert_eq!(baseline.0, EVENT_COUNT as usize);
    assert_eq!(combined.0, EVENT_COUNT as usize);
    assert_eq!(baseline.1, 2);
    assert_eq!(combined.1, 1);
}

/// The viewed mark completes before the task and activity reads begin, and
/// those reads do not wait for each other.
#[tokio::test]
async fn initial_reads_overlap_after_marking_viewed() {
    let barrier = Arc::new(Barrier::new(2));
    let (client, broker) = start_with_read_barrier(1..=3, Some(barrier)).await;
    let follower = TaskFollower::new(client);

    let snapshot =
        tokio::time::timeout(Duration::from_secs(1), follower.watch("task", WATCH_EVENTS))
            .await
            .expect("task and event reads overlapped")
            .expect("watch");

    assert_eq!(snapshot.events.len(), 3);
    assert_eq!(broker.reads_before_viewed(), 0);
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
