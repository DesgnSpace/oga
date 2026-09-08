use std::{
    collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::Path, process::Command,
    sync::Arc, time::Duration,
};

use oga_domain::{
    BranchOutcome, CompletionCode, HoldArgs, HoldVerb, Profile, Provider, Task, TaskCompletion,
    TaskHold, TaskKind, TaskScope, TaskState, WorktreeRequest,
};
use oga_service::{
    ArchiveRequest, CancelRequest, CompletionAssertion, Dispatcher, HandoffRequest, ReplyRequest,
    ResumeRequest, SteerRequest, archive, assert_completion, cancel, force_complete, handoff,
    reply, resume, steer,
};
use oga_store::Store;
use oga_worktree::{branch_exists, create_task_worktree_at};
use tempfile::TempDir;

fn profile(id: &str, default_model: &str) -> Profile {
    Profile {
        id: id.into(),
        label: id.into(),
        provider: Provider::Claude,
        default_model: default_model.into(),
        enabled: true,
        env: BTreeMap::new(),
        capabilities: vec![],
        command: Some(vec![
            "sh".into(),
            "-c".into(),
            "printf 'finished\\nOGA_RESULT: completed\\n'".into(),
        ]),
    }
}

fn task(id: &str, cwd: &str, state: TaskState) -> Task {
    Task {
        id: id.into(),
        kind: Some(TaskKind::Delegated),
        profile_id: "one".into(),
        model: "model-one".into(),
        prompt: "original task".into(),
        shipped_prompt: None,
        cwd: cwd.into(),
        branch: None,
        worktree: None,
        worktree_label: None,
        state,
        created_at: "2026-01-01T00:00:00.000Z".into(),
        updated_at: "2026-01-01T00:00:00.000Z".into(),
        duration_ms: 0,
        running_since: None,
        output: String::new(),
        error: None,
        question: (state == TaskState::NeedsInput).then(|| "which path?".into()),
        parent_task_id: None,
        orchestrator_id: None,
        scope: TaskScope {
            read: vec!["**".into()],
            write: vec!["**".into()],
        },
        grant_id: None,
        allow_questions: true,
        timeout_ms: None,
        effort: None,
        effort_actual: None,
        tldr: None,
        title: None,
        session_id: None,
        completion: (state == TaskState::Failed || state == TaskState::Blocked).then(|| {
            TaskCompletion {
                exit_code: None,
                blocked: true,
                code: CompletionCode::WorkerError,
                reason: Some("provider stopped".into()),
                suggested_scope: None,
                resets_at: None,
                asserted_completion: None,
                dependency_blocked: None,
            }
        }),
        attempts: vec![],
        cost_usd: None,
        cost_usd_estimated: false,
        turns: None,
        archived_at: None,
        queued_follow_ups: None,
        queued_follow_up_items: None,
        hold: None,
        attachments: Vec::new(),
    }
}

fn service() -> (TempDir, Arc<Store>, Dispatcher) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
    store
        .repositories()
        .profiles()
        .insert(&profile("one", "model-one"), "2026-01-01T00:00:00.000Z")
        .expect("profile one");
    store
        .repositories()
        .profiles()
        .insert(&profile("two", "model-two"), "2026-01-01T00:00:00.000Z")
        .expect("profile two");
    switch_models_on(&store);
    let dispatcher = Dispatcher::new(store.clone(), oga_runner::ProviderRunner::default());
    (directory, store, dispatcher)
}

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .output()
        .expect("git is installed");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Every model these fixtures run on, switched on. A model is unavailable until
/// the user turns it on; what happens afterwards is what these tests are about.
fn switch_models_on(store: &Store) {
    let on = serde_json::json!({
        "profiles": {
            "one": { "modelEnabled": { "model-one": true } },
            "two": { "modelEnabled": { "model-two": true } },
            "session": { "modelEnabled": { "session-model": true, "model-one": true } },
            "failing": { "modelEnabled": { "model-one": true } },
        }
    });
    store
        .repositories()
        .settings()
        .put(
            &oga_config::canonical_cwd(oga_config::global_cwd())
                .display()
                .to_string(),
            oga_config::MODEL_SETTINGS_KEY,
            &on.to_string(),
            "2026-01-01T00:00:00.000Z",
        )
        .expect("model settings");
}

fn seed(store: &Store, id: &str, cwd: &str, state: TaskState) {
    store
        .repositories()
        .tasks()
        .insert(&task(id, cwd, state))
        .expect("task");
}

fn seed_with_profile(
    store: &Store,
    id: &str,
    cwd: &str,
    state: TaskState,
    profile_id: &str,
    session_id: Option<&str>,
) {
    let mut task = task(id, cwd, state);
    task.profile_id = profile_id.into();
    task.session_id = session_id.map(str::to_owned);
    store.repositories().tasks().insert(&task).expect("task");
}

