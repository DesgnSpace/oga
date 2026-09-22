//! Tasks over ACP, through the real dispatcher, against a scripted agent and a
//! scripted command line: no account, no model, no network.

use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use oga_domain::{
    AcpRestore, AcpSteering, CompletionCode, Profile, Provider, Task, TaskScope, TaskState,
    TaskTransport, Transport, TransportPreference, TransportReason,
};
use oga_providers::{AcpAdapter, AcpAdapters};
use oga_service::{
    CancelRequest, DispatchRequest, Dispatcher, ResumeRequest, SteerRequest, cancel,
    reconcile::ReconcileTrigger, resume, set_transport_preference, steer,
};
use oga_store::Store;
use serde_json::Value;
use tempfile::TempDir;

const NOW: &str = "2026-01-01T00:00:00.000Z";

struct Harness {
    _directory: TempDir,
    cwd: PathBuf,
    log: PathBuf,
    store: Arc<Store>,
    dispatcher: Dispatcher,
}

/// A Claude profile with no custom command, so it is eligible for ACP, whose
/// command line resolves to a script that records that it ran.
fn harness(mode: &str) -> Harness {
    harness_with(mode, env!("CARGO_BIN_EXE_fake-acp-agent"))
}

fn harness_with(mode: &str, agent: &str) -> Harness {
    let directory = tempfile::tempdir().expect("temporary directory");
    let cwd = directory.path().join("project");
    let bin = directory.path().join("bin");
    fs::create_dir_all(cwd.join("src")).expect("project");
    fs::create_dir_all(&bin).expect("bin");
    let cli = bin.join("claude");
    fs::write(
        &cli,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PWD/cli-ran\"\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"ran on the command line\\nOGA_RESULT: completed\",\"session_id\":\"cli-session-1\"}'\n",
    )
    .expect("fake command line");
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).expect("executable");

    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
    let profile = Profile {
        id: "work".into(),
        label: "Work".into(),
        provider: Provider::Claude,
        default_model: "model-one".into(),
        enabled: true,
        env: BTreeMap::from([
            ("PATH".into(), bin.display().to_string()),
            (
                "CLAUDE_CONFIG_DIR".into(),
                directory.path().join("account").display().to_string(),
            ),
        ]),
        capabilities: vec![],
        command: None,
    };
    store
        .repositories()
        .profiles()
        .insert(&profile, NOW)
        .expect("profile");
    store
        .repositories()
        .settings()
        .put(
            &oga_config::canonical_cwd(oga_config::global_cwd())
                .display()
                .to_string(),
            oga_config::MODEL_SETTINGS_KEY,
            &serde_json::json!({"profiles": {"work": {"modelEnabled": {"model-one": true}}}})
                .to_string(),
            NOW,
        )
        .expect("model settings");

    let log = directory.path().join("agent.log");
    let (agent, mode, log_arg) = (agent.to_owned(), mode.to_owned(), log.display().to_string());
    let adapters = AcpAdapters::builtin().register(
        Provider::Claude,
        AcpAdapter::new("fake-claude-acp", move |_| {
            vec![agent.clone(), mode.clone(), log_arg.clone()]
        })
        .oga_tools(true),
    );
    let dispatcher = Dispatcher::new(store.clone(), oga_runner::ProviderRunner::default())
        .with_acp_adapters(adapters);
    Harness {
        _directory: directory,
        cwd,
        log,
        store,
        dispatcher,
    }
}

/// Answers `opencode run` the way OpenCode's command line does, after noting
/// that it ran.
const OPENCODE_CLI: &str = r#"printf '%s\n' "$@" >> "$PWD/cli-ran"
printf '%s\n' '{"type":"step_start","sessionID":"ses_cli1"}' '{"type":"text","part":{"text":"ran on the command line\nOGA_RESULT: completed"}}'
"#;

/// An OpenCode profile on the adapters Oga ships, with `opencode` resolving to
/// a script: `opencode acp` starts the scripted agent in `mode`, and its
/// command line records that it ran. `without-acp` is an OpenCode with no ACP
/// server, and `missing` is no OpenCode at all.
fn opencode_harness(mode: &str) -> Harness {
    opencode_harness_on(Provider::OpenCode, mode)
}

/// The same for an OpenCode 2 profile, whose executable is `opencode2`.
fn opencode2_harness(mode: &str) -> Harness {
    opencode_harness_on(Provider::OpenCode2, mode)
}

fn opencode_harness_on(provider: Provider, mode: &str) -> Harness {
    let executable = if provider == Provider::OpenCode2 {
        "opencode2"
    } else {
        "opencode"
    };
    let directory = tempfile::tempdir().expect("temporary directory");
    let cwd = directory.path().join("project");
    let bin = directory.path().join("bin");
    fs::create_dir_all(cwd.join("src")).expect("project");
    fs::create_dir_all(&bin).expect("bin");
    let log = directory.path().join("agent.log");
    let acp = match mode {
        "without-acp" => "echo 'opencode: unknown command acp' >&2; exit 1".to_owned(),
        _ => format!(
            "exec '{}' '{mode}' '{}'",
            env!("CARGO_BIN_EXE_fake-acp-agent"),
            log.display()
        ),
    };
    if mode != "missing" {
        let cli = bin.join(executable);
        fs::write(
            &cli,
            format!("#!/bin/sh\nif [ \"$1\" = acp ]; then\n  {acp}\nfi\n{OPENCODE_CLI}"),
        )
        .expect("fake opencode");
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).expect("executable");
    }

    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
    let profile = Profile {
        id: "work".into(),
        label: "Work".into(),
        provider,
        default_model: "opencode/deep".into(),
        enabled: true,
        env: BTreeMap::from([
            ("PATH".into(), bin.display().to_string()),
            (
                "XDG_DATA_HOME".into(),
                directory.path().join("account").display().to_string(),
            ),
        ]),
        capabilities: vec![],
        command: None,
    };
    store
        .repositories()
        .profiles()
        .insert(&profile, NOW)
        .expect("profile");
    store
        .repositories()
        .settings()
        .put(
            &oga_config::canonical_cwd(oga_config::global_cwd())
                .display()
                .to_string(),
            oga_config::MODEL_SETTINGS_KEY,
            &serde_json::json!({"profiles": {"work": {"modelEnabled": {"opencode/deep": true}}}})
                .to_string(),
            NOW,
        )
        .expect("model settings");
    let dispatcher = Dispatcher::new(store.clone(), oga_runner::ProviderRunner::default());
    Harness {
        _directory: directory,
        cwd,
        log,
        store,
        dispatcher,
    }
}

impl Harness {
    async fn run(&self, prompt: &str) -> Task {
        self.run_with(DispatchRequest::new("work", prompt, &self.cwd))
            .await
    }

    async fn run_with(&self, request: DispatchRequest) -> Task {
        self.dispatcher
            .dispatch_and_wait(request)
            .await
            .expect("dispatched")
    }

