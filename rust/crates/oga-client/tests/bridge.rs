//! The shell-side half of the desktop bridge: call dispatch and the stream pump.

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
use oga_client::{
    EventFrame, EventStreamOptions, LoopbackClient,
    bridge::{BrokerCall, StreamFrame, StreamPump, StreamStatus},
};
use serde_json::json;
use tokio::{
    net::TcpListener,
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
};

const FAST: EventStreamOptions = EventStreamOptions {
    initial_backoff: Duration::from_millis(1),
    max_backoff: Duration::from_millis(1),
};

#[derive(Clone)]
struct Broker {
    stream_queries: Arc<Mutex<Vec<String>>>,
    connections: Arc<AtomicUsize>,
    stream_floor: i64,
    event_head: i64,
    events_fail: bool,
}

async fn start(stream_floor: i64, events_fail: bool) -> (LoopbackClient, Broker) {
    start_with_head(stream_floor, 0, events_fail).await
}

async fn start_with_head(
    stream_floor: i64,
    event_head: i64,
    events_fail: bool,
) -> (LoopbackClient, Broker) {
    let broker = Broker {
        stream_queries: Arc::new(Mutex::new(Vec::new())),
        connections: Arc::new(AtomicUsize::new(0)),
        stream_floor,
        event_head,
        events_fail,
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
    if request.uri().path() == "/api/events" {
        return events(&broker, query.unwrap_or_default());
    }
    if request.uri().path() == "/api/events/head" {
        return json(StatusCode::OK, json!({ "cursor": broker.event_head }));
    }
    if request.uri().path() == "/health" {
        return json(
            StatusCode::OK,
            json!({ "status": "ok", "version": oga_domain::VERSION, "mcpContractVersion": 32,
                "build": "dev", "stale": false }),
        );
    }
    json(StatusCode::CONFLICT, json!({ "error": "stale revision" }))
}

/// Each connection serves one batch, then closes so the pump must reconnect.
fn events(broker: &Broker, query: String) -> Response<Body> {
    broker
        .stream_queries
        .lock()
        .expect("query lock")
        .push(query.clone());
    if broker.events_fail {
        return json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "broker restarting" }),
        );
    }
    let connection = broker.connections.fetch_add(1, Ordering::SeqCst);
    let after = query
        .strip_prefix("after=")
        .and_then(|value| value.split('&').next())
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let stale = broker.stream_floor > 0 && after < broker.stream_floor;
    let cursor = if stale {
        broker.stream_floor - 1
    } else {
        after
    };
    let body = format!(
        "event: ready\ndata: {}\n\nevent: task\ndata: {}\n\n",
        json!({ "version": 1, "cursor": cursor, "streamFloor": broker.stream_floor,
            "stale": stale }),
        pointer(cursor + 1, connection),
    );
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from(body))
        .expect("response")
}

fn pointer(cursor: i64, connection: usize) -> serde_json::Value {
    json!({
        "id": cursor, "cursor": cursor, "taskId": format!("task-{connection}"),
        "type": "started", "kind": "lifecycle", "state": "running",
        "at": "2026-01-01T00:00:00.000Z", "title": "Task", "summary": "started"
    })
}

fn json(status: StatusCode, value: serde_json::Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(value.to_string()))
        .expect("response")
}

struct Pumped {
    frames: UnboundedReceiver<StreamFrame>,
    statuses: UnboundedReceiver<StreamStatus>,
    task: tokio::task::JoinHandle<()>,
}

impl Pumped {
    async fn next_frame(&mut self) -> StreamFrame {
        tokio::time::timeout(Duration::from_secs(5), self.frames.recv())
            .await
            .expect("a frame within five seconds")
            .expect("the pump is still running")
    }

    async fn next_status(&mut self) -> StreamStatus {
        tokio::time::timeout(Duration::from_secs(5), self.statuses.recv())
            .await
            .expect("a status within five seconds")
            .expect("the pump is still running")
    }

    async fn stays_silent(&mut self) -> bool {
        tokio::time::timeout(Duration::from_millis(200), self.statuses.recv())
            .await
            .is_err()
    }
}

