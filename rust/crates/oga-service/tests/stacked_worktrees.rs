//! Worktree tasks chained with `dependsOn`, each starting from the branch the
//! one before it worked on.

use std::{collections::BTreeMap, fs, path::Path, process::Command, sync::Arc, time::Duration};

use oga_domain::{Profile, Provider, Task, TaskState, WorktreeOption, WorktreeRequest};
use oga_service::{DispatchRequest, Dispatcher};
use oga_store::Store;
use tempfile::TempDir;

const COMMITS: &str = "git -c user.name=Worker -c user.email=worker@example.com -c commit.gpgsign=false commit -q --allow-empty -m \"work on $(git branch --show-current)\" && printf 'done\\nOGA_RESULT: completed\\n'";
const DROPS_ITS_BRANCH: &str = "branch=$(git branch --show-current) && git checkout -q --detach && git branch -q -D \"$branch\" && printf 'done\\nOGA_RESULT: completed\\n'";

struct Fixture {
    directory: TempDir,
    repo: std::path::PathBuf,
    dispatcher: Dispatcher,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("temporary directory");
    let repo = directory.path().join("project");
    fs::create_dir(&repo).expect("repository directory");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "first",
        ],
    );
    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
    for (id, command) in [("commits", COMMITS), ("drops", DROPS_ITS_BRANCH)] {
        store
            .repositories()
            .profiles()
            .insert(
                &Profile {
                    id: id.into(),
                    label: id.into(),
                    provider: Provider::Claude,
                    default_model: "fake-model".into(),
                    enabled: true,
                    env: BTreeMap::new(),
                    capabilities: vec![],
                    command: Some(vec!["sh".into(), "-c".into(), command.into()]),
                },
                "2026-01-01T00:00:00.000Z",
            )
            .expect("profile");
    }
    store
        .repositories()
        .settings()
        .put(
            &oga_config::canonical_cwd(oga_config::global_cwd())
                .display()
                .to_string(),
            oga_config::MODEL_SETTINGS_KEY,
            &serde_json::json!({
                "profiles": {
                    "commits": { "modelEnabled": { "fake-model": true } },
                    "drops": { "modelEnabled": { "fake-model": true } },
                }
            })
            .to_string(),
            "2026-01-01T00:00:00.000Z",
        )
        .expect("model settings");
    let dispatcher = Dispatcher::new(store, oga_runner::ProviderRunner::default())
        .with_worktrees_root(directory.path().join("worktrees"));
    Fixture {
        directory,
        repo,
        dispatcher,
    }
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git is installed");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git output")
        .trim()
        .to_owned()
}

fn worktree(branch: &str, from: Option<&str>) -> WorktreeOption {
    WorktreeOption::Request(WorktreeRequest {
        branch: Some(branch.into()),
        from: from.map(str::to_owned),
        ..WorktreeRequest::default()
    })
}

async fn delegate(
    fixture: &Fixture,
    profile: &str,
    branch: &str,
    from: Option<&str>,
    depends_on: Option<&Task>,
) -> Task {
    let mut request = DispatchRequest::new(profile, format!("work on {branch}"), &fixture.repo)
        .worktree(worktree(branch, from));
    if let Some(prerequisite) = depends_on {
        request = request.depends_on([prerequisite.id.clone()]);
    }
    fixture
        .dispatcher
        .dispatch(request)
        .await
        .expect("delegated")
        .task
}

async fn wait_until_done(dispatcher: &Dispatcher, id: &str) -> Task {
    for _ in 0..2_000 {
        let task = dispatcher.task(id).expect("task");
        if task.state.settled() || task.state == TaskState::Blocked {
            return task;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("task did not finish: {id}");
}

#[tokio::test]
async fn each_checkout_in_a_chain_starts_from_the_work_before_it() {
    let fixture = fixture();
    let first = delegate(&fixture, "commits", "launch/l1", None, None).await;
    let second = delegate(
        &fixture,
        "commits",
        "launch/l2",
        Some("launch/l1"),
        Some(&first),
    )
    .await;
    // `launch/l2` does not exist yet: the task that makes it has not started.
    let third = delegate(
        &fixture,
        "commits",
        "launch/l3",
        Some("launch/l2"),
        Some(&second),
    )
    .await;

    let second_checkout = second.worktree.clone().expect("worktree").path;
    assert_eq!(second.state, TaskState::Pending);
    assert!(!Path::new(&second_checkout).exists());

    let third = wait_until_done(&fixture.dispatcher, &third.id).await;
    assert_eq!(third.state, TaskState::Completed, "{:?}", third.error);
    let second = fixture.dispatcher.task(&second.id).expect("second task");

    let l1 = git(&fixture.repo, &["rev-parse", "launch/l1"]);
    let l2 = git(&fixture.repo, &["rev-parse", "launch/l2"]);
    assert_eq!(second.worktree.expect("worktree").base, Some(l1));
    assert_eq!(third.worktree.expect("worktree").base, Some(l2));
    assert_eq!(
        git(&fixture.repo, &["log", "--format=%s", "launch/l3"]),
        "work on launch/l3\nwork on launch/l2\nwork on launch/l1\nfirst"
    );
}

#[tokio::test]
async fn a_base_no_prerequisite_will_create_is_still_refused() {
    let fixture = fixture();
    let first = delegate(&fixture, "commits", "launch/l1", None, None).await;

    let refusal = fixture
        .dispatcher
        .dispatch(
            DispatchRequest::new("commits", "work on l2", &fixture.repo)
                .worktree(worktree("launch/l2", Some("launch/nowhere")))
                .depends_on([first.id.clone()]),
        )
        .await
        .expect_err("refused");

    assert!(
        refusal
            .to_string()
            .contains("worktree base is not a commit in this repository: launch/nowhere"),
        "{refusal}"
    );
}

#[tokio::test]
async fn a_prerequisite_that_leaves_no_branch_blocks_its_dependent() {
    let fixture = fixture();
    let first = delegate(&fixture, "drops", "launch/l1", None, None).await;
    let second = delegate(
        &fixture,
        "commits",
        "launch/l2",
        Some("launch/l1"),
        Some(&first),
    )
    .await;

    let second = wait_until_done(&fixture.dispatcher, &second.id).await;

    assert_eq!(
        fixture.dispatcher.task(&first.id).expect("first").state,
        TaskState::Completed
    );
    assert_eq!(second.state, TaskState::Blocked);
    assert!(
        second
            .error
            .as_deref()
            .is_some_and(|error| error.contains("worktree base branch launch/l1 does not exist")),
        "{:?}",
        second.error
    );
    let checkout = second.worktree.expect("worktree");
    assert!(!Path::new(&checkout.path).exists());
    assert_eq!(checkout.base, None);
    let branches = git(&fixture.repo, &["branch", "--list", "launch/*"]);
    assert!(branches.is_empty(), "{branches}");
}

#[tokio::test]
async fn a_task_without_prerequisites_gets_its_checkout_when_delegated() {
    let fixture = fixture();
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);

    let task = delegate(&fixture, "commits", "launch/l1", None, None).await;

    assert_eq!(task.state, TaskState::PreparingCheckout);
    assert_eq!(
        task.worktree.clone().expect("worktree").base,
        Some(head.clone())
    );
    let task = wait_until_done(&fixture.dispatcher, &task.id).await;
    assert_eq!(task.state, TaskState::Completed, "{:?}", task.error);
    assert_eq!(
        git(&fixture.repo, &["rev-parse", "launch/l1^"]),
        head,
        "{:?}",
        fixture.directory.path()
    );
}