    async fn settle(&self, id: &str) -> Task {
        for _ in 0..2_000 {
            let task = self.dispatcher.task(id).expect("task");
            if task.state.settled() || task.state == TaskState::Pending {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("task did not settle: {id}");
    }

    /// Every complete line the agent has logged. A line still being written
    /// while a turn runs is left for the next read.
    fn agent_log(&self) -> Vec<Value> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// Waits until the agent has been sent `method`, so a test can act on a
    /// turn while it is still running.
    async fn await_received(&self, method: &str) {
        for _ in 0..2_000 {
            if !self.received(method).is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the agent was never sent {method}");
    }

    fn received(&self, method: &str) -> Vec<Value> {
        self.agent_log()
            .into_iter()
            .filter(|entry| entry["received"]["method"] == method)
            .map(|entry| entry["received"]["params"].clone())
            .collect()
    }

    fn cli_runs(&self) -> Vec<String> {
        fs::read_to_string(self.cwd.join("cli-ran"))
            .map(|args| args.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    fn events(&self, id: &str) -> Vec<oga_domain::TaskEvent> {
        self.store.repositories().events().list(id).expect("events")
    }
}

fn transport(task: &Task) -> &TaskTransport {
    task.transport.as_ref().expect("transport recorded")
}

#[tokio::test]
async fn a_task_completes_a_full_turn_over_acp() {
    let harness = harness("turn");

    let task = harness.run("summarise the readme").await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert!(task.output.starts_with("Done."), "{}", task.output);
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Acp);
    assert_eq!(recorded.acp_session_id.as_deref(), Some("acp-session-1"));
    assert_eq!(recorded.restore, Some(AcpRestore::Resume));
    let agent = recorded.agent.as_ref().expect("agent identity");
    assert_eq!(agent.adapter, "fake-claude-acp");
    assert_eq!(agent.version.as_deref(), Some("2.1.0"));
    assert_eq!(
        task.session_id, None,
        "an ACP id is not a provider session a terminal could resume"
    );

    let prompts = harness.received("session/prompt");
    assert_eq!(prompts.len(), 1);
    let sessions = harness.received("session/new");
    let server = &sessions[0]["mcpServers"][0];
    assert_eq!(server["name"], "oga");
    assert_eq!(server["headers"][0]["name"], "x-oga-task-id");
    assert_eq!(server["headers"][0]["value"], task.id.as_str());
    let started = &harness.agent_log()[0]["env"];
    assert_eq!(started["OGA_TASK_ID"], task.id.as_str());
    assert!(
        started["CLAUDE_CONFIG_DIR"]
            .as_str()
            .is_some_and(|dir| dir.ends_with("/account")),
        "the agent runs in the profile's own account: {started}"
    );
    assert!(harness.cli_runs().is_empty());

    let events = harness.events(&task.id);
    let turn = harness
        .store
        .repositories()
        .turns()
        .list(&task.id)
        .expect("turns")
        .into_iter()
        .next()
        .expect("turn");
    let started = events
        .iter()
        .find(|event| event.kind == "started")
        .expect("started event");
    assert_eq!(started.turn_id, Some(turn.id));
    let completed = events
        .iter()
        .find(|event| event.kind == "completed")
        .expect("completed event");
    assert_eq!(completed.payload["completion"]["stopReason"], "end_turn");

    let messages: Vec<String> = events
        .iter()
        .filter(|event| event.kind == "agent.agent_message_chunk")
        .map(|event| {
            event.payload["content"]["text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        messages,
        ["Looking at the task", "Done.\nOGA_RESULT: completed"]
    );
}

#[tokio::test]
async fn a_session_total_is_charged_once_however_often_the_agent_reports_it() {
    let harness = harness("usage");

    let first = harness.run("edit the library").await;

    assert_eq!(first.state, TaskState::Completed, "{first:?}");
    assert_eq!(
        first.cost_usd,
        Some(1.25),
        "the freshest session total is the spend, not the sum of every reading"
    );
    assert!(!first.cost_usd_estimated);
    assert_eq!(
        first.turns, None,
        "ACP publishes no turn count, so it stays unknown"
    );
    let readings: Vec<(u64, f64)> = harness
        .events(&first.id)
        .iter()
        .filter(|event| event.kind == "agent.usage_update")
        .map(|event| {
            (
                event.payload["used"].as_u64().expect("a window fill"),
                event.payload["cost"]["amount"].as_f64().expect("a total"),
            )
        })
        .collect();
    assert_eq!(readings, [(12_000, 0.5), (24_000, 1.25)]);

    resume(
        &harness.dispatcher,
        ResumeRequest::new(&first.id).instruction("again"),
    )
    .await
    .expect("resumed");
    let second = harness.settle(&first.id).await;

    assert_eq!(second.state, TaskState::Completed, "{second:?}");
    assert_eq!(
        second.cost_usd,
        Some(2.5),
        "a second turn charges what the session grew by, never the whole total again"
    );
}

/// Recorded from an fx run: the agent retried a failing provider, gave up on a
/// usage limit, and answered `refusal`. Nothing refused anything, and the work
/// only needs the limit to clear.
#[tokio::test]
async fn a_provider_that_gave_up_on_a_usage_limit_settles_as_rate_limited() {
    let harness = harness("recovery");

    let dispatched = harness
        .dispatcher
        .dispatch(DispatchRequest::new(
            "work",
            "port the runner",
            &harness.cwd,
        ))
        .await
        .expect("dispatched");
    let task = harness.settle(&dispatched.task.id).await;

    assert_eq!(
        task.state,
        TaskState::Pending,
        "the run waits for the limit instead of dying: {task:?}"
    );
    let hold = oga_service::holds::get_hold(&harness.store, &task.id)
        .expect("a hold read")
        .expect("a hold");
    assert_eq!(hold.await_profile.as_deref(), Some("work"));
    assert!(
        hold.note.starts_with("Waiting for usage to reset"),
        "{}",
        hold.note
    );
    let completion = task.completion.as_ref().expect("a completion");
    assert_eq!(completion.code, CompletionCode::RateLimit);
    assert_eq!(
        completion.stop_reason.as_deref(),
        Some("refusal"),
        "what the agent said is still on the record"
    );
    let reason = completion.reason.as_deref().expect("a reason");
    assert!(
        reason.contains("rate limited") && reason.contains("rate_limit_exceeded"),
        "{reason}"
    );

    let rows: Vec<String> = harness
        .events(&task.id)
        .iter()
        .filter(|event| event.kind == "agent.session_info_update")
        .map(|event| oga_events::event_view(event, Provider::Claude))
        .map(|view| view.detail.unwrap_or(view.title))
        .collect();
    assert_eq!(
        rows,
        [
            "Provider unavailable · retrying, attempt 1 of 3",
            "Provider unavailable · retrying, attempt 2 of 3",
            "Provider unavailable · retrying, attempt 3 of 3",
            "Rate limited · stopped after 3 attempts",
        ],
        "every attempt reads as itself, and none of them says \"Session updated\""
    );
}

#[tokio::test]
async fn a_follow_up_resumes_the_same_acp_conversation() {
    let harness = harness("turn");
    let first = harness.run("start").await;

    resume(
        &harness.dispatcher,
        ResumeRequest::new(&first.id).instruction("now the tests"),
    )
    .await
    .expect("resumed");
    let second = harness.settle(&first.id).await;

    assert_eq!(second.state, TaskState::Completed, "{second:?}");
    assert_eq!(harness.received("session/new").len(), 1);
    let resumed = harness.received("session/resume");
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0]["sessionId"], "acp-session-1");
    let prompts = harness.received("session/prompt");
    assert_eq!(prompts.len(), 2);
    let text = prompts[1]["prompt"][0]["text"]
        .as_str()
        .expect("prompt text");
    assert!(text.contains("now the tests"), "{text}");
    assert!(
        !text.contains("start"),
        "a continuation sends only the instruction: {text}"
    );
}

#[tokio::test]
async fn a_disconnect_after_the_prompt_fails_and_is_never_run_again() {
    let harness = harness("exit-after-prompt");

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    let completion = task.completion.as_ref().expect("completion");
    assert_eq!(completion.code, CompletionCode::WorkerError);
    assert!(
        completion
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("didn't start it over")),
        "{completion:?}"
    );
    assert_eq!(transport(&task).kind, Transport::Acp);
    assert_eq!(harness.received("session/prompt").len(), 1);
    assert!(
        harness.cli_runs().is_empty(),
        "a prompt that may have run is never sent through the command line"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        harness.dispatcher.task(&task.id).expect("task").state,
        TaskState::Failed
    );
    assert_eq!(harness.received("session/prompt").len(), 1);
}

#[tokio::test]
async fn auto_falls_back_to_the_command_line_only_before_any_prompt_and_stays_there() {
    let harness = harness("no-http");

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert!(harness.received("session/prompt").is_empty());
    assert!(!harness.cli_runs().is_empty());
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Cli);
    assert_eq!(recorded.reason, Some(TransportReason::Unavailable));
    assert!(
        recorded
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("Oga's tools")),
        "{recorded:?}"
    );
    assert_eq!(task.session_id.as_deref(), Some("cli-session-1"));
    assert!(
        harness
            .events(&task.id)
            .iter()
            .any(|event| event.kind == "transport_fallback")
    );

    let started = harness.agent_log().len();
    resume(
        &harness.dispatcher,
        ResumeRequest::new(&task.id).instruction("keep going"),
    )
    .await
    .expect("resumed");
    let resumed = harness.settle(&task.id).await;

    assert_eq!(resumed.state, TaskState::Completed, "{resumed:?}");
    assert_eq!(
        harness.agent_log().len(),
        started,
        "a task that fell back never starts an ACP agent again"
    );
    assert!(
        harness
            .cli_runs()
            .windows(2)
            .any(|pair| pair == ["--resume", "cli-session-1"])
    );
}

#[tokio::test]
async fn a_profile_set_to_the_command_line_never_starts_an_agent() {
    let harness = harness("turn");
    set_transport_preference(&harness.store, "work", TransportPreference::Cli, NOW)
        .expect("preference");

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert_eq!(task.output.trim(), "ran on the command line");
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Cli);
    assert_eq!(recorded.reason, Some(TransportReason::Preference));
    assert!(
        harness.agent_log().is_empty(),
        "a profile asked for its command line never starts an agent to find out"
    );
}

#[tokio::test]
async fn explicit_acp_fails_visibly_when_the_agent_cannot_start() {
    let harness = harness_with("turn", "/nonexistent/acp-agent");
    set_transport_preference(&harness.store, "work", TransportPreference::Acp, NOW)
        .expect("preference");

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert!(
        task.error
            .as_deref()
            .is_some_and(|error| error.contains("couldn't connect over ACP")),
        "{task:?}"
    );
    assert!(harness.cli_runs().is_empty());
}

#[tokio::test]
async fn an_authentication_refusal_never_falls_back() {
    let harness = harness("auth");

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert_eq!(
        task.completion.as_ref().map(|completion| completion.code),
        Some(CompletionCode::Auth)
    );
    assert!(harness.cli_runs().is_empty());
    assert!(harness.received("session/prompt").is_empty());
}

#[tokio::test]
async fn a_task_from_before_acp_resumes_on_its_command_line() {
    let harness = harness("turn");
    let task = Task {
        id: "before-acp".into(),
        profile_id: "work".into(),
        model: "model-one".into(),
        prompt: "old work".into(),
        cwd: harness.cwd.display().to_string(),
        state: TaskState::Completed,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        scope: TaskScope {
            read: vec!["**".into()],
            write: vec!["**".into()],
        },
        session_id: Some("claude-old-session".into()),
        transport: Some(TaskTransport::cli(TransportReason::Legacy, None, NOW)),
        ..Task::default()
    };
    harness
        .store
        .repositories()
        .tasks()
        .insert(&task)
        .expect("task");

    resume(
        &harness.dispatcher,
        ResumeRequest::new("before-acp").instruction("one more thing"),
    )
    .await
    .expect("resumed");
    let resumed = harness.settle("before-acp").await;

    assert_eq!(resumed.state, TaskState::Completed, "{resumed:?}");
    assert!(harness.agent_log().is_empty(), "no ACP agent was started");
    assert!(
        harness
            .cli_runs()
            .windows(2)
            .any(|pair| pair == ["--resume", "claude-old-session"])
    );
    assert_eq!(transport(&resumed).reason, Some(TransportReason::Legacy));
}

#[tokio::test]
async fn a_loaded_conversation_is_not_counted_as_the_new_turn() {
    let harness = harness("load-only");
    let first = harness.run("start").await;
    assert_eq!(transport(&first).restore, Some(AcpRestore::Load));

    resume(
        &harness.dispatcher,
        ResumeRequest::new(&first.id).instruction("again"),
    )
    .await
    .expect("resumed");
    let second = harness.settle(&first.id).await;

    assert_eq!(second.state, TaskState::Completed, "{second:?}");
    assert_eq!(harness.received("session/load").len(), 1);
    let events = harness.events(&first.id);
    let reused = events
        .iter()
        .find(|event| event.kind == "session_reused")
        .expect("session reused");
    assert_eq!(reused.payload["replayedUpdates"], 3);
    assert!(
        events.iter().all(|event| {
            event
                .payload
                .get("content")
                .and_then(|content| content["text"].as_str())
                .is_none_or(|text| !text.contains("earlier"))
        }),
        "replayed history is never recorded as work"
    );
    assert!(!second.output.contains("earlier"), "{}", second.output);
}

#[tokio::test]
async fn permission_answers_follow_the_tasks_scope() {
    let harness = harness("permission");
    let request = DispatchRequest::new("work", "edit things", &harness.cwd).scope(TaskScope {
        read: vec!["**".into()],
        write: vec!["src/**".into()],
    });

    let task = harness.run_with(request).await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert!(
        task.output.contains("inside: allow, outside: reject"),
        "{}",
        task.output
    );
    let answers: Vec<bool> = harness
        .events(&task.id)
        .iter()
        .filter(|event| event.kind == "permission_answered")
        .map(|event| event.payload["allowed"].as_bool().expect("allowed"))
        .collect();
    assert_eq!(answers, [true, false]);
}

