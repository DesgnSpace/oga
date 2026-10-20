//! Rearming a task that already ended, over the wire: it waits for its
//! prerequisites to complete again, then runs again in order.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use axum::body::Body;
use http_body_util::BodyExt;
use oga_http::HttpState;
use oga_mcp::{McpServer, router};
use oga_store::Store;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

/// A broker whose `head` worker finishes only once `head-gate` exists, and
/// takes the gate with it so every run waits for a new one. Each worker
/// appends its name to `runs` as it finishes, so a test reads which tasks ran
/// and in what order.
struct Broker {
    directory: TempDir,
    server: McpServer,
}

impl Broker {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        let root = directory.path().display().to_string();
        let finish = |name: &str| {
            format!("echo {name} >> '{root}/runs'; printf 'finished\\nOGA_RESULT: completed\\n'")
        };
        let workers = [
            (
                "head",
                format!(
                    "while [ ! -e '{root}/head-gate' ]; do sleep 0.02; done; rm '{root}/head-gate'; {}",
                    finish("head")
                ),
            ),
            ("a", finish("a")),
            ("b", finish("b")),
            ("c", finish("c")),
        ];
        let mut enabled = serde_json::Map::new();
        for (id, script) in workers {
            store
                .repositories()
                .profiles()
                .insert(
                    &oga_domain::Profile {
                        id: id.into(),
                        label: id.into(),
                        provider: oga_domain::Provider::Claude,
                        default_model: "model".into(),
                        enabled: true,
                        env: BTreeMap::new(),
                        capabilities: Vec::new(),
                        command: Some(vec!["sh".into(), "-c".into(), script]),
                    },
                    "2026-10-20T00:00:00.000Z",
                )
                .expect("profile");
            enabled.insert(id.into(), json!({ "modelEnabled": { "model": true } }));
        }
        store
            .repositories()
            .settings()
            .put(
                &oga_config::canonical_cwd(oga_config::global_cwd())
                    .display()
                    .to_string(),
                oga_config::MODEL_SETTINGS_KEY,
                &json!({ "profiles": enabled }).to_string(),
                "2026-10-20T00:00:00.000Z",
            )
            .expect("models on");
        let server = McpServer::new(HttpState::new(store));
        Self { directory, server }
    }

    fn cwd(&self) -> PathBuf {
        self.directory.path().to_path_buf()
    }

    fn open_head(&self) {
        std::fs::write(self.directory.path().join("head-gate"), "").expect("open gate");
    }

    fn runs(&self) -> Vec<String> {
        std::fs::read_to_string(self.directory.path().join("runs"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// One pass of the hold sweep, the scheduler's timer-driven release.
    fn sweep(&self) {
        self.server
            .state()
            .dispatcher
            .sweep_holds()
            .expect("hold sweep");
    }

    async fn call(&self, name: &str, arguments: Value) -> Value {
        let response = router(self.server.state().clone())
            .oneshot(
                axum::http::Request::post("/mcp")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .body(Body::from(
                        json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "tools/call",
                            "params": { "name": name, "arguments": arguments }
                        })
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let body = String::from_utf8(body.to_vec()).expect("UTF-8 body");
        let payload = body
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap_or(&body);
        let response: Value = serde_json::from_str(payload).expect("JSON-RPC response");
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("tool text in {response}"));
        serde_json::from_str(text).unwrap_or_else(|_| json!({ "error": text }))
    }

    async fn delegate(&self, profile: &str, title: &str, depends_on: &[&str]) -> String {
        let body = self
            .call(
                "delegate",
                json!({
                    "prompt": format!("{title} work"),
                    "cwd": self.cwd(),
                    "tldr": title,
                    "title": title,
                    "profile": profile,
                    "model": "model",
                    "dependsOn": depends_on,
                }),
            )
            .await;
        body["id"]
            .as_str()
            .unwrap_or_else(|| panic!("delegate answered {body}"))
            .to_owned()
    }

    async fn state(&self, id: &str) -> String {
        self.call("inspect", json!({ "taskId": id })).await["state"]
            .as_str()
            .expect("state")
            .to_owned()
    }

    async fn wait_for(&self, id: &str, wanted: &str) {
        let mut state = String::new();
        for _ in 0..1_000 {
            state = self.state(id).await;
            if state == wanted {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{id} never reached {wanted}; it is {state}");
    }

    /// Asserts `id` holds `state` for long enough that a release would have
    /// shown, with a sweep pass in the middle.
    async fn stays(&self, id: &str, state: &str) {
        for step in 0..20 {
            assert_eq!(self.state(id).await, state, "{id}");
            if step == 10 {
                self.sweep();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn rearm(&self, id: &str) -> Value {
        self.call("edit", json!({ "taskId": id, "rearm": true }))
            .await
    }

    /// Runs `head` again and waits for it to complete.
    async fn rerun_head(&self, head: &str) {
        let body = self
            .call(
                "resume",
                json!({ "taskId": head, "instruction": "run again" }),
            )
            .await;
        assert!(body["error"].is_null(), "resume head: {body}");
        self.open_head();
        self.wait_for(head, "completed").await;
    }

    /// `head`, then `a` waiting on it, both run to completion.
    async fn head_and_dependent(&self) -> (String, String) {
        let head = self.delegate("head", "Head", &[]).await;
        let a = self.delegate("a", "A", &[&head]).await;
        self.open_head();
        self.wait_for(&a, "completed").await;
        (head, a)
    }
}

fn error(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

/// Bug caught: a rearmed task counts its prerequisite's old completion and runs
/// at once, on stale results.
#[tokio::test]
async fn a_rearmed_task_waits_for_its_prerequisite_to_complete_again() {
    let broker = Broker::new();
    let (head, a) = broker.head_and_dependent().await;

    let body = broker.rearm(&a).await;
    assert_eq!(body["state"], "pending", "{body}");
    let inspected = broker.call("inspect", json!({ "taskId": a })).await;
    assert!(
        inspected
            .to_string()
            .contains("rearmed: waiting for Head to complete again"),
        "{inspected}"
    );
    broker.stays(&a, "pending").await;

    broker
        .call(
            "resume",
            json!({ "taskId": head, "instruction": "run again" }),
        )
        .await;
    broker.wait_for(&head, "running").await;
    broker.stays(&a, "pending").await;

    broker.open_head();
    broker.wait_for(&head, "completed").await;
    broker.wait_for(&a, "completed").await;
    assert_eq!(broker.runs(), ["head", "a", "head", "a"]);
    let inspected = broker.call("inspect", json!({ "taskId": a })).await;
    assert_eq!(inspected["id"], a);
    assert_eq!(
        inspected["attemptCount"], 1,
        "the first run is kept as an attempt: {inspected}"
    );
}

/// Bug caught: rearmed tasks in a chain start together, or out of order, when
/// the head of the chain completes again.
#[tokio::test]
async fn a_rearmed_chain_runs_in_order_after_its_head_completes_again() {
    let broker = Broker::new();
    let (head, a) = broker.head_and_dependent().await;
    let b = broker.delegate("b", "B", &[&a]).await;
    let c = broker.delegate("c", "C", &[&b]).await;
    broker.wait_for(&c, "completed").await;

    for id in [&c, &b, &a] {
        let body = broker.rearm(id).await;
        assert_eq!(body["state"], "pending", "{body}");
    }
    broker.rerun_head(&head).await;
    broker.wait_for(&c, "completed").await;
    assert_eq!(
        broker.runs(),
        ["head", "a", "b", "c", "head", "a", "b", "c"]
    );
}

/// Bug caught: resuming a prerequisite reruns everything after it, though
/// nobody rearmed it.
#[tokio::test]
async fn a_prerequisite_run_again_leaves_dependents_that_were_not_rearmed() {
    let broker = Broker::new();
    let (head, a) = broker.head_and_dependent().await;

    broker.rerun_head(&head).await;
    broker.stays(&a, "completed").await;
    assert_eq!(broker.runs(), ["head", "a", "head"]);
}

/// Bug caught: a rearm parks a task that can never be released, or yanks a
/// running one out of its run.
#[tokio::test]
async fn a_rearm_without_prerequisites_or_on_a_running_task_is_refused() {
    let broker = Broker::new();
    let head = broker.delegate("head", "Head", &[]).await;
    broker.wait_for(&head, "running").await;

    let running = broker.rearm(&head).await;
    assert!(
        error(&running).contains("only a task that has ended can be rearmed"),
        "{running}"
    );

    broker.open_head();
    broker.wait_for(&head, "completed").await;
    let alone = broker.rearm(&head).await;
    assert!(error(&alone).contains("waits on no other task"), "{alone}");
    assert_eq!(broker.state(&head).await, "completed");
}

/// Bug caught: a rearm dropped by cancel or `rearm: false` still fires when the
/// prerequisite completes again.
#[tokio::test]
async fn cancel_and_rearm_false_drop_a_waiting_rearm() {
    let broker = Broker::new();
    let (head, a) = broker.head_and_dependent().await;
    let b = broker.delegate("b", "B", &[&head]).await;
    broker.wait_for(&b, "completed").await;

    broker.rearm(&a).await;
    let cancelled = broker.call("cancel", json!({ "taskId": a })).await;
    assert!(cancelled.to_string().contains("cancelled"), "{cancelled}");

    broker.rearm(&b).await;
    let dropped = broker
        .call("edit", json!({ "taskId": b, "rearm": false }))
        .await;
    assert_eq!(dropped["state"], "completed", "{dropped}");
    let again = broker
        .call("edit", json!({ "taskId": b, "rearm": false }))
        .await;
    assert!(error(&again).contains("not waiting on a rearm"), "{again}");

    broker.rerun_head(&head).await;
    broker.stays(&a, "cancelled").await;
    broker.stays(&b, "completed").await;
    let runs = broker.runs();
    assert_eq!(runs.iter().filter(|run| *run == "a").count(), 1, "{runs:?}");
    assert_eq!(runs.iter().filter(|run| *run == "b").count(), 1, "{runs:?}");
}