impl Drop for Pumped {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn pump(client: LoopbackClient) -> (StreamPump, Pumped) {
    let pump = StreamPump::new(client, FAST);
    let (frame_sender, frames): (UnboundedSender<StreamFrame>, _) = mpsc::unbounded_channel();
    let (status_sender, statuses) = mpsc::unbounded_channel();
    let running = pump.clone();
    let task = tokio::spawn(async move {
        running
            .run(
                move |frame| {
                    let (event, data) = frame.clone().into_parts();
                    let _ = frame_sender.send(StreamFrame { event, data });
                },
                move |status| {
                    let _ = status_sender.send(status);
                },
            )
            .await
    });
    (
        pump,
        Pumped {
            frames,
            statuses,
            task,
        },
    )
}

#[tokio::test]
async fn the_pump_starts_at_the_event_log_head() {
    let (client, broker) = start_with_head(0, 900, false).await;
    let (_pump, mut pumped) = pump(client);

    assert_eq!(pumped.next_frame().await.event, "ready");
    let first = pumped.next_frame().await;
    assert_eq!(first.data["cursor"], 901);

    let queries = broker.stream_queries.lock().expect("query lock");
    assert_eq!(queries[0], "after=900");
}

#[tokio::test]
async fn the_pump_resumes_from_its_own_cursor_after_a_reconnect() {
    let (client, broker) = start(0, false).await;
    let (pump, mut pumped) = pump(client);

    assert_eq!(pumped.next_frame().await.event, "ready");
    let first = pumped.next_frame().await;
    assert_eq!(first.data["cursor"], 1);
    assert_eq!(first.data["taskId"], "task-0");

    // The connection closed after that batch, so the next frames prove the
    // pump reconnected and asked the broker to carry on from cursor 1.
    assert_eq!(pumped.next_frame().await.event, "ready");
    let second = pumped.next_frame().await;
    assert_eq!(second.data["cursor"], 2);
    assert_eq!(second.data["taskId"], "task-1");
    assert!(pump.status().cursor >= 2);

    let queries = broker.stream_queries.lock().expect("query lock");
    assert_eq!(queries[0], "after=0");
    assert_eq!(queries[1], "after=1");
}

#[tokio::test]
async fn a_truncated_log_reaches_the_web_view_as_a_stale_stream() {
    let (client, _) = start(40, false).await;
    let (pump, mut pumped) = pump(client);

    let ready = pumped.next_frame().await;

    assert_eq!(ready.event, "ready");
    assert_eq!(ready.data["streamFloor"], 40);
    assert_eq!(ready.data["stale"], true);
    let status = pump.status();
    assert_eq!(status.stream_floor, 40);
    assert!(status.stale);
    assert!(
        status.cursor >= 39,
        "the pump adopted the floor as its cursor, got {}",
        status.cursor
    );
}

#[tokio::test]
async fn frames_reach_the_web_view_unchanged() {
    let (client, _) = start(0, false).await;
    let (_pump, mut pumped) = pump(client);

    let ready = pumped.next_frame().await;
    let task = pumped.next_frame().await;

    assert_eq!(
        EventFrame::from_parts(&ready.event, ready.data).expect("ready frame"),
        EventFrame::Ready(oga_client::ReadyFrame {
            version: 1,
            cursor: 0,
            stream_floor: 0,
            tasks: Vec::new(),
            kinds: Vec::new(),
            agents: false,
            stale: false,
        })
    );
    assert!(matches!(
        EventFrame::from_parts(&task.event, task.data).expect("task frame"),
        EventFrame::Task(pointer) if pointer.task_id == "task-0"
    ));
}

/// The cursor moves with every frame and no view reads it, so a status that
/// says nothing new must not cross to the web view — a streaming run would
/// otherwise pay for one on every event.
#[tokio::test]
async fn a_connection_that_holds_is_announced_once() {
    let (client, _) = start(0, false).await;
    let (_pump, mut pumped) = pump(client);

    let connected = pumped.next_status().await;
    assert!(connected.connected);
    for _ in 0..4 {
        let _ = pumped.next_frame().await;
    }

    assert!(
        pumped.stays_silent().await,
        "a connection that never changed was announced twice"
    );
}

#[tokio::test]
async fn an_unreachable_stream_is_reported_without_losing_the_cursor() {
    let (client, _) = start(0, true).await;
    let (pump, mut pumped) = pump(client);

    let status = pumped.next_status().await;

    assert!(!status.connected);
    assert!(
        status
            .error
            .as_deref()
            .is_some_and(|error| error.contains("broker restarting")),
        "expected the broker error, got {:?}",
        status.error
    );
    assert_eq!(pump.status().cursor, 0);
}

#[tokio::test]
async fn calls_carry_the_broker_status_back_to_the_web_view() {
    let (client, _) = start(0, false).await;

    let health = client.call(BrokerCall::Health).await.expect("health");
    let refused = client
        .call(BrokerCall::Projects)
        .await
        .expect_err("the mock refuses every other route");

    assert_eq!(health["status"], "ok");
    assert_eq!(refused.status, Some(409));
    assert!(refused.message.contains("stale revision"));
}