#[tokio::test]
async fn cancelling_stops_the_agent_and_settles_the_task() {
    let harness = harness("hang");
    let dispatched = harness
        .dispatcher
        .dispatch(DispatchRequest::new("work", "wait forever", &harness.cwd))
        .await
        .expect("dispatched");
    harness.await_received("session/prompt").await;

    cancel(&harness.dispatcher, CancelRequest::new(&dispatched.task.id))
        .await
        .expect("cancelled");
    let task = harness.settle(&dispatched.task.id).await;

    assert_eq!(task.state, TaskState::Cancelled, "{task:?}");
    let worker = harness
        .store
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT worker_json FROM tasks WHERE id=?",
                [&task.id],
                |row| row.get::<_, Option<String>>(0),
            )?)
        })
        .expect("worker column");
    let pid = worker
        .as_deref()
        .and_then(|json| serde_json::from_str::<Value>(json).ok())
        .and_then(|worker| worker["pid"].as_u64());
    if let Some(pid) = pid {
        for _ in 0..400 {
            if oga_runner::process_liveness(pid as u32) != oga_runner::Liveness::Alive {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_ne!(
            oga_runner::process_liveness(pid as u32),
            oga_runner::Liveness::Alive
        );
    }
    assert_eq!(harness.received("session/prompt").len(), 1);
}

#[tokio::test]
async fn an_instruction_reaches_the_turn_a_worker_is_already_running() {
    let harness = harness("steer");
    let dispatched = harness
        .dispatcher
        .dispatch(DispatchRequest::new(
            "work",
            "rename the field",
            &harness.cwd,
        ))
        .await
        .expect("dispatched");
    harness.await_received("session/prompt").await;

    let outcome = steer(
        &harness.dispatcher,
        SteerRequest::new(&dispatched.task.id).instruction("call it label instead"),
    )
    .await
    .expect("steered");

    assert!(
        !outcome.queued,
        "the instruction was handed over, not queued"
    );
    let task = harness.settle(&dispatched.task.id).await;
    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert!(
        task.output.contains("call it label instead"),
        "the running turn picked it up: {}",
        task.output
    );
    assert_eq!(
        transport(&task).steering,
        Some(AcpSteering::Extension),
        "what the agent offered is kept with the transport"
    );
    let handed = harness.received("_session/steering");
    assert_eq!(handed.len(), 1);
    assert_eq!(handed[0]["prompt"][0]["text"], "call it label instead");
    assert_eq!(
        handed[0]["_meta"]["steering"]["idleBehavior"], "promptRequired",
        "an idle agent must not start a turn nobody is waiting on"
    );
    assert_eq!(
        harness.received("session/prompt").len(),
        1,
        "the instruction joined the turn instead of starting another"
    );
    let kinds: Vec<String> = harness
        .events(&task.id)
        .into_iter()
        .map(|event| event.kind)
        .collect();
    assert!(kinds.iter().any(|kind| kind == "steered"), "{kinds:?}");
    assert!(
        !kinds.iter().any(|kind| kind == "follow_up_queued"),
        "{kinds:?}"
    );
}

#[tokio::test]
async fn a_worker_that_takes_no_instruction_mid_turn_still_queues_it() {
    let harness = harness("hang");
    let dispatched = harness
        .dispatcher
        .dispatch(DispatchRequest::new("work", "wait forever", &harness.cwd))
        .await
        .expect("dispatched");
    harness.await_received("session/prompt").await;

    let outcome = steer(
        &harness.dispatcher,
        SteerRequest::new(&dispatched.task.id).instruction("check the tests too"),
    )
    .await
    .expect("steered");

    assert!(outcome.queued, "it waits for this run to finish");
    assert_eq!(outcome.task.queued_follow_ups, Some(1));
    assert_eq!(
        transport(&harness.dispatcher.task(&dispatched.task.id).expect("task")).steering,
        None,
        "this agent never offered to take one"
    );
    assert!(
        harness.received("_session/steering").is_empty(),
        "an agent that did not offer it is never asked"
    );
    let queued = harness
        .events(&dispatched.task.id)
        .into_iter()
        .find(|event| event.kind == "follow_up_queued")
        .expect("queued event");
    assert_eq!(queued.payload["instruction"], "check the tests too");
    assert!(
        !queued.payload.contains_key("reason"),
        "nothing was tried, so there is nothing to explain"
    );

    cancel(&harness.dispatcher, CancelRequest::new(&dispatched.task.id))
        .await
        .expect("cancelled");
    harness.settle(&dispatched.task.id).await;
}

#[tokio::test]
async fn an_instruction_the_worker_will_not_take_falls_back_to_the_queue() {
    let harness = harness("steer-refused");
    let dispatched = harness
        .dispatcher
        .dispatch(DispatchRequest::new(
            "work",
            "rename the field",
            &harness.cwd,
        ))
        .await
        .expect("dispatched");
    harness.await_received("session/prompt").await;

    let outcome = steer(
        &harness.dispatcher,
        SteerRequest::new(&dispatched.task.id).instruction("call it label instead"),
    )
    .await
    .expect("steered");

    assert!(outcome.queued, "it fell back to waiting its turn");
    assert_eq!(harness.received("_session/steering").len(), 1);
    let queued = harness
        .events(&dispatched.task.id)
        .into_iter()
        .find(|event| event.kind == "follow_up_queued")
        .expect("queued event");
    assert_eq!(queued.payload["instruction"], "call it label instead");
    assert_eq!(
        queued.payload["reason"], "the run had already finished",
        "the record says why it had to wait"
    );
    let task = harness.dispatcher.task(&dispatched.task.id).expect("task");
    assert_eq!(
        transport(&task).steering,
        Some(AcpSteering::Extension),
        "the agent did offer to take one"
    );
}

#[tokio::test]
async fn an_instruction_joins_the_run_an_opencode_worker_is_already_on() {
    let harness = opencode_harness("opencode-join");
    let dispatched = harness
        .dispatcher
        .dispatch(DispatchRequest::new(
            "work",
            "rename the field",
            &harness.cwd,
        ))
        .await
        .expect("dispatched");
    harness.await_received("session/prompt").await;

    let outcome = steer(
        &harness.dispatcher,
        SteerRequest::new(&dispatched.task.id).instruction("call it label instead"),
    )
    .await
    .expect("steered");

    assert!(
        !outcome.queued,
        "the instruction was handed over, not queued"
    );
    let task = harness.settle(&dispatched.task.id).await;
    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert!(
        task.output.contains("call it label instead"),
        "the running turn picked it up: {}",
        task.output
    );
    assert_eq!(
        transport(&task).steering,
        Some(AcpSteering::Prompt),
        "OpenCode advertises nothing, so the adapter is what says it takes one"
    );
    let prompts = harness.received("session/prompt");
    assert_eq!(
        prompts.len(),
        2,
        "the instruction went over as a prompt of its own"
    );
    assert_eq!(prompts[1]["prompt"][0]["text"], "call it label instead");
    assert_eq!(
        prompts[1]["sessionId"], prompts[0]["sessionId"],
        "on the session the turn is already running in"
    );
    assert!(
        harness.received("_session/steering").is_empty(),
        "an agent that never offered the extension is not asked over it"
    );
    let kinds: Vec<String> = harness
        .events(&task.id)
        .into_iter()
        .map(|event| event.kind)
        .collect();
    assert!(kinds.iter().any(|kind| kind == "steered"), "{kinds:?}");
    assert!(
        !kinds.iter().any(|kind| kind == "follow_up_queued"),
        "{kinds:?}"
    );
}

#[tokio::test]
async fn a_joined_instruction_leaves_the_turn_settling_once() {
    let harness = opencode_harness("opencode-join");
    let dispatched = harness
        .dispatcher
        .dispatch(DispatchRequest::new(
            "work",
            "edit the library",
            &harness.cwd,
        ))
        .await
        .expect("dispatched");
    harness.await_received("session/prompt").await;
    steer(
        &harness.dispatcher,
        SteerRequest::new(&dispatched.task.id).instruction("call it label instead"),
    )
    .await
    .expect("steered");

    let task = harness.settle(&dispatched.task.id).await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    let events = harness.events(&task.id);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "completed")
            .count(),
        1,
        "both prompts answer for the one run they shared, which settles once"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "agent.usage_update")
            .count(),
        1
    );
    assert_eq!(
        task.cost_usd,
        Some(0.5),
        "the run is charged for once, not once per prompt"
    );
}

#[tokio::test]
async fn a_restart_picks_an_acp_conversation_back_up_instead_of_dropping_it() {
    let harness = harness("turn");
    let task = Task {
        id: "interrupted".into(),
        profile_id: "work".into(),
        model: "model-one".into(),
        prompt: "long work".into(),
        cwd: harness.cwd.display().to_string(),
        state: TaskState::Running,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        transport: Some(TaskTransport {
            kind: Transport::Acp,
            reason: None,
            detail: None,
            acp_session_id: Some("acp-session-1".into()),
            restore: Some(AcpRestore::Resume),
            agent: None,
            steering: None,
            decided_at: NOW.into(),
        }),
        ..Task::default()
    };
    harness
        .store
        .repositories()
        .tasks()
        .insert(&task)
        .expect("task");

    let report = harness
        .dispatcher
        .reconcile(ReconcileTrigger::BrokerStart)
        .expect("reconciled");

    assert_eq!(report.resumed, ["interrupted"]);
    assert!(report.stopped.is_empty());
    assert!(!Path::new(&harness.cwd.join("cli-ran")).exists());
}

#[tokio::test]
async fn a_turn_past_the_tasks_timeout_fails_as_a_timeout() {
    let harness = harness("hang");

    let task = harness
        .run_with(
            DispatchRequest::new("work", "wait forever", &harness.cwd)
                .timeout(Duration::from_millis(300)),
        )
        .await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert_eq!(
        task.completion.as_ref().map(|completion| completion.code),
        Some(CompletionCode::Timeout)
    );
    assert_eq!(harness.received("session/prompt").len(), 1);
    assert!(harness.cli_runs().is_empty());
}

#[tokio::test]
async fn a_handoff_to_another_account_decides_its_transport_again() {
    let harness = harness("exit-after-prompt");
    let first = harness.run("start").await;
    assert_eq!(first.state, TaskState::Failed, "{first:?}");
    let first_decided_at = transport(&first).decided_at.clone();

    let mut other = harness
        .store
        .repositories()
        .profiles()
        .get("work")
        .expect("profile")
        .expect("work profile");
    other.id = "other".into();
    harness
        .store
        .repositories()
        .profiles()
        .insert(&other, NOW)
        .expect("other profile");
    harness
        .store
        .repositories()
        .settings()
        .put(
            &oga_config::canonical_cwd(oga_config::global_cwd())
                .display()
                .to_string(),
            oga_config::MODEL_SETTINGS_KEY,
            &serde_json::json!({"profiles": {
                "work": {"modelEnabled": {"model-one": true}},
                "other": {"modelEnabled": {"model-one": true}},
            }})
            .to_string(),
            NOW,
        )
        .expect("model settings");

    oga_service::handoff(
        &harness.dispatcher,
        oga_service::HandoffRequest::new(first.id.clone()).profile("other"),
    )
    .await
    .expect("handed off");
    let moved = harness.settle(&first.id).await;

    assert_eq!(moved.state, TaskState::Failed, "{moved:?}");
    assert_eq!(moved.profile_id, "other");
    assert_ne!(
        transport(&moved).decided_at,
        first_decided_at,
        "a fresh session on a different account is a fresh decision, not the old account's"
    );
}

#[tokio::test]
async fn a_handoff_to_another_account_leaves_the_acp_conversation_behind() {
    let harness = harness("exit-after-prompt");
    let first = harness.run("start").await;
    assert_eq!(first.state, TaskState::Failed, "{first:?}");
    let mut other = harness
        .store
        .repositories()
        .profiles()
        .get("work")
        .expect("profile")
        .expect("work profile");
    other.id = "other".into();
    harness
        .store
        .repositories()
        .profiles()
        .insert(&other, NOW)
        .expect("other profile");
    harness
        .store
        .repositories()
        .settings()
        .put(
            &oga_config::canonical_cwd(oga_config::global_cwd())
                .display()
                .to_string(),
            oga_config::MODEL_SETTINGS_KEY,
            &serde_json::json!({"profiles": {
                "work": {"modelEnabled": {"model-one": true}},
                "other": {"modelEnabled": {"model-one": true}},
            }})
            .to_string(),
            NOW,
        )
        .expect("model settings");

    oga_service::handoff(
        &harness.dispatcher,
        oga_service::HandoffRequest::new(first.id.clone()).profile("other"),
    )
    .await
    .expect("handed off");
    let moved = harness.settle(&first.id).await;

    assert_eq!(moved.state, TaskState::Failed, "{moved:?}");
    assert_eq!(moved.profile_id, "other");
    assert_eq!(
        harness.received("session/new").len(),
        2,
        "another account opens its own conversation"
    );
    assert!(harness.received("session/resume").is_empty());
}

fn methods(harness: &Harness) -> Vec<String> {
    harness
        .agent_log()
        .iter()
        .filter_map(|entry| entry["received"]["method"].as_str())
        .map(str::to_owned)
        .collect()
}

