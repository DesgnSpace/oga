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
    AcpRestore, CompletionCode, Profile, Provider, Task, TaskScope, TaskState, TaskTransport,
    Transport, TransportPreference, TransportReason,
};
use oga_providers::{AcpAdapter, AcpAdapters};
use oga_service::{
    CancelRequest, DispatchRequest, Dispatcher, ResumeRequest, cancel, reconcile::ReconcileTrigger,
    resume, set_transport_preference,
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

    let messages: Vec<String> = harness
        .events(&task.id)
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
    for _ in 0..2_000 {
        if !harness.received("session/prompt").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

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