async fn wait_for_settlement(dispatcher: &Dispatcher, id: &str) -> TaskState {
    for _ in 0..1_000 {
        let state = dispatcher.task(id).expect("task").state;
        if state.settled() {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("task did not settle: {id}");
}

#[tokio::test]
async fn pi_events_are_persisted_before_the_process_exits() {
    let (directory, store, dispatcher) = service();
    let partial = directory.path().join("pi-partial");
    let complete = directory.path().join("pi-complete");
    let release = directory.path().join("pi-release");
    let current = store
        .repositories()
        .profiles()
        .get("one")
        .expect("profile lookup")
        .expect("profile one");
    let mut pi = current.clone();
    pi.provider = Provider::Pi;
    pi.env.insert(
        "PI_TEST_PARTIAL".into(),
        partial.to_string_lossy().into_owned(),
    );
    pi.env.insert(
        "PI_TEST_COMPLETE".into(),
        complete.to_string_lossy().into_owned(),
    );
    pi.env.insert(
        "PI_TEST_RELEASE".into(),
        release.to_string_lossy().into_owned(),
    );
    pi.command = Some(vec![
        "sh".into(),
        "-c".into(),
        r##"printf '{"type":"session","id":"pi-live-session'
: > "$PI_TEST_PARTIAL"
while [ ! -e "$PI_TEST_COMPLETE" ]; do sleep 0.01; done
printf '"}
'
while [ ! -e "$PI_TEST_RELEASE" ]; do sleep 0.01; done
printf '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}
OGA_RESULT: completed
'
"##
            .into(),
    ]);
    assert!(
        store
            .repositories()
            .profiles()
            .update_if_unchanged("one", &current, &pi, "2026-01-01T00:00:01.000Z")
            .expect("update Pi profile")
    );

    let task = dispatcher
        .dispatch(oga_service::DispatchRequest::new(
            "one",
            "stream Pi events",
            directory.path(),
        ))
        .await
        .expect("dispatch")
        .task;

    let mut partial_seen = false;
    for _ in 0..200 {
        if partial.exists() {
            partial_seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(partial_seen, "Pi did not write its partial JSON frame");
    let events = store
        .repositories()
        .events()
        .list(&task.id)
        .expect("events before completed frame");
    assert!(
        !events.iter().any(|event| event.kind == "agent.session"),
        "an incomplete Pi frame must not emit an event"
    );

    fs::write(&complete, []).expect("complete partial frame");
    let mut observed_live_event = false;
    for _ in 0..400 {
        let events = store
            .repositories()
            .events()
            .list(&task.id)
            .expect("live events");
        if events.iter().any(|event| event.kind == "agent.session") {
            assert_eq!(
                dispatcher.task(&task.id).expect("running task").state,
                TaskState::Running
            );
            observed_live_event = true;
            break;
        }
        assert_eq!(
            dispatcher.task(&task.id).expect("running task").state,
            TaskState::Running
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    fs::write(&release, []).expect("release Pi process");
    assert!(
        observed_live_event,
        "Pi events must be persisted before process exit"
    );
    assert_eq!(
        wait_for_settlement(&dispatcher, &task.id).await,
        TaskState::Completed
    );

    let events = store
        .repositories()
        .events()
        .list(&task.id)
        .expect("settled events");
    let agent_kinds = events
        .iter()
        .filter(|event| event.kind.starts_with("agent."))
        .map(|event| event.kind.as_str())
        .collect::<Vec<_>>();
    assert_eq!(agent_kinds, ["agent.session", "agent.message_end"]);
}

#[tokio::test]
async fn cancel_and_timeout_cover_every_legal_source_state() {
    let (directory, store, dispatcher) = service();
    let states = [
        TaskState::Queued,
        TaskState::Pending,
        TaskState::Running,
        TaskState::NeedsInput,
        TaskState::Answered,
        TaskState::Blocked,
    ];
    for (index, state) in states.into_iter().enumerate() {
        seed(
            &store,
            &format!("cancel-{index}"),
            directory.path().to_str().unwrap(),
            state,
        );
        let result = cancel(&dispatcher, CancelRequest::new(format!("cancel-{index}")))
            .await
            .expect("cancel");
        assert_eq!(result.state, TaskState::Cancelled);
    }
    seed(
        &store,
        "timeout",
        directory.path().to_str().unwrap(),
        TaskState::Running,
    );
    let result = cancel(&dispatcher, CancelRequest::new("timeout").timed_out())
        .await
        .expect("timeout");
    assert_eq!(result.state, TaskState::Failed);
    assert_eq!(
        result.completion.expect("completion").code,
        CompletionCode::Timeout
    );
}

#[tokio::test]
async fn resume_and_handoff_reopen_states_without_settling_tasks() {
    let (directory, store, dispatcher) = service();
    let cwd = directory.path().to_str().unwrap();
    for (index, state) in [
        TaskState::Failed,
        TaskState::Cancelled,
        TaskState::Blocked,
        TaskState::Pending,
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("resume-{index}");
        seed(&store, &id, cwd, state);
        let result = resume(&dispatcher, ResumeRequest::new(&id))
            .await
            .expect("resume");
        assert_eq!(result.state, TaskState::Queued);
        assert_eq!(
            wait_for_settlement(&dispatcher, &id).await,
            TaskState::Completed
        );
    }
    seed(&store, "completed", cwd, TaskState::Completed);
    assert!(
        resume(&dispatcher, ResumeRequest::new("completed"))
            .await
            .is_err()
    );
    seed(&store, "queued", cwd, TaskState::Queued);
    assert!(
        resume(&dispatcher, ResumeRequest::new("queued"))
            .await
            .is_err()
    );
    seed(&store, "running", cwd, TaskState::Running);
    assert!(
        resume(&dispatcher, ResumeRequest::new("running"))
            .await
            .is_err()
    );
    seed(&store, "question", cwd, TaskState::NeedsInput);
    assert!(
        resume(&dispatcher, ResumeRequest::new("question"))
            .await
            .is_err()
    );

    seed(&store, "handoff-failed", cwd, TaskState::Failed);
    let result = handoff(
        &dispatcher,
        HandoffRequest::new("handoff-failed").profile("two"),
    )
    .await
    .expect("handoff");
    assert_eq!(result.state, TaskState::Queued);
    assert_eq!(result.profile_id, "two");
    assert_eq!(
        wait_for_settlement(&dispatcher, "handoff-failed").await,
        TaskState::Completed
    );

    seed(&store, "handoff-queued", cwd, TaskState::Queued);
    let result = handoff(
        &dispatcher,
        HandoffRequest::new("handoff-queued").profile("two"),
    )
    .await
    .expect("queued handoff");
    assert_eq!(result.id, "handoff-queued");
    assert_eq!(result.state, TaskState::Queued);
    assert_eq!(result.profile_id, "two");
    assert_eq!(result.model, "model-two");
    seed(&store, "handoff-question", cwd, TaskState::NeedsInput);
    let result = handoff(
        &dispatcher,
        HandoffRequest::new("handoff-question").profile("two"),
    )
    .await
    .expect("needs-input handoff");
    assert_eq!(result.id, "handoff-question");
    assert_eq!(result.state, TaskState::Queued);
    assert_eq!(result.profile_id, "two");
}

#[tokio::test]
async fn handoff_without_a_session_rebuilds_context_and_keeps_the_task_id() {
    let (directory, store, dispatcher) = service();
    let cwd = directory.path().to_str().unwrap();
    let mut fixture = task("handoff-no-session", cwd, TaskState::Failed);
    fixture.profile_id = "one".into();
    fixture.prompt = "MARKER-NO-SESSION: continue the widget".into();
    fixture.session_id = None;
    store.repositories().tasks().insert(&fixture).expect("task");

    let result = handoff(
        &dispatcher,
        HandoffRequest::new("handoff-no-session").profile("two"),
    )
    .await
    .expect("handoff");
    assert_eq!(result.id, "handoff-no-session");
    assert_eq!(result.state, TaskState::Queued);
    assert_eq!(
        wait_for_settlement(&dispatcher, "handoff-no-session").await,
        TaskState::Completed
    );
    let task = dispatcher.task("handoff-no-session").expect("task");
    assert_eq!(task.id, "handoff-no-session");
    assert!(
        task.shipped_prompt
            .expect("shipped prompt")
            .contains("MARKER-NO-SESSION")
    );
    let events = store
        .repositories()
        .events()
        .list("handoff-no-session")
        .expect("events");
    assert!(events.iter().any(|event| event.kind == "handoff_brief"));
}

#[tokio::test]
async fn archive_stops_before_hiding_and_restore_keeps_terminal_state() {
    let (directory, store, dispatcher) = service();
    let cwd = directory.path().to_str().unwrap();
    for (index, state) in [
        TaskState::Queued,
        TaskState::Pending,
        TaskState::Running,
        TaskState::NeedsInput,
        TaskState::Answered,
        TaskState::Blocked,
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("archive-{index}");
        seed(&store, &id, cwd, state);
        let result = archive(&dispatcher, ArchiveRequest::new(&id, true))
            .await
            .expect("archive");
        assert!(result.stopped);
        assert!(result.task.archived_at.is_some());
    }
    let restored = archive(&dispatcher, ArchiveRequest::new("archive-0", false))
        .await
        .expect("restore");
    assert!(restored.task.archived_at.is_none());
    assert_eq!(restored.task.state, TaskState::Cancelled);
}

#[tokio::test]
async fn archives_a_clean_worktree_and_deletes_its_branch() {
    let (directory, store, dispatcher) = service();
    let repo = directory.path().join("project");
    fs::create_dir(&repo).expect("repository directory");
    git(&repo, &["init", "-b", "main"]);
    fs::write(repo.join("tracked.txt"), "one\n").expect("tracked file");
    git(&repo, &["add", "tracked.txt"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial",
        ],
    );
    let created = create_task_worktree_at(
        &directory.path().join("worktrees"),
        &repo,
        "archive-worktree",
        &WorktreeRequest::default(),
        Some("archive worktree"),
    )
    .await
    .expect("worktree created");
    let mut fixture = task(
        "archive-worktree",
        &created.cwd.to_string_lossy(),
        TaskState::Completed,
    );
    fixture.branch = Some(created.worktree.branch.clone());
    fixture.worktree = Some(created.worktree.clone());
    store.repositories().tasks().insert(&fixture).expect("task");
    store
        .transaction(|tx| {
            tx.execute(
                "UPDATE tasks SET origin_cwd=?,worktree_path=?,worktree_branch=? WHERE id=?",
                rusqlite::params![
                    &created.worktree.origin_cwd,
                    &created.worktree.path,
                    &created.worktree.branch,
                    &fixture.id,
                ],
            )?;
            Ok(())
        })
        .expect("worktree recorded");

    let result = archive(
        &dispatcher,
        ArchiveRequest::new("archive-worktree", true).delete_branch(),
    )
    .await
    .expect("archive");

    assert_eq!(result.checkout.as_deref(), Some("removed"));
    assert_eq!(result.branch, Some(BranchOutcome::Deleted));
    assert!(result.branch_reason.is_none());
    assert!(!Path::new(&created.worktree.path).exists());
    assert!(
        !branch_exists(&repo, &created.worktree.branch)
            .await
            .expect("branch inspected")
    );
}

#[tokio::test]
async fn prunes_a_missing_checkout_before_deleting_its_branch() {
    let (directory, store, dispatcher) = service();
    let repo = directory.path().join("project");
    fs::create_dir(&repo).expect("repository directory");
    git(&repo, &["init", "-b", "main"]);
    fs::write(repo.join("tracked.txt"), "one\n").expect("tracked file");
    git(&repo, &["add", "tracked.txt"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial",
        ],
    );
    let created = create_task_worktree_at(
        &directory.path().join("worktrees"),
        &repo,
        "archive-missing-checkout",
        &WorktreeRequest::default(),
        Some("archive missing checkout"),
    )
    .await
    .expect("worktree created");
    let mut fixture = task(
        "archive-missing-checkout",
        &created.cwd.to_string_lossy(),
        TaskState::Completed,
    );
    fixture.branch = Some(created.worktree.branch.clone());
    fixture.worktree = Some(created.worktree.clone());
    store.repositories().tasks().insert(&fixture).expect("task");
    store
        .transaction(|tx| {
            tx.execute(
                "UPDATE tasks SET origin_cwd=?,worktree_path=?,worktree_branch=? WHERE id=?",
                rusqlite::params![
                    &created.worktree.origin_cwd,
                    &created.worktree.path,
                    &created.worktree.branch,
                    &fixture.id,
                ],
            )?;
            Ok(())
        })
        .expect("worktree recorded");
    fs::remove_dir_all(&created.worktree.path).expect("missing checkout");

    let result = archive(
        &dispatcher,
        ArchiveRequest::new("archive-missing-checkout", true).delete_branch(),
    )
    .await
    .expect("archive");

    assert_eq!(result.checkout.as_deref(), Some("nothing to remove"));
    assert_eq!(result.branch, Some(BranchOutcome::Deleted));
    assert!(
        !branch_exists(&repo, &created.worktree.branch)
            .await
            .expect("branch inspected")
    );
}

#[tokio::test]
async fn keeps_uncommitted_worktree_and_branch_when_archiving() {
    let (directory, store, dispatcher) = service();
    let repo = directory.path().join("project");
    fs::create_dir(&repo).expect("repository directory");
    git(&repo, &["init", "-b", "main"]);
    fs::write(repo.join("tracked.txt"), "one\n").expect("tracked file");
    git(&repo, &["add", "tracked.txt"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial",
        ],
    );
    let created = create_task_worktree_at(
        &directory.path().join("worktrees"),
        &repo,
        "archive-uncommitted",
        &WorktreeRequest::default(),
        Some("archive uncommitted worktree"),
    )
    .await
    .expect("worktree created");
    fs::write(created.cwd.join("unfinished.txt"), "unfinished\n").expect("unfinished file");
    let mut fixture = task(
        "archive-uncommitted",
        &created.cwd.to_string_lossy(),
        TaskState::Completed,
    );
    fixture.branch = Some(created.worktree.branch.clone());
    fixture.worktree = Some(created.worktree.clone());
    store.repositories().tasks().insert(&fixture).expect("task");
    store
        .transaction(|tx| {
            tx.execute(
                "UPDATE tasks SET origin_cwd=?,worktree_path=?,worktree_branch=? WHERE id=?",
                rusqlite::params![
                    &created.worktree.origin_cwd,
                    &created.worktree.path,
                    &created.worktree.branch,
                    &fixture.id,
                ],
            )?;
            Ok(())
        })
        .expect("worktree recorded");

    let result = archive(
        &dispatcher,
        ArchiveRequest::new("archive-uncommitted", true).delete_branch(),
    )
    .await
    .expect("archive");

    assert!(
        result
            .checkout
            .as_deref()
            .is_some_and(|checkout| checkout.starts_with("kept because it has uncommitted work: "))
    );
    assert_eq!(result.branch, Some(BranchOutcome::Kept));
    assert_eq!(
        result.branch_reason.as_deref(),
        Some("checkout was kept, so the branch was kept")
    );
    assert!(Path::new(&created.worktree.path).exists());
    assert!(
        branch_exists(&repo, &created.worktree.branch)
            .await
            .expect("branch inspected")
    );
}

#[tokio::test]
async fn archive_keeps_a_checkout_used_by_a_settled_joined_task() {
    let (directory, store, dispatcher) = service();
    let repo = directory.path().join("project");
    fs::create_dir(&repo).expect("repository directory");
    git(&repo, &["init", "-b", "main"]);
    fs::write(repo.join("tracked.txt"), "one\n").expect("tracked file");
    git(&repo, &["add", "tracked.txt"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial",
        ],
    );
    let created = create_task_worktree_at(
        &directory.path().join("worktrees"),
        &repo,
        "archive-owner",
        &WorktreeRequest::default(),
        Some("archive owner"),
    )
    .await
    .expect("worktree created");
    for (id, state) in [
        ("archive-owner", TaskState::Completed),
        ("archive-joined", TaskState::Completed),
    ] {
        let mut fixture = task(id, &created.cwd.to_string_lossy(), state);
        fixture.branch = Some(created.worktree.branch.clone());
        fixture.worktree = Some(created.worktree.clone());
        store.repositories().tasks().insert(&fixture).expect("task");
        store
            .transaction(|tx| {
                tx.execute(
                    "UPDATE tasks SET origin_cwd=?,worktree_path=?,worktree_branch=? WHERE id=?",
                    rusqlite::params![
                        &created.worktree.origin_cwd,
                        &created.worktree.path,
                        &created.worktree.branch,
                        id,
                    ],
                )?;
                Ok(())
            })
            .expect("worktree recorded");
    }

    let result = archive(
        &dispatcher,
        ArchiveRequest::new("archive-owner", true).delete_branch(),
    )
    .await
    .expect("archive");

    assert_eq!(
        result.checkout.as_deref(),
        Some("kept because archive-joined is still using it")
    );
    assert_eq!(result.branch, Some(BranchOutcome::Kept));
    assert!(Path::new(&created.worktree.path).exists());
    assert!(
        branch_exists(&repo, &created.worktree.branch)
            .await
            .expect("branch inspected")
    );

    let result = archive(
        &dispatcher,
        ArchiveRequest::new("archive-joined", true).delete_branch(),
    )
    .await
    .expect("archive joined task");
    assert_eq!(result.checkout.as_deref(), Some("removed"));
    assert_eq!(result.branch, Some(BranchOutcome::Deleted));
    assert!(!Path::new(&created.worktree.path).exists());
}

#[tokio::test]
async fn refuses_branch_deletion_without_a_worktree() {
    let (directory, store, dispatcher) = service();
    let cwd = directory.path().to_str().unwrap();
    seed(&store, "archive-no-worktree", cwd, TaskState::Completed);

    let error = archive(
        &dispatcher,
        ArchiveRequest::new("archive-no-worktree", true).delete_branch(),
    )
    .await
    .expect_err("branch deletion requires a worktree");
    assert_eq!(
        error.to_string(),
        "task action refused: deleteBranch only applies to worktree tasks"
    );
    assert!(
        dispatcher
            .task("archive-no-worktree")
            .expect("task")
            .archived_at
            .is_none()
    );
}

#[tokio::test]
async fn reply_reopens_a_captured_session_only_from_needs_input() {
    let (directory, store, dispatcher) = service();
    let bin = directory.path().join("bin");
    fs::create_dir(&bin).expect("bin");
    let claude = bin.join("claude");
    fs::write(
        &claude,
        "#!/bin/sh\nprintf 'finished\\nOGA_RESULT: completed\\n'\n",
    )
    .expect("fake claude");
    let mut permissions = fs::metadata(&claude).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&claude, permissions).expect("permissions");
    let session_profile = Profile {
        id: "session".into(),
        label: "session".into(),
        provider: Provider::Claude,
        default_model: "session-model".into(),
        enabled: true,
        env: BTreeMap::from([("PATH".into(), format!("{}:/usr/bin:/bin", bin.display()))]),
        capabilities: vec![],
        command: None,
    };
    store
        .repositories()
        .profiles()
        .insert(&session_profile, "2026-01-01T00:00:00.000Z")
        .expect("session profile");
    let cwd = directory.path().to_str().unwrap();
    seed_with_profile(
        &store,
        "question",
        cwd,
        TaskState::NeedsInput,
        "session",
        Some("session-1"),
    );
    let answered = reply(&dispatcher, ReplyRequest::new("question", "use path two"))
        .await
        .expect("reply");
    assert_eq!(answered.state, TaskState::Queued);
    assert_eq!(
        wait_for_settlement(&dispatcher, "question").await,
        TaskState::Completed
    );
    let completed = dispatcher.task("question").expect("task");
    assert_eq!(completed.attempts.len(), 1);

    for (index, state) in [
        TaskState::Queued,
        TaskState::Pending,
        TaskState::Running,
        TaskState::Blocked,
        TaskState::Failed,
        TaskState::Cancelled,
        TaskState::Completed,
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("reply-refused-{index}");
        seed(&store, &id, cwd, state);
        assert!(
            reply(&dispatcher, ReplyRequest::new(&id, "answer"))
                .await
                .is_err()
        );
    }
}

#[test]
fn completion_assertion_only_accepts_failed_or_blocked_and_force_excludes_completed() {
    let (directory, store, dispatcher) = service();
    let cwd = directory.path().to_str().unwrap();
    seed(&store, "blocked", cwd, TaskState::Blocked);
    let completed = assert_completion(
        &dispatcher,
        CompletionAssertion::new("blocked", "reviewer", "verified in checkout"),
    )
    .expect("assertion");
    assert_eq!(completed.state, TaskState::Completed);
    assert_eq!(
        completed
            .completion
            .expect("completion")
            .asserted_completion
            .expect("override")
            .asserted_by,
        "reviewer"
    );

    seed(&store, "running", cwd, TaskState::Running);
    assert!(
        assert_completion(
            &dispatcher,
            CompletionAssertion::new("running", "reviewer", "not live"),
        )
        .is_err()
    );
    seed(&store, "cancelled", cwd, TaskState::Cancelled);
    assert!(
        assert_completion(
            &dispatcher,
            CompletionAssertion::new("cancelled", "reviewer", "not this path"),
        )
        .is_err()
    );
    let forced = force_complete(
        &dispatcher,
        CompletionAssertion::new("cancelled", "reviewer", "manual close"),
    )
    .expect("force complete");
    assert_eq!(forced.state, TaskState::Completed);
}

#[tokio::test]
async fn steer_without_a_live_channel_queues_the_instruction_and_refuses_a_model_change() {
    let (directory, store, dispatcher) = service();
    let cwd = directory.path().to_str().unwrap();
    seed(&store, "running", cwd, TaskState::Running);
    let outcome = steer(
        &dispatcher,
        SteerRequest::new("running").instruction("change direction"),
    )
    .await
    .expect("steer queues the instruction");
    assert!(outcome.queued);
    assert_eq!(outcome.task.queued_follow_ups, Some(1));
    let queued = oga_service::list_follow_ups(&store, "running").expect("queue");
    assert_eq!(
        queued
            .iter()
            .map(|follow_up| follow_up.instruction.as_str())
            .collect::<Vec<_>>(),
        vec!["change direction"]
    );
    let events = store
        .repositories()
        .events()
        .list("running")
        .expect("events");
    assert_eq!(events.last().expect("queued").kind, "follow_up_queued");

    let error = steer(
        &dispatcher,
        SteerRequest::new("running")
            .instruction("use the other model")
            .model("model"),
    )
    .await
    .expect_err("a model change cannot wait in the queue");
    assert!(
        error
            .to_string()
            .contains("use handoff to change the model")
    );
    assert_eq!(oga_service::count_follow_ups(&store, "running").unwrap(), 1);
    let events = store
        .repositories()
        .events()
        .list("running")
        .expect("events");
    assert_eq!(events.last().expect("rejection").kind, "steer_rejected");
}

#[tokio::test]
async fn resume_without_a_session_ships_the_original_task_and_records_the_fallback() {
    let (directory, store, dispatcher) = service();
    let cwd = directory.path().to_str().unwrap();
    // The fixture task has no captured session, matching a task that is
    // resumed for the first time after waiting behind a dependency hold.
    let mut fixture = task("no-session", cwd, TaskState::Blocked);
    fixture.prompt = "MARKER-BRIEF-BODY: build the widget".into();
    store.repositories().tasks().insert(&fixture).expect("task");
    resume(&dispatcher, ResumeRequest::new("no-session"))
        .await
        .expect("resume");
    assert_eq!(
        wait_for_settlement(&dispatcher, "no-session").await,
        TaskState::Completed
    );
    let task = dispatcher.task("no-session").expect("task");
    let shipped = task.shipped_prompt.expect("shipped prompt");
    assert!(
        shipped.contains("MARKER-BRIEF-BODY"),
        "a resume with no session to reopen must carry the original task text: {shipped}"
    );
    let events = store
        .repositories()
        .events()
        .list("no-session")
        .expect("events");
    assert!(
        events.iter().any(|event| event.kind == "resume_fallback"),
        "the fallback must be recorded in the task's event trace"
    );
}

#[tokio::test]
async fn a_session_captured_during_the_fallback_run_lets_the_next_resume_reopen_it() {
    let (directory, store, dispatcher) = service();
    let bin = directory.path().join("bin");
    fs::create_dir(&bin).expect("bin");
    let capture = directory.path().join("captured-argv");
    let claude = bin.join("claude");
    fs::write(
        &claude,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\nprintf '{{\"type\":\"system\",\"session_id\":\"fresh-session-xyz\"}}\\n'\nprintf 'finished\\nOGA_RESULT: completed\\n'\n",
            capture.display()
        ),
    )
    .expect("fake claude");
    let mut permissions = fs::metadata(&claude).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&claude, permissions).expect("permissions");
    let session_profile = Profile {
        id: "session".into(),
        label: "session".into(),
        provider: Provider::Claude,
        default_model: "session-model".into(),
        enabled: true,
        env: BTreeMap::from([("PATH".into(), format!("{}:/usr/bin:/bin", bin.display()))]),
        capabilities: vec![],
        command: None,
    };
    store
        .repositories()
        .profiles()
        .insert(&session_profile, "2026-01-01T00:00:00.000Z")
        .expect("session profile");
    let cwd = directory.path().to_str().unwrap();
    // No session captured yet: the first resume must fall back to the rebuilt
    // brief, same as the no-session case above, but on a provider that can
    // actually report a session id once it runs.
    let mut fixture = task("captures-session", cwd, TaskState::Blocked);
    fixture.profile_id = "session".into();
    store.repositories().tasks().insert(&fixture).expect("task");
    resume(&dispatcher, ResumeRequest::new("captures-session"))
        .await
        .expect("resume");
    assert_eq!(
        wait_for_settlement(&dispatcher, "captures-session").await,
        TaskState::Completed
    );
    let after_first_run = dispatcher.task("captures-session").expect("task");
    assert_eq!(
        after_first_run.session_id.as_deref(),
        Some("fresh-session-xyz"),
        "the session the fallback run captured must be recorded on the task"
    );

    // A second resume now has a session to reopen, so it must take the short
    // path — no rebuilt brief, no second fallback — and must hand the
    // captured session id back to the provider instead of starting over.
    resume(
        &dispatcher,
        ResumeRequest::new("captures-session").instruction("keep going"),
    )
    .await
    .expect("second resume");
    assert_eq!(
        wait_for_settlement(&dispatcher, "captures-session").await,
        TaskState::Completed
    );
    let events = store
        .repositories()
        .events()
        .list("captures-session")
        .expect("events");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "resume_fallback")
            .count(),
        1,
        "only the first resume — the one with no session — should have fallen back"
    );
    let captured = fs::read_to_string(&capture).expect("captured argv");
    assert!(
        captured.contains("fresh-session-xyz"),
        "the second run must reopen the captured session instead of starting fresh: {captured}"
    );
}

#[tokio::test]
async fn resume_with_a_reopenable_session_keeps_the_short_instruction() {
    let (directory, store, dispatcher) = service();
    let bin = directory.path().join("bin");
    fs::create_dir(&bin).expect("bin");
    let capture = directory.path().join("captured-argv");
    let claude = bin.join("claude");
    fs::write(
        &claude,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nprintf 'finished\\nOGA_RESULT: completed\\n'\n",
            capture.display()
        ),
    )
    .expect("fake claude");
    let mut permissions = fs::metadata(&claude).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&claude, permissions).expect("permissions");
    let session_profile = Profile {
        id: "session".into(),
        label: "session".into(),
        provider: Provider::Claude,
        default_model: "session-model".into(),
        enabled: true,
        env: BTreeMap::from([("PATH".into(), format!("{}:/usr/bin:/bin", bin.display()))]),
        capabilities: vec![],
        command: None,
    };
    store
        .repositories()
        .profiles()
        .insert(&session_profile, "2026-01-01T00:00:00.000Z")
        .expect("session profile");
    let cwd = directory.path().to_str().unwrap();
    let mut fixture = task("has-session", cwd, TaskState::Failed);
    fixture.profile_id = "session".into();
    fixture.session_id = Some("session-1".into());
    fixture.prompt = "MARKER-BRIEF-BODY: build the widget".into();
    store.repositories().tasks().insert(&fixture).expect("task");
    resume(&dispatcher, ResumeRequest::new("has-session"))
        .await
        .expect("resume");
    assert_eq!(
        wait_for_settlement(&dispatcher, "has-session").await,
        TaskState::Completed
    );
    let captured = fs::read_to_string(&capture).expect("captured argv");
    assert!(
        !captured.contains("MARKER-BRIEF-BODY"),
        "a reopenable session already holds the brief; resending it wastes the run: {captured}"
    );
    let events = store
        .repositories()
        .events()
        .list("has-session")
        .expect("events");
    assert!(
        !events.iter().any(|event| event.kind == "resume_fallback"),
        "a resume that can reopen its session must not trigger the fallback"
    );
}