fn chosen_settings(harness: &Harness) -> Vec<(String, String)> {
    harness
        .received("session/set_config_option")
        .iter()
        .map(|params| {
            (
                params["configId"].as_str().unwrap_or_default().to_owned(),
                params["value"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn opencode_runs_over_acp_by_default_with_its_model_effort_and_account() {
    let harness = opencode_harness("opencode");
    let mut request = DispatchRequest::new("work", "summarise the readme", &harness.cwd);
    request.effort = Some("high".into());

    let task = harness.run_with(request).await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Acp);
    assert_eq!(recorded.acp_session_id.as_deref(), Some("ses_acp1"));
    assert_eq!(
        recorded.agent.as_ref().map(|agent| agent.adapter.as_str()),
        Some("opencode-acp")
    );
    assert_eq!(
        task.session_id.as_deref(),
        Some("ses_acp1"),
        "OpenCode's ACP session is the session its terminal resumes"
    );
    assert_eq!(
        methods(&harness),
        [
            "initialize",
            "session/new",
            "session/set_config_option",
            "session/set_config_option",
            "session/prompt",
        ],
        "the model and effort are chosen before the prompt goes out"
    );
    assert_eq!(
        chosen_settings(&harness),
        [
            ("model".to_owned(), "opencode/deep".to_owned()),
            ("effort".to_owned(), "high".to_owned()),
        ]
    );
    assert_eq!(
        harness.received("session/new")[0]["mcpServers"],
        serde_json::json!([]),
        "OpenCode keeps its own MCP servers and, as on its command line, gets no Oga tools"
    );
    assert!(
        harness.agent_log()[0]["env"]["XDG_DATA_HOME"]
            .as_str()
            .is_some_and(|dir| dir.ends_with("/account")),
        "the agent runs in the profile's own account"
    );
    assert!(harness.cli_runs().is_empty());
}

#[tokio::test]
async fn opencode_without_an_acp_server_runs_on_its_command_line_before_any_prompt() {
    let harness = opencode_harness("without-acp");

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Cli);
    assert_eq!(recorded.reason, Some(TransportReason::Unavailable));
    assert_eq!(task.session_id.as_deref(), Some("ses_cli1"));
    let ran = harness.cli_runs();
    assert!(
        ran.windows(2)
            .any(|pair| pair == ["--model", "opencode/deep"]),
        "{ran:?}"
    );
    assert!(
        harness
            .events(&task.id)
            .iter()
            .any(|event| event.kind == "transport_fallback")
    );

    let missing = opencode_harness("missing");
    let task = missing.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Cli);
    assert_eq!(recorded.reason, Some(TransportReason::Unavailable));
    assert!(missing.agent_log().is_empty());
}

#[tokio::test]
async fn an_opencode_model_it_does_not_offer_never_reaches_the_prompt_or_the_command_line() {
    for preference in [TransportPreference::Auto, TransportPreference::Acp] {
        let harness = opencode_harness("opencode-no-model");
        set_transport_preference(&harness.store, "work", preference, NOW).expect("preference");

        let task = harness.run("do the work").await;

        let case = preference.as_str();
        assert_eq!(task.state, TaskState::Failed, "{case}: {task:?}");
        assert!(
            task.error
                .as_deref()
                .is_some_and(|error| error.contains("doesn't offer opencode/deep as a model")),
            "{case}: {task:?}"
        );
        assert!(harness.received("session/prompt").is_empty(), "{case}");
        assert!(chosen_settings(&harness).is_empty(), "{case}");
        assert!(
            harness.cli_runs().is_empty(),
            "{case}: an agent Oga reached is never stepped around through the command line"
        );
    }
}

#[tokio::test]
async fn an_opencode_follow_up_continues_its_session_and_a_lost_prompt_is_never_rerun() {
    let harness = opencode_harness("opencode");
    let first = harness.run("start").await;

    resume(
        &harness.dispatcher,
        ResumeRequest::new(&first.id).instruction("now the tests"),
    )
    .await
    .expect("resumed");
    let second = harness.settle(&first.id).await;

    assert_eq!(second.state, TaskState::Completed, "{second:?}");
    let resumed = harness.received("session/resume");
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0]["sessionId"], "ses_acp1");
    assert_eq!(harness.received("session/new").len(), 1);
    assert_eq!(harness.received("session/prompt").len(), 2);
    assert_eq!(
        chosen_settings(&harness)
            .iter()
            .filter(|(id, _)| id == "model")
            .count(),
        2,
        "a resumed session is given the task's model again"
    );
    assert!(harness.cli_runs().is_empty());

    let lost = opencode_harness("opencode-exit-after-prompt");
    let task = lost.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert_eq!(lost.received("session/prompt").len(), 1);
    assert!(
        lost.cli_runs().is_empty(),
        "a prompt that may have run is never sent through the command line"
    );
}

#[tokio::test]
async fn only_opencode_itself_gives_a_task_a_terminal_session() {
    let harness = opencode_harness("opencode-renamed");

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert_eq!(transport(&task).acp_session_id.as_deref(), Some("ses_acp1"));
    assert_eq!(
        task.session_id, None,
        "an agent that does not report itself as OpenCode has no session OpenCode can resume"
    );
}

#[tokio::test]
async fn an_opencode_task_from_before_acp_resumes_on_its_command_line() {
    let harness = opencode_harness("opencode");
    let task = Task {
        id: "before-acp".into(),
        profile_id: "work".into(),
        model: "opencode/deep".into(),
        prompt: "old work".into(),
        cwd: harness.cwd.display().to_string(),
        state: TaskState::Completed,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        scope: TaskScope {
            read: vec!["**".into()],
            write: vec!["**".into()],
        },
        session_id: Some("ses_old".into()),
        ..Task::default()
    };
    harness
        .store
        .repositories()
        .tasks()
        .insert(&task)
        .expect("task");

    resume(
        &harness.dispatcher,
        ResumeRequest::new("before-acp").instruction("one more thing"),
    )
    .await
    .expect("resumed");
    let resumed = harness.settle("before-acp").await;

    assert_eq!(resumed.state, TaskState::Completed, "{resumed:?}");
    assert!(harness.agent_log().is_empty(), "no ACP agent was started");
    assert!(
        harness
            .cli_runs()
            .windows(2)
            .any(|pair| pair == ["--session", "ses_old"])
    );
    assert_eq!(transport(&resumed).reason, Some(TransportReason::Legacy));
}

#[tokio::test]
async fn opencode2_runs_over_acp_by_default_with_its_model_effort_and_account() {
    let harness = opencode2_harness("opencode2");
    let mut request = DispatchRequest::new("work", "summarise the readme", &harness.cwd);
    request.effort = Some("high".into());

    let task = harness.run_with(request).await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Acp);
    let agent = recorded.agent.as_ref().expect("agent identity");
    assert_eq!(agent.adapter, "opencode2-acp");
    assert_eq!(agent.version.as_deref(), Some("0.0.0-beta-18999"));
    assert_eq!(recorded.acp_session_id.as_deref(), Some("ses_acp1"));
    assert_eq!(
        task.session_id.as_deref(),
        Some("ses_acp1"),
        "the verified build's ACP session is the session `opencode2 --session` continues"
    );
    assert_eq!(
        methods(&harness),
        [
            "initialize",
            "session/new",
            "session/set_config_option",
            "session/set_config_option",
            "session/prompt",
        ],
        "the model and effort are chosen before the prompt goes out"
    );
    assert_eq!(
        chosen_settings(&harness),
        [
            ("model".to_owned(), "opencode/deep".to_owned()),
            ("effort".to_owned(), "high".to_owned()),
        ],
        "the two halves of --model opencode/deep#high"
    );
    let opened = &harness.received("session/new")[0];
    assert_eq!(
        opened["cwd"], task.cwd,
        "the session opens in the task's directory"
    );
    assert_eq!(
        opened["mcpServers"],
        serde_json::json!([]),
        "as on its command line, OpenCode 2 gets no Oga tools"
    );
    assert!(
        harness.agent_log()[0]["env"]["XDG_DATA_HOME"]
            .as_str()
            .is_some_and(|dir| dir.ends_with("/account")),
        "the agent runs in the profile's own account"
    );
    assert!(harness.cli_runs().is_empty());
}

#[tokio::test]
async fn opencode2_without_acp_runs_on_its_command_line_before_any_prompt() {
    let harness = opencode2_harness("without-acp");
    let mut request = DispatchRequest::new("work", "do the work", &harness.cwd);
    request.effort = Some("high".into());

    let task = harness.run_with(request).await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Cli);
    assert_eq!(recorded.reason, Some(TransportReason::Unavailable));
    assert_eq!(task.session_id.as_deref(), Some("ses_cli1"));
    let ran = harness.cli_runs();
    assert!(
        ran.windows(2)
            .any(|pair| pair == ["--model", "opencode/deep#high"]),
        "{ran:?}"
    );
    assert!(
        harness
            .events(&task.id)
            .iter()
            .any(|event| event.kind == "transport_fallback")
    );

    let missing = opencode2_harness("missing");
    let task = missing.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Cli);
    assert_eq!(recorded.reason, Some(TransportReason::Unavailable));
    assert!(missing.agent_log().is_empty());
}

#[tokio::test]
async fn an_opencode2_build_oga_was_not_verified_against_never_opens_a_session() {
    // `opencode` is an OpenCode 1 release answering as `opencode2`, under the
    // same name.
    for mode in ["opencode2-next", "opencode", "opencode2-renamed"] {
        let harness = opencode2_harness(mode);

        let task = harness.run("do the work").await;

        assert_eq!(task.state, TaskState::Completed, "{mode}: {task:?}");
        let recorded = transport(&task);
        assert_eq!(recorded.kind, Transport::Cli, "{mode}");
        assert!(
            recorded
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("OpenCode 0.0.0-beta-18999")),
            "{mode}: {recorded:?}"
        );
        assert_eq!(methods(&harness), ["initialize"], "{mode}");
        assert_eq!(task.session_id.as_deref(), Some("ses_cli1"), "{mode}");
        assert!(!harness.cli_runs().is_empty(), "{mode}");
    }

    let explicit = opencode2_harness("opencode2-next");
    set_transport_preference(&explicit.store, "work", TransportPreference::Acp, NOW)
        .expect("preference");
    let task = explicit.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert!(
        task.error
            .as_deref()
            .is_some_and(|error| error.contains("not OpenCode 0.0.0-beta-19000")),
        "{task:?}"
    );
    assert_eq!(methods(&explicit), ["initialize"]);
    assert!(explicit.cli_runs().is_empty());
}

#[tokio::test]
async fn an_opencode2_follow_up_continues_its_session_and_a_lost_prompt_is_never_rerun() {
    let harness = opencode2_harness("opencode2");
    let first = harness.run("start").await;

    resume(
        &harness.dispatcher,
        ResumeRequest::new(&first.id).instruction("now the tests"),
    )
    .await
    .expect("resumed");
    let second = harness.settle(&first.id).await;

    assert_eq!(second.state, TaskState::Completed, "{second:?}");
    let resumed = harness.received("session/resume");
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0]["sessionId"], "ses_acp1");
    assert_eq!(harness.received("session/new").len(), 1);
    assert_eq!(harness.received("session/prompt").len(), 2);
    assert_eq!(
        chosen_settings(&harness)
            .iter()
            .filter(|(id, _)| id == "model")
            .count(),
        2,
        "a resumed session is given the task's model again"
    );
    assert!(harness.cli_runs().is_empty());

    let lost = opencode2_harness("opencode2-exit-after-prompt");
    let task = lost.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert_eq!(lost.received("session/prompt").len(), 1);
    assert!(
        lost.cli_runs().is_empty(),
        "a prompt that may have run is never sent through the command line"
    );
}

#[tokio::test]
async fn an_opencode2_task_from_before_acp_resumes_on_its_command_line() {
    let harness = opencode2_harness("opencode2");
    let task = Task {
        id: "before-acp".into(),
        profile_id: "work".into(),
        model: "opencode/deep".into(),
        prompt: "old work".into(),
        cwd: harness.cwd.display().to_string(),
        state: TaskState::Completed,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        scope: TaskScope {
            read: vec!["**".into()],
            write: vec!["**".into()],
        },
        session_id: Some("ses_old".into()),
        ..Task::default()
    };
    harness
        .store
        .repositories()
        .tasks()
        .insert(&task)
        .expect("task");

    resume(
        &harness.dispatcher,
        ResumeRequest::new("before-acp").instruction("one more thing"),
    )
    .await
    .expect("resumed");
    let resumed = harness.settle("before-acp").await;

    assert_eq!(resumed.state, TaskState::Completed, "{resumed:?}");
    assert!(harness.agent_log().is_empty(), "no ACP agent was started");
    assert!(
        harness
            .cli_runs()
            .windows(2)
            .any(|pair| pair == ["--session", "ses_old"])
    );
    assert_eq!(transport(&resumed).reason, Some(TransportReason::Legacy));
}

