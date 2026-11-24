//! How a worker's last words settle its task, over the wire: a request for
//! input parks the task and holds what waits on it.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use axum::body::Body;
use http_body_util::BodyExt;
use oga_http::HttpState;
use oga_mcp::{McpServer, router};
use oga_store::Store;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

struct Broker {
    directory: TempDir,
    server: McpServer,
}

impl Broker {
    /// One worker per name, each running its script as the task.
    fn new(workers: &[(&str, &str)]) -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        let mut enabled = serde_json::Map::new();
        for (id, script) in workers {
            store
                .repositories()
                .profiles()
                .insert(
                    &oga_domain::Profile {
                        id: (*id).into(),
                        label: (*id).into(),
                        provider: oga_domain::Provider::Claude,
                        default_model: "model".into(),
                        enabled: true,
                        env: BTreeMap::new(),
                        capabilities: Vec::new(),
                        command: Some(vec!["sh".into(), "-c".into(), (*script).into()]),
                    },
                    "2026-10-20T00:00:00.000Z",
                )
                .expect("profile");
            enabled.insert((*id).into(), json!({ "modelEnabled": { "model": true } }));
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
        Self {
            directory,
            server: McpServer::new(HttpState::new(store)),
        }
    }

    fn cwd(&self) -> PathBuf {
        self.directory.path().to_path_buf()
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

    async fn delegate(&self, profile: &str, depends_on: &[&str]) -> String {
        let body = self
            .call(
                "delegate",
                json!({
                    "prompt": format!("{profile} work"),
                    "cwd": self.cwd(),
                    "tldr": profile,
                    "title": profile,
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

    async fn inspect(&self, id: &str) -> Value {
        self.call("inspect", json!({ "taskId": id })).await
    }

    async fn wait_for(&self, id: &str, wanted: &str) {
        let mut state = String::new();
        for _ in 0..1_000 {
            state = self.inspect(id).await["state"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if state == wanted {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{id} never reached {wanted}; it is {state}");
    }
}

/// Bug caught: a worker that opens its last message with `needs_input:` and
/// then writes its report settles as completed, and what waits on it starts.
#[tokio::test]
async fn a_request_for_input_ahead_of_a_report_parks_the_task_and_holds_its_dependent() {
    let broker = Broker::new(&[
        (
            "asker",
            "printf '%s\\n' 'needs_input: please start `make dev`, then tell me.' '' 'TL;DR: the code is written and committed.' '' 'Not verified: every screen.'",
        ),
        ("after", "printf 'finished\\nOGA_RESULT: completed\\n'"),
    ]);
    let asker = broker.delegate("asker", &[]).await;
    let after = broker.delegate("after", &[&asker]).await;

    broker.wait_for(&asker, "needs_input").await;
    let inspected = broker.inspect(&asker).await;
    assert_eq!(
        inspected["question"], "please start `make dev`, then tell me.",
        "{inspected}"
    );
    for _ in 0..30 {
        let dependent = broker.inspect(&after).await;
        assert_eq!(dependent["state"], "pending", "{dependent}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A report that only quotes the marker mid-way is still a finished report.
#[tokio::test]
async fn a_report_that_quotes_the_marker_mid_way_still_completes() {
    let broker = Broker::new(&[(
        "quoter",
        "printf '%s\\n' 'Done.' '' 'The worker prompt says to write needs_input: <question> when blocked.' '' 'Everything is merged.'",
    )]);
    let quoter = broker.delegate("quoter", &[]).await;
    broker.wait_for(&quoter, "completed").await;
}

/// Bug caught: a worker that asks the owner one question with choices and then
/// writes its report settles as completed, so nobody is told it is waiting.
#[tokio::test]
async fn a_question_with_choices_ahead_of_a_report_parks_the_task() {
    let broker = Broker::new(&[(
        "asker",
        "printf '%s\\n' \
            'I raised the spacing in 2 commits. No app dev server is running, so there are no screenshots.' \
            '' \
            'Question: can you start the app dev server and send me the URL? Port 3000 is the landing site.' \
            '- A: you start it, and I screenshot every screen, then fix what I see.' \
            '- B: skip the visual check for now.' \
            '' \
            'TL;DR: every screen has more room now.' \
            '' \
            'Not verified: how it looks on screen.'",
    )]);
    let asker = broker.delegate("asker", &[]).await;

    broker.wait_for(&asker, "needs_input").await;
    let inspected = broker.inspect(&asker).await;
    assert_eq!(
        inspected["question"],
        "Question: can you start the app dev server and send me the URL? Port 3000 is the landing site.\n\
         - A: you start it, and I screenshot every screen, then fix what I see.\n\
         - B: skip the visual check for now.",
        "{inspected}"
    );
}

/// A report that asks itself a question, and lists choices it already made,
/// is still a finished report.
#[tokio::test]
async fn a_report_with_a_question_and_lettered_choices_it_made_still_completes() {
    let broker = Broker::new(&[(
        "reporter",
        "printf '%s\\n' \
            'TL;DR: the import retries now.' \
            '' \
            'Why did it fail? The token expired mid-run.' \
            '- The retry keeps the first amount.' \
            '' \
            'Choices I weighed:' \
            '- A: retry in the job.' \
            '- B: retry in the client.' \
            '' \
            'Tests pass.'",
    )]);
    let reporter = broker.delegate("reporter", &[]).await;
    broker.wait_for(&reporter, "completed").await;
}