#[tokio::test]
async fn handoff_to_a_different_profile_starts_fresh_with_the_rebuilt_brief() {
    let (directory, store, dispatcher) = service();
    let cwd = directory.path().to_str().unwrap();
    let mut fixture = task("cross-profile", cwd, TaskState::Failed);
    fixture.profile_id = "one".into();
    fixture.session_id = Some("stale-session-from-profile-one".into());
    fixture.prompt = "MARKER-BRIEF-BODY: build the widget".into();
    store.repositories().tasks().insert(&fixture).expect("task");
    handoff(
        &dispatcher,
        HandoffRequest::new("cross-profile").profile("two"),
    )
    .await
    .expect("handoff");
    assert_eq!(
        wait_for_settlement(&dispatcher, "cross-profile").await,
        TaskState::Completed
    );
    let task = dispatcher.task("cross-profile").expect("task");
    assert_eq!(
        task.session_id, None,
        "a provider session belongs to one account; the destination must not be handed the source's session id"
    );
    let shipped = task.shipped_prompt.expect("shipped prompt");
    assert!(
        shipped.contains("MARKER-BRIEF-BODY"),
        "a cross-profile handoff must carry the rebuilt brief: {shipped}"
    );
    let events = store
        .repositories()
        .events()
        .list("cross-profile")
        .expect("events");
    assert!(
        events.iter().any(|event| event.kind == "handoff_brief"),
        "the rebuilt brief must be recorded in the task's event trace"
    );
}