/// Answers `claude -p` the way Claude Code's command line does, after noting
/// that it ran.
const CLAUDE_CLI: &str = r#"printf '%s\n' "$@" >> "$PWD/cli-ran"
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"ran on the command line\nOGA_RESULT: completed","session_id":"cli-session-1"}'
"#;

/// A Claude profile on the adapters Oga ships, with `claude-agent-acp`
/// resolving to the scripted agent in `mode` and `claude` to a command line
/// that records that it ran. `missing` is an account with no adapter installed.
fn claude_harness(mode: &str) -> Harness {
    let directory = tempfile::tempdir().expect("temporary directory");
    let cwd = directory.path().join("project");
    let bin = directory.path().join("bin");
    let account = directory.path().join("account");
    fs::create_dir_all(cwd.join("src")).expect("project");
    fs::create_dir_all(account.join("skills")).expect("skills");
    fs::create_dir_all(&bin).expect("bin");
    let log = directory.path().join("agent.log");
    if mode != "missing" {
        let adapter = bin.join("claude-agent-acp");
        fs::write(
            &adapter,
            format!(
                "#!/bin/sh\nexec '{}' '{mode}' '{}'\n",
                env!("CARGO_BIN_EXE_fake-acp-agent"),
                log.display()
            ),
        )
        .expect("fake adapter");
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).expect("executable");
    }
    let cli = bin.join("claude");
    fs::write(&cli, format!("#!/bin/sh\n{CLAUDE_CLI}")).expect("fake command line");
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).expect("executable");

    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
    let profile = Profile {
        id: "work".into(),
        label: "Work".into(),
        provider: Provider::Claude,
        default_model: "claude-opus-4-5".into(),
        enabled: true,
        env: BTreeMap::from([
            ("PATH".into(), bin.display().to_string()),
            ("CLAUDE_CONFIG_DIR".into(), account.display().to_string()),
        ]),
        capabilities: vec![],
        command: None,
    };
    store
        .repositories()
        .profiles()
        .insert(&profile, NOW)
        .expect("profile");
    store
        .repositories()
        .settings()
        .put(
            &oga_config::canonical_cwd(oga_config::global_cwd())
                .display()
                .to_string(),
            oga_config::MODEL_SETTINGS_KEY,
            &serde_json::json!({"profiles": {"work": {"modelEnabled": {"claude-opus-4-5": true}}}})
                .to_string(),
            NOW,
        )
        .expect("model settings");
    let dispatcher = Dispatcher::new(store.clone(), oga_runner::ProviderRunner::default());
    Harness {
        _directory: directory,
        cwd,
        log,
        store,
        dispatcher,
    }
}

#[tokio::test]
async fn claude_runs_over_acp_by_default_with_its_model_effort_account_and_tools() {
    let harness = claude_harness("claude");
    let mut request = DispatchRequest::new("work", "summarise the readme", &harness.cwd);
    request.effort = Some("high".into());

    let task = harness.run_with(request).await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Acp);
    assert_eq!(
        recorded.agent.as_ref().map(|agent| agent.adapter.as_str()),
        Some("claude-agent-acp")
    );
    assert_eq!(
        task.session_id.as_deref(),
        recorded.acp_session_id.as_deref(),
        "the adapter opens a Claude Code session, so a terminal can resume it"
    );
    assert_eq!(
        methods(&harness),
        [
            "initialize",
            "session/new",
            "session/set_config_option",
            "session/prompt",
        ],
        "the effort is chosen before the prompt goes out"
    );
    assert_eq!(
        chosen_settings(&harness),
        [("effort".to_owned(), "high".to_owned())],
        "the model rides the environment, not the session's own aliases"
    );
    let opened = &harness.received("session/new")[0];
    assert_eq!(
        opened["mcpServers"][0]["name"], "oga",
        "the tools --mcp-config carries reach the session"
    );
    assert_eq!(
        opened["mcpServers"][0]["headers"][0]["value"], task.id,
        "Oga's tools answer as this task"
    );
    assert!(
        opened["additionalDirectories"][0]
            .as_str()
            .is_some_and(|dir| dir.ends_with("/account/skills")),
        "the skills --add-dir names are opened as workspace roots: {opened}"
    );
    let started = &harness.agent_log()[0]["env"];
    assert_eq!(started["ANTHROPIC_MODEL"], "claude-opus-4-5");
    assert!(
        started["CLAUDE_CONFIG_DIR"]
            .as_str()
            .is_some_and(|dir| dir.ends_with("/account")),
        "the agent runs in the profile's own account"
    );
    assert!(harness.cli_runs().is_empty());
}

#[tokio::test]
async fn claude_without_its_adapter_installed_runs_on_its_command_line_before_any_prompt() {
    let harness = claude_harness("missing");

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Cli);
    assert_eq!(recorded.reason, Some(TransportReason::Unavailable));
    assert_eq!(task.session_id.as_deref(), Some("cli-session-1"));
    assert!(harness.agent_log().is_empty());
    let ran = harness.cli_runs();
    assert!(
        ran.windows(2)
            .any(|pair| pair == ["--model", "claude-opus-4-5"]),
        "{ran:?}"
    );
    assert!(
        harness
            .events(&task.id)
            .iter()
            .any(|event| event.kind == "transport_fallback")
    );
}

#[tokio::test]
async fn a_claude_effort_it_cannot_offer_never_reaches_the_prompt_or_the_command_line() {
    for preference in [TransportPreference::Auto, TransportPreference::Acp] {
        let harness = claude_harness("claude-no-effort");
        set_transport_preference(&harness.store, "work", preference, NOW).expect("preference");
        let mut request = DispatchRequest::new("work", "do the work", &harness.cwd);
        request.effort = Some("high".into());

        let task = harness.run_with(request).await;

        let case = preference.as_str();
        assert_eq!(task.state, TaskState::Failed, "{case}: {task:?}");
        assert!(
            task.error
                .as_deref()
                .is_some_and(|error| error.contains("can't use this task's model or effort")),
            "{case}: {task:?}"
        );
        assert!(harness.received("session/prompt").is_empty(), "{case}");
        assert!(
            harness.cli_runs().is_empty(),
            "{case}: an agent Oga reached is never stepped around through the command line"
        );
    }
}

#[tokio::test]
async fn only_claudes_own_adapter_gives_a_task_a_terminal_session() {
    let harness = claude_harness("claude-renamed");

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert!(transport(&task).acp_session_id.is_some());
    assert_eq!(
        task.session_id, None,
        "another agent behind the same command names no session Claude Code can resume"
    );
}

#[tokio::test]
async fn a_claude_follow_up_continues_its_session_and_a_lost_prompt_is_never_rerun() {
    let harness = claude_harness("claude");
    let first = harness.run("start").await;

    resume(
        &harness.dispatcher,
        ResumeRequest::new(&first.id).instruction("now the tests"),
    )
    .await
    .expect("resumed");
    let second = harness.settle(&first.id).await;

    assert_eq!(second.state, TaskState::Completed, "{second:?}");
    let resumed = harness.received("session/resume");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        resumed[0]["sessionId"].as_str(),
        transport(&second).acp_session_id.as_deref()
    );
    assert_eq!(harness.received("session/new").len(), 1);
    assert_eq!(harness.received("session/prompt").len(), 2);
    assert!(harness.cli_runs().is_empty());

    let lost = claude_harness("claude-exit-after-prompt");
    let task = lost.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert_eq!(lost.received("session/prompt").len(), 1);
    assert!(
        lost.cli_runs().is_empty(),
        "a prompt that may have run is never sent through the command line"
    );
}

#[tokio::test]
async fn a_claude_task_from_before_acp_resumes_on_its_command_line() {
    let harness = claude_harness("claude");
    let task = Task {
        id: "before-acp".into(),
        profile_id: "work".into(),
        model: "claude-opus-4-5".into(),
        prompt: "old work".into(),
        cwd: harness.cwd.display().to_string(),
        state: TaskState::Completed,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        scope: TaskScope {
            read: vec!["**".into()],
            write: vec!["**".into()],
        },
        session_id: Some("claude-session-old".into()),
        ..Task::default()
    };
    harness
        .store
        .repositories()
        .tasks()
        .insert(&task)
        .expect("task");

    resume(
        &harness.dispatcher,
        ResumeRequest::new("before-acp").instruction("one more thing"),
    )
    .await
    .expect("resumed");
    let resumed = harness.settle("before-acp").await;

    assert_eq!(resumed.state, TaskState::Completed, "{resumed:?}");
    assert!(harness.agent_log().is_empty(), "no ACP agent was started");
    assert!(
        harness
            .cli_runs()
            .windows(2)
            .any(|pair| pair == ["--resume", "claude-session-old"])
    );
    assert_eq!(transport(&resumed).reason, Some(TransportReason::Legacy));
}

/// Answers `codex exec` the way Codex's command line does, after noting that it
/// ran.
const CODEX_CLI: &str = r#"printf '%s\n' "$@" >> "$PWD/cli-ran"
printf '%s\n' '{"type":"thread.started","thread_id":"019a4c1e-0000-7000-8000-00000000c11a"}' '{"type":"item.completed","item":{"type":"agent_message","text":"ran on the command line\nOGA_RESULT: completed"}}'
"#;

/// A Codex profile on the adapters Oga ships, with `codex-acp` resolving to the
/// scripted agent in `mode` and `codex` to a command line that records that it
/// ran. `missing` is an account with no adapter installed. The profile names its
/// own `CODEX_API_KEY`, blank unless a test signs in with one, so a key the test
/// process inherits never decides the transport.
fn codex_harness(mode: &str, api_key: &str) -> Harness {
    let directory = tempfile::tempdir().expect("temporary directory");
    let cwd = directory.path().join("project");
    let bin = directory.path().join("bin");
    let account = directory.path().join("account");
    fs::create_dir_all(cwd.join("src")).expect("project");
    fs::create_dir_all(&account).expect("account");
    fs::create_dir_all(&bin).expect("bin");
    let log = directory.path().join("agent.log");
    if mode != "missing" {
        let adapter = bin.join("codex-acp");
        fs::write(
            &adapter,
            format!(
                "#!/bin/sh\nexec '{}' '{mode}' '{}'\n",
                env!("CARGO_BIN_EXE_fake-acp-agent"),
                log.display()
            ),
        )
        .expect("fake adapter");
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).expect("executable");
    }
    let cli = bin.join("codex");
    fs::write(&cli, format!("#!/bin/sh\n{CODEX_CLI}")).expect("fake command line");
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).expect("executable");

    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
    let profile = Profile {
        id: "work".into(),
        label: "Work".into(),
        provider: Provider::Codex,
        default_model: "gpt-5.5".into(),
        enabled: true,
        env: BTreeMap::from([
            ("PATH".into(), bin.display().to_string()),
            ("CODEX_HOME".into(), account.display().to_string()),
            ("CODEX_API_KEY".into(), api_key.into()),
        ]),
        capabilities: vec![],
        command: None,
    };
    store
        .repositories()
        .profiles()
        .insert(&profile, NOW)
        .expect("profile");
    store
        .repositories()
        .settings()
        .put(
            &oga_config::canonical_cwd(oga_config::global_cwd())
                .display()
                .to_string(),
            oga_config::MODEL_SETTINGS_KEY,
            &serde_json::json!({"profiles": {"work": {"modelEnabled": {"gpt-5.5": true}}}})
                .to_string(),
            NOW,
        )
        .expect("model settings");
    let dispatcher = Dispatcher::new(store.clone(), oga_runner::ProviderRunner::default());
    Harness {
        _directory: directory,
        cwd,
        log,
        store,
        dispatcher,
    }
}

#[tokio::test]
async fn codex_runs_over_acp_by_default_with_its_model_effort_account_and_tools() {
    let harness = codex_harness("codex", "");
    let mut request = DispatchRequest::new("work", "summarise the readme", &harness.cwd);
    request.effort = Some("high".into());

    let task = harness.run_with(request).await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Acp);
    let agent = recorded.agent.as_ref().expect("agent identity");
    assert_eq!(agent.adapter, "codex-acp");
    assert_eq!(agent.version.as_deref(), Some("1.12.0"));
    assert_eq!(
        task.session_id.as_deref(),
        recorded.acp_session_id.as_deref(),
        "the adapter opens a Codex thread, so a terminal can resume it"
    );
    assert_eq!(
        methods(&harness),
        [
            "initialize",
            "session/new",
            "session/set_config_option",
            "session/prompt",
        ],
        "full access is chosen before the prompt goes out"
    );
    assert_eq!(
        chosen_settings(&harness),
        [("mode".to_owned(), "agent-full-access".to_owned())],
        "the session already holds the model and effort the launch's config named"
    );
    let opened = &harness.received("session/new")[0];
    assert_eq!(
        opened["mcpServers"][0]["name"], "oga",
        "the tools -c mcp_servers.oga carries reach the session"
    );
    assert_eq!(
        opened["mcpServers"][0]["headers"][0]["value"], task.id,
        "Oga's tools answer as this task"
    );
    let started = &harness.agent_log()[0]["env"];
    assert!(
        started["CODEX_HOME"]
            .as_str()
            .is_some_and(|dir| dir.ends_with("/account")),
        "the agent runs in the profile's own account: {started}"
    );
    let config: Value =
        serde_json::from_str(started["CODEX_CONFIG"].as_str().expect("config overrides"))
            .expect("config is JSON");
    assert_eq!(
        config,
        serde_json::json!({"model": "gpt-5.5", "model_reasoning_effort": "high"})
    );
    assert_eq!(started["DISABLE_MCP_CONFIG_FILTERING"], "true");
    assert!(harness.cli_runs().is_empty());
}

#[tokio::test]
async fn codex_without_its_adapter_installed_runs_on_its_command_line_before_any_prompt() {
    let harness = codex_harness("missing", "");

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Cli);
    assert_eq!(recorded.reason, Some(TransportReason::Unavailable));
    assert_eq!(
        task.session_id.as_deref(),
        Some("019a4c1e-0000-7000-8000-00000000c11a")
    );
    assert!(harness.agent_log().is_empty());
    let ran = harness.cli_runs();
    assert!(
        ran.windows(2).any(|pair| pair == ["--model", "gpt-5.5"]),
        "{ran:?}"
    );
    assert!(
        harness
            .events(&task.id)
            .iter()
            .any(|event| event.kind == "transport_fallback")
    );
}

#[tokio::test]
async fn a_codex_adapter_oga_was_not_verified_against_never_opens_a_session() {
    for mode in ["codex-next", "codex-renamed"] {
        let harness = codex_harness(mode, "");

        let task = harness.run("do the work").await;

        assert_eq!(task.state, TaskState::Completed, "{mode}: {task:?}");
        let recorded = transport(&task);
        assert_eq!(recorded.kind, Transport::Cli, "{mode}");
        assert!(
            recorded
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("@agentclientprotocol/codex-acp 1.12.x")),
            "{mode}: {recorded:?}"
        );
        assert_eq!(methods(&harness), ["initialize"], "{mode}");
        assert!(!harness.cli_runs().is_empty(), "{mode}");
    }

    let explicit = codex_harness("codex-next", "");
    set_transport_preference(&explicit.store, "work", TransportPreference::Acp, NOW)
        .expect("preference");
    let task = explicit.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert!(
        task.error
            .as_deref()
            .is_some_and(|error| error.contains("not @agentclientprotocol/codex-acp 1.13.0")),
        "{task:?}"
    );
    assert_eq!(methods(&explicit), ["initialize"]);
    assert!(explicit.cli_runs().is_empty());
}

#[tokio::test]
async fn a_codex_account_that_signs_in_with_an_api_key_keeps_to_its_command_line() {
    let harness = codex_harness("codex", "sk-test");

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Cli);
    assert_eq!(recorded.reason, Some(TransportReason::Unavailable));
    assert!(
        recorded
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("CODEX_API_KEY")),
        "{recorded:?}"
    );
    assert!(harness.agent_log().is_empty(), "the adapter never started");
    assert!(!harness.cli_runs().is_empty());

    let explicit = codex_harness("codex", "sk-test");
    set_transport_preference(&explicit.store, "work", TransportPreference::Acp, NOW)
        .expect("preference");
    let task = explicit.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert!(
        task.error
            .as_deref()
            .is_some_and(|error| error.contains("CODEX_API_KEY")),
        "{task:?}"
    );
    assert!(explicit.agent_log().is_empty());
    assert!(explicit.cli_runs().is_empty());
}

#[tokio::test]
async fn a_codex_adapter_without_full_access_never_reaches_the_prompt_or_the_command_line() {
    let harness = codex_harness("codex-no-full-access", "");

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert!(
        task.error
            .as_deref()
            .is_some_and(|error| error.contains("agent-full-access")),
        "{task:?}"
    );
    assert!(harness.received("session/prompt").is_empty());
    assert!(
        harness.cli_runs().is_empty(),
        "an agent Oga reached is never stepped around through the command line"
    );
}

#[tokio::test]
async fn a_codex_follow_up_continues_its_thread_and_a_lost_prompt_is_never_rerun() {
    let harness = codex_harness("codex", "");
    let first = harness.run("start").await;

    resume(
        &harness.dispatcher,
        ResumeRequest::new(&first.id).instruction("now the tests"),
    )
    .await
    .expect("resumed");
    let second = harness.settle(&first.id).await;

    assert_eq!(second.state, TaskState::Completed, "{second:?}");
    let resumed = harness.received("session/resume");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        resumed[0]["sessionId"].as_str(),
        second.session_id.as_deref(),
        "the thread a terminal would resume is the one the follow-up continues"
    );
    assert_eq!(harness.received("session/new").len(), 1);
    assert_eq!(harness.received("session/prompt").len(), 2);
    assert_eq!(
        chosen_settings(&harness),
        [
            ("mode".to_owned(), "agent-full-access".to_owned()),
            ("mode".to_owned(), "agent-full-access".to_owned()),
        ],
        "a restored session is given full access again before its prompt"
    );
    assert!(harness.cli_runs().is_empty());

    let lost = codex_harness("codex-exit-after-prompt", "");
    let task = lost.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert_eq!(lost.received("session/prompt").len(), 1);
    assert!(
        lost.cli_runs().is_empty(),
        "a prompt that may have run is never sent through the command line"
    );
}

#[tokio::test]
async fn a_codex_task_from_before_acp_resumes_on_its_command_line() {
    let harness = codex_harness("codex", "");
    let task = Task {
        id: "before-acp".into(),
        profile_id: "work".into(),
        model: "gpt-5.5".into(),
        prompt: "old work".into(),
        cwd: harness.cwd.display().to_string(),
        state: TaskState::Completed,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        scope: TaskScope {
            read: vec!["**".into()],
            write: vec!["**".into()],
        },
        session_id: Some("019a0000-0000-7000-8000-0000000001d0".into()),
        ..Task::default()
    };
    harness
        .store
        .repositories()
        .tasks()
        .insert(&task)
        .expect("task");

    resume(
        &harness.dispatcher,
        ResumeRequest::new("before-acp").instruction("one more thing"),
    )
    .await
    .expect("resumed");
    let resumed = harness.settle("before-acp").await;

    assert_eq!(resumed.state, TaskState::Completed, "{resumed:?}");
    assert!(harness.agent_log().is_empty(), "no ACP agent was started");
    assert!(
        harness
            .cli_runs()
            .windows(2)
            .any(|pair| pair == ["resume", "019a0000-0000-7000-8000-0000000001d0"])
    );
    assert_eq!(transport(&resumed).reason, Some(TransportReason::Legacy));
}

/// Answers `agy --print` the way Antigravity's command line does, after noting
/// that it ran.
const ANTIGRAVITY_CLI: &str = r#"printf '%s\n' "$@" >> "$PWD/cli-ran"
printf '%s\n' '{"event":"init","conversation_id":"c0ffee00-0000-4000-8000-00000000c11a"}' '{"event":"result","result":{"response":"ran on the command line\nOGA_RESULT: completed","conversation_id":"c0ffee00-0000-4000-8000-00000000c11a"}}'
"#;

/// A Google sign-in the ACP server keeps in its own Gemini home.
const GOOGLE_SIGN_IN: &str = r#"{"auth": {"type": "oauth-personal"}}"#;

/// An Antigravity profile on the adapters Oga ships, with its own Gemini home,
/// `agy_acp_server.par` resolving to the scripted server in `mode`, and `agy`
/// to a command line that records that it ran. `missing` is an account with no
/// ACP server installed. `sign_in` is the server's `settings.json`, saved with a
/// token when it names a Google sign-in; `None` is a server nobody signed in to.
fn antigravity_harness(mode: &str, sign_in: Option<&str>) -> Harness {
    let directory = tempfile::tempdir().expect("temporary directory");
    let cwd = directory.path().join("project");
    let bin = directory.path().join("bin");
    let server_home = directory.path().join("gemini").join("antigravity-acp");
    fs::create_dir_all(cwd.join("src")).expect("project");
    fs::create_dir_all(&server_home).expect("gemini home");
    fs::create_dir_all(&bin).expect("bin");
    if let Some(settings) = sign_in {
        fs::write(server_home.join("settings.json"), settings).expect("settings");
        fs::write(server_home.join("acp_token.json"), "{}").expect("token");
    }
    let log = directory.path().join("agent.log");
    if mode != "missing" {
        let server = bin.join("agy_acp_server.par");
        fs::write(
            &server,
            format!(
                "#!/bin/sh\nexec '{}' '{mode}' '{}'\n",
                env!("CARGO_BIN_EXE_fake-acp-agent"),
                log.display()
            ),
        )
        .expect("fake server");
        fs::set_permissions(&server, fs::Permissions::from_mode(0o755)).expect("executable");
    }
    let cli = bin.join("agy");
    fs::write(&cli, format!("#!/bin/sh\n{ANTIGRAVITY_CLI}")).expect("fake command line");
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).expect("executable");

    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
    let profile = Profile {
        id: "work".into(),
        label: "Work".into(),
        provider: Provider::Antigravity,
        default_model: "gemini-3.6-flash-medium".into(),
        enabled: true,
        env: BTreeMap::from([
            ("PATH".into(), bin.display().to_string()),
            (
                "GEMINI_HOME".into(),
                directory.path().join("gemini").display().to_string(),
            ),
        ]),
        capabilities: vec![],
        command: None,
    };
    store
        .repositories()
        .profiles()
        .insert(&profile, NOW)
        .expect("profile");
    store
        .repositories()
        .settings()
        .put(
            &oga_config::canonical_cwd(oga_config::global_cwd())
                .display()
                .to_string(),
            oga_config::MODEL_SETTINGS_KEY,
            &serde_json::json!({"profiles": {"work": {"modelEnabled": {"gemini-3.6-flash-medium": true}}}})
                .to_string(),
            NOW,
        )
        .expect("model settings");
    let dispatcher = Dispatcher::new(store.clone(), oga_runner::ProviderRunner::default());
    Harness {
        _directory: directory,
        cwd,
        log,
        store,
        dispatcher,
    }
}