/// A Claude profile whose `claude` binary is the given shell script.
fn claude_profile(directory: &TempDir, store: &Store, id: &str, script: &str) -> Profile {
    let bin = directory.path().join(format!("bin-{id}"));
    fs::create_dir(&bin).expect("bin");
    let claude = bin.join("claude");
    fs::write(&claude, script).expect("fake claude");
    let mut permissions = fs::metadata(&claude).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&claude, permissions).expect("permissions");
    let profile = Profile {
        id: id.into(),
        label: id.into(),
        provider: Provider::Claude,
        default_model: "session-model".into(),
        enabled: true,
        env: BTreeMap::from([("PATH".into(), format!("{}:/usr/bin:/bin", bin.display()))]),
        capabilities: vec![],
        command: None,
    };
    store
        .repositories()
        .profiles()
        .insert(&profile, "2026-01-01T00:00:00.000Z")
        .expect("claude profile");
    profile
}

async fn wait_for_state(dispatcher: &Dispatcher, id: &str, wanted: TaskState) -> TaskState {
    let mut state = dispatcher.task(id).expect("task").state;
    for _ in 0..2_000 {
        state = dispatcher.task(id).expect("task").state;
        if state == wanted {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    state
}

#[tokio::test]
async fn handoff_off_a_rate_limited_account_drops_its_hold_and_starts_now() {
    let (directory, store, dispatcher) = service();
    let capture = directory.path().join("captured-argv");
    claude_profile(
        &directory,
        &store,
        "session",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nprintf 'finished\\nOGA_RESULT: completed\\n'\n",
            capture.display()
        ),
    );
    let cwd = directory.path().to_str().expect("cwd");
    let mut fixture = task("rate-limited", cwd, TaskState::Failed);
    fixture.prompt = "MARKER-BRIEF-BODY: build the widget".into();
    fixture.session_id = Some("session-from-profile-one".into());
    fixture.output = "got partway".into();
    fixture.error = Some("usage limit reached".into());
    fixture.completion = Some(TaskCompletion {
        exit_code: Some(1),
        blocked: true,
        code: CompletionCode::RateLimit,
        reason: Some("usage limit reached".into()),
        suggested_scope: None,
        resets_at: Some("2099-01-01T00:00:00.000Z".into()),
        asserted_completion: None,
        dependency_blocked: None,
    });
    store.repositories().tasks().insert(&fixture).expect("task");
    // Parked exactly as a rate-limited run parks: waiting on the account that
    // ran out, not on the clock alone.
    let hold = TaskHold {
        task_id: "rate-limited".into(),
        verb: HoldVerb::Resume,
        args: HoldArgs::default(),
        start_at: Some("2099-01-01T00:00:00.000Z".into()),
        await_profile: Some("one".into()),
        await_model: Some("model-one".into()),
        next_check_at: "2099-01-01T00:00:00.000Z".into(),
        expires_at: "2099-01-02T00:00:00.000Z".into(),
        probe_count: 0,
        note: "waiting for usage to reset".into(),
        created_at: "2026-01-01T00:00:00.000Z".into(),
        updated_at: "2026-01-01T00:00:00.000Z".into(),
    };
    oga_service::arm_hold(&store, &hold).expect("hold armed");
    assert_eq!(
        dispatcher.task("rate-limited").expect("task").state,
        TaskState::Pending
    );

    let moved = handoff(
        &dispatcher,
        HandoffRequest::new("rate-limited").profile("session"),
    )
    .await
    .expect("handoff");
    assert_ne!(
        moved.state,
        TaskState::Pending,
        "the new account has usage; the task must not keep waiting for the old account's reset"
    );
    assert_eq!(moved.hold, None, "the old account's hold must be dropped");
    assert_eq!(
        moved.session_id, None,
        "a provider session belongs to one account and cannot move with the task"
    );

    assert_eq!(
        wait_for_settlement(&dispatcher, "rate-limited").await,
        TaskState::Completed
    );
    let captured = fs::read_to_string(&capture).expect("captured argv");
    assert!(
        !captured.contains("--resume"),
        "the destination cannot be handed the source account's session id: {captured}"
    );
    assert!(
        captured.contains("MARKER-BRIEF-BODY"),
        "the fresh run must be seeded with the original task: {captured}"
    );
    let settled = dispatcher.task("rate-limited").expect("task");
    assert_eq!(settled.profile_id, "session");
    assert_eq!(
        settled.attempts.len(),
        1,
        "the rate-limited run must be filed as an attempt"
    );
    let events = store
        .repositories()
        .events()
        .list("rate-limited")
        .expect("events");
    assert!(
        events.iter().any(|event| event.kind == "hold_released"),
        "the move out of the wait must be recorded in the task's event trace"
    );
    assert!(
        events.iter().any(|event| event.kind == "handoff_brief"),
        "the rebuilt brief must be recorded in the task's event trace"
    );
}

#[tokio::test]
async fn a_session_the_provider_rejects_is_forgotten_and_the_task_runs_fresh() {
    let (directory, store, dispatcher) = service();
    let capture = directory.path().join("captured-argv");
    claude_profile(
        &directory,
        &store,
        "session",
        &format!(
            "#!/bin/sh\nfor arg in \"$@\"; do\n  if [ \"$arg\" = \"--resume\" ]; then\n    echo 'No conversation found with session ID: stale-session' >&2\n    exit 1\n  fi\ndone\nprintf '%s\\n' \"$@\" > {}\nprintf 'fresh run\\nOGA_RESULT: completed\\n'\n",
            capture.display()
        ),
    );
    let cwd = directory.path().to_str().expect("cwd");
    let mut fixture = task("rejected-session", cwd, TaskState::Failed);
    fixture.profile_id = "session".into();
    fixture.model = "session-model".into();
    fixture.prompt = "MARKER-BRIEF-BODY: build the widget".into();
    fixture.session_id = Some("stale-session".into());
    store.repositories().tasks().insert(&fixture).expect("task");

    resume(&dispatcher, ResumeRequest::new("rejected-session"))
        .await
        .expect("resume");
    assert_eq!(
        wait_for_state(&dispatcher, "rejected-session", TaskState::Completed).await,
        TaskState::Completed,
        "a rejected session must not fail the task the same way twice"
    );
    let settled = dispatcher.task("rejected-session").expect("task");
    assert_eq!(settled.session_id, None);
    let captured = fs::read_to_string(&capture).expect("captured argv");
    assert!(
        captured.contains("MARKER-BRIEF-BODY"),
        "the retry must be seeded with the original task: {captured}"
    );
    let events = store
        .repositories()
        .events()
        .list("rejected-session")
        .expect("events");
    assert!(
        events.iter().any(|event| event.kind == "session_rejected"),
        "the rejected session must be recorded in the task's event trace"
    );
}

#[tokio::test]
async fn handoff_from_a_running_task_does_not_let_the_old_run_fail_the_new_one() {
    let (directory, store, dispatcher) = service();
    let current = store
        .repositories()
        .profiles()
        .get("one")
        .expect("profile lookup")
        .expect("profile one");
    let mut slow = current.clone();
    slow.command = Some(vec![
        "sh".into(),
        "-c".into(),
        "sleep 1; printf 'old run\nOGA_RESULT: completed\n'".into(),
    ]);
    assert!(
        store
            .repositories()
            .profiles()
            .update_if_unchanged("one", &current, &slow, "2026-01-01T00:00:01.000Z",)
            .expect("update slow profile")
    );

    let task = dispatcher
        .dispatch(oga_service::DispatchRequest::new(
            "one",
            "running work",
            directory.path(),
        ))
        .await
        .expect("dispatch")
        .task;
    for _ in 0..200 {
        if dispatcher.task(&task.id).expect("task").state == TaskState::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        dispatcher.task(&task.id).expect("running task").state,
        TaskState::Running
    );

    handoff(&dispatcher, HandoffRequest::new(&task.id).profile("two"))
        .await
        .expect("handoff running task");
    assert_eq!(
        wait_for_settlement(&dispatcher, &task.id).await,
        TaskState::Completed
    );
    let handed_off = dispatcher.task(&task.id).expect("handed-off task");
    assert_eq!(handed_off.profile_id, "two");
    assert_eq!(handed_off.attempts.len(), 1);
}

#[tokio::test]
async fn queued_follow_up_starts_after_clean_completion_in_same_session() {
    use oga_service::FollowUpQueue;
    let (directory, store, dispatcher) = service();
    // Dispatch a task that will complete cleanly. The fake runner sleeps 0.05,
    // giving us a window to queue a follow-up while it is still running.
    let task = dispatcher
        .dispatch(oga_service::DispatchRequest::new(
            "one",
            "first work",
            directory.path(),
        ))
        .await
        .expect("dispatch")
        .task;
    // Queue while the task is still working — this is the path the bug dropped.
    let queue = FollowUpQueue::new(store.clone());
    // The task is queued/running at this point; either state is accepted.
    let current = dispatcher.task(&task.id).expect("task").state;
    queue
        .queue(&task.id, current, "second instruction")
        .expect("queue first");
    queue
        .queue(&task.id, current, "third instruction")
        .expect("queue second");

    // Wait for the chain to drain: first run completes, second and third follow.
    // Poll until the queue is empty and the task has settled with two attempts
    // (first run filed, second and third each file one).
    for _ in 0..400 {
        let task = dispatcher.task(&task.id).expect("task");
        let waiting = queue.count(&task.id).unwrap_or(0);
        if waiting == 0 && task.state == TaskState::Completed && task.attempts.len() >= 2 {
            // Verify FIFO: the shipped prompt of the final run should contain the
            // third instruction (the last to run), and events should show two starts.
            let events = store
                .repositories()
                .events()
                .list(&task.id)
                .expect("events");
            let started = events
                .iter()
                .filter(|e| e.kind == "follow_up_started")
                .count();
            assert_eq!(
                started, 2,
                "both queued instructions should have started, FIFO"
            );
            assert!(
                task.shipped_prompt
                    .as_deref()
                    .unwrap_or_default()
                    .contains("third instruction"),
                "final run should carry the last queued instruction"
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("queued follow-ups did not drain after clean completion");
}

#[tokio::test]
async fn failed_run_leaves_queued_follow_ups_intact_and_pauses() {
    use oga_service::FollowUpQueue;
    let (directory, store, dispatcher) = service();
    // Profile that fails: non-zero exit with a failed marker. The runner will
    // interpret this as TaskState::Failed, which must leave the queue untouched.
    let failing = Profile {
        id: "failing".into(),
        label: "failing".into(),
        provider: Provider::Claude,
        default_model: "model-one".into(),
        enabled: true,
        env: BTreeMap::new(),
        capabilities: vec![],
        command: Some(vec![
            "sh".into(),
            "-c".into(),
            "printf 'oops\\nOGA_RESULT: failed\\n'; exit 1".into(),
        ]),
    };
    store
        .repositories()
        .profiles()
        .insert(&failing, "2026-01-01T00:00:00.000Z")
        .expect("failing profile");
    let task = dispatcher
        .dispatch(oga_service::DispatchRequest::new(
            "failing",
            "work that fails",
            directory.path(),
        ))
        .await
        .expect("dispatch")
        .task;
    let queue = FollowUpQueue::new(store.clone());
    let current = dispatcher.task(&task.id).expect("task").state;
    queue
        .queue(&task.id, current, "should stay queued")
        .expect("queue");

    // Wait for the task to settle as failed.
    for _ in 0..200 {
        let task = dispatcher.task(&task.id).expect("task");
        if task.state == TaskState::Failed {
            let waiting = queue.count(&task.id).expect("count");
            assert_eq!(waiting, 1, "failed run must leave queue intact");
            let events = store
                .repositories()
                .events()
                .list(&task.id)
                .expect("events");
            assert!(
                events.iter().any(|e| e.kind == "follow_ups_paused"),
                "failed run with waiting items must emit follow_ups_paused"
            );
            assert!(
                !events.iter().any(|e| e.kind == "follow_up_started"),
                "failed run must not start a follow-up"
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("task did not settle as failed");
}

/// The dependency trigger has to fire for a completion written straight to the
/// store, not only for one a provider run reached: a blocker asserted complete
/// after it ended badly both revives the dependents that ending dropped and
/// starts the ones now clear to run.
#[tokio::test]
async fn asserted_completion_releases_the_tasks_waiting_on_it() {
    let (directory, store, dispatcher) = service();
    let cwd = directory.path().to_str().expect("cwd");
    seed(&store, "blocker", cwd, TaskState::Blocked);
    seed(&store, "dependent", cwd, TaskState::Queued);
    let now = "2026-01-01T00:00:00.000Z";
    oga_service::add_dependencies(&store, "dependent", &["blocker".into()], now)
        .expect("dependency edge");
    let hold = oga_service::dependency_hold(
        "dependent",
        &[dispatcher.task("blocker").expect("blocker")],
        oga_domain::OnBlockerFailure::Hold,
        None,
        now,
    );
    oga_service::arm_hold(&store, &hold).expect("hold armed");

    force_complete(
        &dispatcher,
        CompletionAssertion::new("blocker", "operator", "work landed by hand"),
    )
    .expect("blocker completed");

    assert_eq!(
        wait_for_settlement(&dispatcher, "dependent").await,
        TaskState::Completed
    );
}

#[tokio::test]
async fn handoff_keeps_dependency_hold_until_prerequisite_completes() {
    let (directory, store, dispatcher) = service();
    let blocker_profile = store
        .repositories()
        .profiles()
        .get("one")
        .expect("profile lookup")
        .expect("profile one");
    let mut slow = blocker_profile.clone();
    slow.command = Some(vec![
        "sh".into(),
        "-c".into(),
        "sleep 0.05; printf 'blocker done\\nOGA_RESULT: completed\\n'".into(),
    ]);
    assert!(
        store
            .repositories()
            .profiles()
            .update_if_unchanged("one", &blocker_profile, &slow, "2026-01-01T00:00:01.000Z",)
            .expect("update slow profile")
    );
    let cwd = directory.path().to_path_buf();
    let blocker = dispatcher
        .dispatch(oga_service::DispatchRequest::new(
            "one",
            "blocker work",
            &cwd,
        ))
        .await
        .expect("dispatch blocker")
        .task;
    for _ in 0..200 {
        if dispatcher.task(&blocker.id).expect("blocker").state == TaskState::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        dispatcher.task(&blocker.id).expect("running blocker").state,
        TaskState::Running
    );

    let dependent = dispatcher
        .dispatch(
            oga_service::DispatchRequest::new("two", "dependent work", &cwd)
                .depends_on([blocker.id.clone()]),
        )
        .await
        .expect("dispatch dependent")
        .task;
    assert_eq!(dependent.state, TaskState::Pending);
    let original_hold = dependent.hold.clone().expect("dependency hold");

    let handed_off = handoff(
        &dispatcher,
        HandoffRequest::new(&dependent.id).profile("one"),
    )
    .await
    .expect("handoff dependent");
    assert_eq!(handed_off.state, TaskState::Pending);
    assert_eq!(handed_off.profile_id, "one");
    assert_eq!(handed_off.model, "model-one");
    assert_eq!(handed_off.hold, Some(original_hold.clone()));
    assert_eq!(
        oga_service::dependencies_of(&store, &dependent.id)
            .expect("dependencies")
            .into_iter()
            .map(|task| task.id)
            .collect::<Vec<_>>(),
        vec![blocker.id.clone()]
    );

    assert_eq!(
        wait_for_settlement(&dispatcher, &blocker.id).await,
        TaskState::Completed
    );
    assert_eq!(
        wait_for_settlement(&dispatcher, &dependent.id).await,
        TaskState::Completed
    );
    let completed = dispatcher.task(&dependent.id).expect("completed dependent");
    assert_eq!(completed.profile_id, "one");
    assert_eq!(completed.model, "model-one");
}