#[tokio::test]
async fn antigravity_runs_over_acp_by_default_with_its_model_and_gemini_home() {
    let harness = antigravity_harness("antigravity", Some(GOOGLE_SIGN_IN));
    let mut request = DispatchRequest::new("work", "summarise the readme", &harness.cwd);
    request.effort = Some("high".into());

    let task = harness.run_with(request).await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert!(task.output.contains("Done."), "{task:?}");
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Acp);
    let agent = recorded.agent.as_ref().expect("agent identity");
    assert_eq!(agent.adapter, "antigravity-acp");
    assert_eq!(agent.version.as_deref(), Some("agy_acp_server_1.1.1"));
    assert_eq!(
        recorded.acp_session_id.as_deref(),
        Some("5b8e2c4a-1f3d-4e6b-9a7c-2d4f6e8a0b1c")
    );
    assert_eq!(recorded.restore, Some(AcpRestore::Resume));
    assert_eq!(
        task.session_id, None,
        "the server's conversation is not one `agy --conversation` can reopen"
    );
    assert_eq!(
        methods(&harness),
        [
            "initialize",
            "session/new",
            "session/set_config_option",
            "session/set_config_option",
            "session/prompt",
        ],
        "the model and the permission mode are chosen before the prompt goes out"
    );
    assert_eq!(
        chosen_settings(&harness),
        [
            ("model".to_owned(), "gemini-3.6-flash-medium".to_owned()),
            ("mode".to_owned(), "yolo".to_owned()),
        ],
        "the model --model names, approving every tool as --dangerously-skip-permissions does"
    );
    let opened = &harness.received("session/new")[0];
    assert_eq!(opened["cwd"], task.cwd);
    assert_eq!(
        opened["mcpServers"],
        serde_json::json!([]),
        "as on its command line, Antigravity gets no Oga tools"
    );
    let started = &harness.agent_log()[0]["env"];
    assert!(
        started["GEMINI_HOME"]
            .as_str()
            .is_some_and(|dir| dir.ends_with("/gemini")),
        "the server runs in the profile's own Gemini home: {started}"
    );
    if cfg!(target_os = "macos") {
        assert_eq!(
            started["AGY_ACP_FORCE_FILE_STORAGE"], "1",
            "a home of its own reads its own sign-in, not the shared keychain's"
        );
    }
    assert!(harness.cli_runs().is_empty());
}

#[tokio::test]
async fn antigravity_without_a_ready_acp_server_runs_on_its_command_line_before_any_prompt() {
    for (mode, sign_in) in [("missing", Some(GOOGLE_SIGN_IN)), ("antigravity", None)] {
        let harness = antigravity_harness(mode, sign_in);

        let task = harness.run("do the work").await;

        assert_eq!(task.state, TaskState::Completed, "{mode}: {task:?}");
        assert_eq!(task.output.trim(), "ran on the command line", "{mode}");
        let recorded = transport(&task);
        assert_eq!(recorded.kind, Transport::Cli, "{mode}");
        assert_eq!(
            recorded.reason,
            Some(TransportReason::Unavailable),
            "{mode}"
        );
        assert_eq!(
            task.session_id.as_deref(),
            Some("c0ffee00-0000-4000-8000-00000000c11a"),
            "{mode}"
        );
        assert!(
            harness.received("session/prompt").is_empty(),
            "{mode}: no prompt reached the server"
        );
        let ran = harness.cli_runs();
        assert!(
            ran.windows(2)
                .any(|pair| pair == ["--model", "gemini-3.6-flash-medium"]),
            "{mode}: {ran:?}"
        );
        assert!(
            ran.iter()
                .any(|arg| arg == "--dangerously-skip-permissions"),
            "{mode}: {ran:?}"
        );
        assert!(
            harness
                .events(&task.id)
                .iter()
                .any(|event| event.kind == "transport_fallback"),
            "{mode}"
        );
    }

    let unsigned = antigravity_harness("antigravity", None);
    let task = unsigned.run("do the work").await;

    assert!(
        transport(&task)
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("isn't signed in")),
        "{task:?}"
    );
    assert!(
        unsigned.agent_log().is_empty(),
        "a server nobody signed in to is never started, so it cannot open a sign-in page"
    );
}

#[tokio::test]
async fn an_antigravity_model_its_server_does_not_offer_never_reaches_the_prompt_or_the_command_line()
 {
    let harness = antigravity_harness("antigravity-no-model", Some(GOOGLE_SIGN_IN));

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert!(
        task.error.as_deref().is_some_and(|error| error
            .contains("doesn't offer gemini-3.6-flash-medium as a model")),
        "{task:?}"
    );
    assert!(harness.received("session/prompt").is_empty());
    assert!(
        harness.cli_runs().is_empty(),
        "a server Oga reached is never stepped around through the command line"
    );
}

#[tokio::test]
async fn explicit_acp_on_an_antigravity_server_nobody_signed_in_to_fails_visibly() {
    let harness = antigravity_harness("antigravity", None);
    set_transport_preference(&harness.store, "work", TransportPreference::Acp, NOW)
        .expect("preference");

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert!(
        task.error
            .as_deref()
            .is_some_and(|error| error.contains("isn't signed in")),
        "{task:?}"
    );
    assert!(harness.agent_log().is_empty());
    assert!(harness.cli_runs().is_empty());
}

#[tokio::test]
async fn an_antigravity_server_that_refuses_the_sign_in_never_falls_back() {
    let harness = antigravity_harness("antigravity-auth", Some(GOOGLE_SIGN_IN));

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert_eq!(
        task.completion.as_ref().map(|completion| completion.code),
        Some(CompletionCode::Auth),
        "{task:?}"
    );
    assert!(harness.received("session/prompt").is_empty());
    assert!(
        harness.cli_runs().is_empty(),
        "a refused sign-in is never stepped around through the command line"
    );
}

#[tokio::test]
async fn an_antigravity_build_oga_was_not_verified_against_never_opens_a_session() {
    for mode in ["antigravity-next", "antigravity-renamed"] {
        let harness = antigravity_harness(mode, Some(GOOGLE_SIGN_IN));

        let task = harness.run("do the work").await;

        assert_eq!(task.state, TaskState::Completed, "{mode}: {task:?}");
        let recorded = transport(&task);
        assert_eq!(recorded.kind, Transport::Cli, "{mode}");
        assert!(
            recorded
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("antigravity-acp agy_acp_server_1.1.1")),
            "{mode}: {recorded:?}"
        );
        assert_eq!(methods(&harness), ["initialize"], "{mode}");
        assert!(!harness.cli_runs().is_empty(), "{mode}");
    }

    let explicit = antigravity_harness("antigravity-next", Some(GOOGLE_SIGN_IN));
    set_transport_preference(&explicit.store, "work", TransportPreference::Acp, NOW)
        .expect("preference");
    let task = explicit.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert!(
        task.error
            .as_deref()
            .is_some_and(|error| error.contains("not antigravity-acp agy_acp_server_1.1.2")),
        "{task:?}"
    );
    assert_eq!(methods(&explicit), ["initialize"]);
    assert!(explicit.cli_runs().is_empty());
}

#[tokio::test]
async fn an_antigravity_follow_up_resumes_its_session_and_a_lost_prompt_is_never_rerun() {
    let harness = antigravity_harness("antigravity", Some(GOOGLE_SIGN_IN));
    let first = harness.run("start").await;

    resume(
        &harness.dispatcher,
        ResumeRequest::new(&first.id).instruction("now the tests"),
    )
    .await
    .expect("resumed");
    let second = harness.settle(&first.id).await;

    assert_eq!(second.state, TaskState::Completed, "{second:?}");
    let resumed = harness.received("session/resume");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        resumed[0]["sessionId"],
        "5b8e2c4a-1f3d-4e6b-9a7c-2d4f6e8a0b1c"
    );
    assert_eq!(harness.received("session/new").len(), 1);
    assert_eq!(harness.received("session/prompt").len(), 2);
    assert_eq!(
        chosen_settings(&harness)
            .iter()
            .filter(|(id, value)| id == "mode" && value == "yolo")
            .count(),
        2,
        "a resumed session approves every tool again before its prompt"
    );
    assert_eq!(second.session_id, None);
    assert!(harness.cli_runs().is_empty());

    let lost = antigravity_harness("antigravity-exit-after-prompt", Some(GOOGLE_SIGN_IN));
    let task = lost.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert_eq!(lost.received("session/prompt").len(), 1);
    assert!(
        lost.cli_runs().is_empty(),
        "a prompt that may have run is never sent through the command line"
    );
}

#[tokio::test]
async fn an_antigravity_task_from_before_acp_resumes_its_conversation_on_the_command_line() {
    let harness = antigravity_harness("antigravity", Some(GOOGLE_SIGN_IN));
    let task = Task {
        id: "before-acp".into(),
        profile_id: "work".into(),
        model: "gemini-3.6-flash-medium".into(),
        prompt: "old work".into(),
        cwd: harness.cwd.display().to_string(),
        state: TaskState::Completed,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        scope: TaskScope {
            read: vec!["**".into()],
            write: vec!["**".into()],
        },
        session_id: Some("c0ffee00-0000-4000-8000-0000000001d0".into()),
        ..Task::default()
    };
    harness
        .store
        .repositories()
        .tasks()
        .insert(&task)
        .expect("task");

    resume(
        &harness.dispatcher,
        ResumeRequest::new("before-acp").instruction("one more thing"),
    )
    .await
    .expect("resumed");
    let resumed = harness.settle("before-acp").await;

    assert_eq!(resumed.state, TaskState::Completed, "{resumed:?}");
    assert!(harness.agent_log().is_empty(), "no ACP server was started");
    assert!(
        harness
            .cli_runs()
            .windows(2)
            .any(|pair| pair == ["--conversation", "c0ffee00-0000-4000-8000-0000000001d0"])
    );
    assert_eq!(transport(&resumed).reason, Some(TransportReason::Legacy));
}

/// Answers `pi --mode json` the way Pi's command line does, after noting that
/// it ran.
const PI_CLI: &str = r#"printf '%s\n' "$@" >> "$PWD/cli-ran"
printf '%s\n' '{"type":"session","id":"c0ffee00-0000-4000-8000-00000000c1a1"}' '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"ran on the command line\nOGA_RESULT: completed"}]}}'
"#;

/// The session id Pi's ACP adapter opens, which is Pi's own.
const PI_SESSION: &str = "0b7d4a8e-5c1f-4d2a-9e3b-6f8c1a2d4e5f";

/// A Pi profile on the adapters Oga ships, with its own home and agent
/// directory, `pi-acp` resolving to the scripted adapter in `mode`, and `pi` to
/// a command line that records that it ran. `missing` is an account without
/// the adapter installed. Each of `resources` is created in the project.
fn pi_harness(mode: &str, resources: &[&str]) -> Harness {
    let directory = tempfile::tempdir().expect("temporary directory");
    let cwd = directory.path().join("project");
    let bin = directory.path().join("bin");
    fs::create_dir_all(cwd.join("src")).expect("project");
    for resource in resources {
        fs::create_dir_all(cwd.join(resource)).expect("project resource");
    }
    fs::create_dir_all(&bin).expect("bin");
    let log = directory.path().join("agent.log");
    if mode != "missing" {
        let adapter = bin.join("pi-acp");
        fs::write(
            &adapter,
            format!(
                "#!/bin/sh\nexec '{}' '{mode}' '{}'\n",
                env!("CARGO_BIN_EXE_fake-acp-agent"),
                log.display()
            ),
        )
        .expect("fake adapter");
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).expect("executable");
    }
    let cli = bin.join("pi");
    fs::write(&cli, format!("#!/bin/sh\n{PI_CLI}")).expect("fake command line");
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).expect("executable");

    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
    let profile = Profile {
        id: "work".into(),
        label: "Work".into(),
        provider: Provider::Pi,
        default_model: "openai/gpt-5.5".into(),
        enabled: true,
        env: BTreeMap::from([
            ("PATH".into(), bin.display().to_string()),
            (
                "HOME".into(),
                directory.path().join("home").display().to_string(),
            ),
            (
                "PI_CODING_AGENT_DIR".into(),
                directory.path().join("agent").display().to_string(),
            ),
        ]),
        capabilities: vec![],
        command: None,
    };
    store
        .repositories()
        .profiles()
        .insert(&profile, NOW)
        .expect("profile");
    store
        .repositories()
        .settings()
        .put(
            &oga_config::canonical_cwd(oga_config::global_cwd())
                .display()
                .to_string(),
            oga_config::MODEL_SETTINGS_KEY,
            &serde_json::json!({"profiles": {"work": {"modelEnabled": {
                "openai/gpt-5.5": true,
                "anthropic/claude-sonnet-4-5": true,
            }}}})
            .to_string(),
            NOW,
        )
        .expect("model settings");
    let dispatcher = Dispatcher::new(store.clone(), oga_runner::ProviderRunner::default());
    Harness {
        _directory: directory,
        cwd,
        log,
        store,
        dispatcher,
    }
}

#[tokio::test]
async fn pi_runs_over_acp_by_default_with_its_model_thinking_level_and_account() {
    let harness = pi_harness("pi", &[]);
    let mut request = DispatchRequest::new("work", "summarise the readme", &harness.cwd);
    request.effort = Some("high".into());

    let task = harness.run_with(request).await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert!(task.output.contains("Done."), "{task:?}");
    let recorded = transport(&task);
    assert_eq!(recorded.kind, Transport::Acp);
    let agent = recorded.agent.as_ref().expect("agent identity");
    assert_eq!(agent.adapter, "pi-acp");
    assert_eq!(agent.version.as_deref(), Some("0.0.33"));
    assert_eq!(recorded.acp_session_id.as_deref(), Some(PI_SESSION));
    assert_eq!(
        recorded.restore,
        Some(AcpRestore::Load),
        "the adapter loads a session but cannot resume one"
    );
    assert_eq!(
        task.session_id.as_deref(),
        Some(PI_SESSION),
        "Pi's own session, which `pi --session-id` continues in a terminal"
    );
    assert_eq!(
        methods(&harness),
        [
            "initialize",
            "session/new",
            "session/set_config_option",
            "session/set_config_option",
            "session/prompt",
        ],
        "the model and thinking level are chosen before the prompt goes out"
    );
    assert_eq!(
        chosen_settings(&harness),
        [
            ("model".to_owned(), "openai/gpt-5.5".to_owned()),
            ("thought_level".to_owned(), "high".to_owned()),
        ],
        "the model --model names and the level --thinking names"
    );
    let opened = &harness.received("session/new")[0];
    assert_eq!(opened["cwd"], task.cwd);
    assert_eq!(
        opened["mcpServers"],
        serde_json::json!([]),
        "as on its command line, Pi gets no Oga tools"
    );
    let started = &harness.agent_log()[0]["env"];
    assert!(
        started["PI_CODING_AGENT_DIR"]
            .as_str()
            .is_some_and(|dir| dir.ends_with("/agent")),
        "{started}"
    );
    assert!(
        started["PI_CODING_AGENT_SESSION_DIR"]
            .as_str()
            .is_some_and(|dir| dir.ends_with("/agent/sessions")),
        "the adapter's pi keeps sessions where the command line does: {started}"
    );
    assert!(harness.cli_runs().is_empty());
}

#[tokio::test]
async fn a_pi_extension_question_is_declined_and_the_startup_summary_left_out() {
    let harness = pi_harness("pi-ask", &[]);
    let mut request = DispatchRequest::new("work", "deploy it", &harness.cwd);
    request.model = Some("anthropic/claude-sonnet-4-5".into());

    let task = harness.run_with(request).await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert_eq!(
        task.output.trim(),
        "confirmed: no",
        "nobody is there to say yes, as on the command line, and the summary Pi writes after opening the session is not the answer"
    );
    assert_eq!(
        methods(&harness),
        ["initialize", "session/new", "session/prompt"],
        "the session already holds the model, so nothing stands between opening it and the prompt"
    );
    let answered: Vec<Value> = harness
        .events(&task.id)
        .into_iter()
        .filter(|event| event.kind == "permission_answered")
        .map(|event| Value::Object(event.payload.into_iter().collect()))
        .collect();
    assert_eq!(answered.len(), 1, "{answered:?}");
    assert_eq!(answered[0]["allowed"], false);
    assert_eq!(answered[0]["unattended"], true);
    assert!(
        !harness
            .events(&task.id)
            .iter()
            .any(|event| format!("{:?}", event.payload).contains("pi v0.85.1")),
        "the startup summary is not recorded as part of the turn"
    );
}

#[tokio::test]
async fn pi_keeps_to_its_command_line_before_any_prompt_when_acp_cannot_run_it_the_same_way() {
    for (mode, effort, resources) in [
        ("missing", None, &[][..]),
        ("pi", None, &[".pi/extensions"][..]),
        ("pi", None, &[".agents/skills"][..]),
    ] {
        let harness = pi_harness(mode, resources);
        let mut request = DispatchRequest::new("work", "do the work", &harness.cwd);
        request.effort = effort.map(str::to_owned);

        let task = harness.run_with(request).await;

        let case = format!("{mode} {effort:?} {resources:?}");
        assert_eq!(task.state, TaskState::Completed, "{case}: {task:?}");
        assert_eq!(task.output.trim(), "ran on the command line", "{case}");
        let recorded = transport(&task);
        assert_eq!(recorded.kind, Transport::Cli, "{case}");
        assert_eq!(
            recorded.reason,
            Some(TransportReason::Unavailable),
            "{case}"
        );
        assert_eq!(
            task.session_id.as_deref(),
            Some("c0ffee00-0000-4000-8000-00000000c1a1"),
            "{case}"
        );
        assert!(
            harness.received("session/prompt").is_empty(),
            "{case}: no prompt reached the adapter"
        );
        let ran = harness.cli_runs();
        assert!(
            ran.windows(2)
                .any(|pair| pair == ["--model", "openai/gpt-5.5"]),
            "{case}: {ran:?}"
        );
        assert!(
            ran.iter().any(|arg| arg == "--no-approve"),
            "{case}: {ran:?}"
        );
        assert!(
            harness
                .events(&task.id)
                .iter()
                .any(|event| event.kind == "transport_fallback"),
            "{case}"
        );
        if !resources.is_empty() {
            assert!(
                recorded
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("Pi files of its own")),
                "{case}: {recorded:?}"
            );
            assert!(
                harness.agent_log().is_empty(),
                "{case}: the adapter is never started for a project it would load"
            );
        }
    }

    let explicit = pi_harness("pi", &[".pi/settings.json"]);
    set_transport_preference(&explicit.store, "work", TransportPreference::Acp, NOW)
        .expect("preference");
    let task = explicit.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert!(
        task.error
            .as_deref()
            .is_some_and(|error| error.contains("Pi files of its own")),
        "{task:?}"
    );
    assert!(explicit.agent_log().is_empty());
    assert!(explicit.cli_runs().is_empty());
}

#[tokio::test]
async fn a_pi_thinking_level_it_does_not_offer_never_reaches_the_prompt_or_the_command_line() {
    let harness = pi_harness("pi", &[]);
    let mut request = DispatchRequest::new("work", "do the work", &harness.cwd);
    request.effort = Some("max".into());

    let task = harness.run_with(request).await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert!(
        task.error
            .as_deref()
            .is_some_and(|error| error.contains("doesn't offer max as a thought_level")),
        "{task:?}"
    );
    assert!(harness.received("session/prompt").is_empty());
    assert!(
        harness.cli_runs().is_empty(),
        "an adapter Oga reached is never stepped around through the command line"
    );
}

#[tokio::test]
async fn a_pi_adapter_oga_was_not_verified_against_never_opens_a_session() {
    for mode in ["pi-next", "pi-renamed"] {
        let harness = pi_harness(mode, &[]);

        let task = harness.run("do the work").await;

        assert_eq!(task.state, TaskState::Completed, "{mode}: {task:?}");
        let recorded = transport(&task);
        assert_eq!(recorded.kind, Transport::Cli, "{mode}");
        assert!(
            recorded
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("pi-acp 0.0.33")),
            "{mode}: {recorded:?}"
        );
        assert_eq!(methods(&harness), ["initialize"], "{mode}");
        assert!(!harness.cli_runs().is_empty(), "{mode}");
    }

    let explicit = pi_harness("pi-next", &[]);
    set_transport_preference(&explicit.store, "work", TransportPreference::Acp, NOW)
        .expect("preference");
    let task = explicit.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert!(
        task.error
            .as_deref()
            .is_some_and(|error| error.contains("not pi-acp 0.0.34")),
        "{task:?}"
    );
    assert_eq!(methods(&explicit), ["initialize"]);
    assert!(explicit.cli_runs().is_empty());
}

#[tokio::test]
async fn a_pi_follow_up_loads_its_session_and_a_lost_prompt_is_never_rerun() {
    let harness = pi_harness("pi", &[]);
    let first = harness.run("start").await;

    resume(
        &harness.dispatcher,
        ResumeRequest::new(&first.id).instruction("now the tests"),
    )
    .await
    .expect("resumed");
    let second = harness.settle(&first.id).await;

    assert_eq!(second.state, TaskState::Completed, "{second:?}");
    assert!(
        !second.output.contains("an earlier answer"),
        "the replayed conversation is not this turn's answer: {second:?}"
    );
    let loaded = harness.received("session/load");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0]["sessionId"], PI_SESSION);
    assert_eq!(harness.received("session/new").len(), 1);
    assert_eq!(harness.received("session/prompt").len(), 2);
    assert_eq!(
        chosen_settings(&harness)
            .iter()
            .filter(|(id, value)| id == "model" && value == "openai/gpt-5.5")
            .count(),
        2,
        "a loaded session is put back on the task's model before its prompt"
    );
    assert_eq!(second.session_id.as_deref(), Some(PI_SESSION));
    assert!(harness.cli_runs().is_empty());

    let lost = pi_harness("pi-exit-after-prompt", &[]);
    let task = lost.run("do the work").await;

    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    assert_eq!(lost.received("session/prompt").len(), 1);
    assert!(
        lost.cli_runs().is_empty(),
        "a prompt that may have run is never sent through the command line"
    );
}

#[tokio::test]
async fn a_pi_session_the_adapter_did_not_tie_to_a_session_file_is_never_given_to_a_terminal() {
    let harness = pi_harness("pi-unmapped", &[]);

    let task = harness.run("do the work").await;

    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert_eq!(transport(&task).acp_session_id.as_deref(), Some(PI_SESSION));
    assert_eq!(task.session_id, None);
}

#[tokio::test]
async fn a_pi_task_from_before_acp_resumes_on_its_command_line() {
    let harness = pi_harness("pi", &[]);
    let task = Task {
        id: "before-acp".into(),
        profile_id: "work".into(),
        model: "openai/gpt-5.5".into(),
        prompt: "old work".into(),
        cwd: harness.cwd.display().to_string(),
        state: TaskState::Completed,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        scope: TaskScope {
            read: vec!["**".into()],
            write: vec!["**".into()],
        },
        session_id: Some("c0ffee00-0000-4000-8000-0000000001d0".into()),
        ..Task::default()
    };
    harness
        .store
        .repositories()
        .tasks()
        .insert(&task)
        .expect("task");

    resume(
        &harness.dispatcher,
        ResumeRequest::new("before-acp").instruction("one more thing"),
    )
    .await
    .expect("resumed");
    let resumed = harness.settle("before-acp").await;

    assert_eq!(resumed.state, TaskState::Completed, "{resumed:?}");
    assert!(harness.agent_log().is_empty(), "no ACP adapter was started");
    assert!(
        harness
            .cli_runs()
            .windows(2)
            .any(|pair| pair == ["--session-id", "c0ffee00-0000-4000-8000-0000000001d0"])
    );
    assert_eq!(transport(&resumed).reason, Some(TransportReason::Legacy));
}
