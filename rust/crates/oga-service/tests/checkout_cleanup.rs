//! Automatic checkout collection: which copies of a repository are removed
//! when their work is finished, and which stay with a reason.

use std::{collections::BTreeMap, fs, path::Path, process::Command, sync::Arc};

use oga_domain::{
    BranchOutcome, Profile, Provider, Task, TaskKind, TaskScope, TaskState, WorktreeRequest,
};
use oga_service::{Dispatcher, sweep_checkouts};
use oga_store::Store;
use oga_worktree::{branch_exists, create_task_worktree_at};
use tempfile::TempDir;

/// Far enough ahead that every fixture task has aged past the retention.
const CUTOFF: &str = "2999-01-01T00:00:00.000Z";

struct Fixture {
    /// Keeps the temporary repository and worktrees root alive for the test.
    _directory: TempDir,
    store: Arc<Store>,
    dispatcher: Dispatcher,
    repo: std::path::PathBuf,
    worktrees: std::path::PathBuf,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("temporary directory");
    let repo = directory.path().join("project");
    let worktrees = directory.path().join("worktrees");
    fs::create_dir(&repo).expect("repository directory");
    git(&repo, &["init", "-q", "-b", "main"]);
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
            "-q",
            "-m",
            "initial",
        ],
    );
    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
    store
        .repositories()
        .profiles()
        .insert(
            &Profile {
                id: "one".into(),
                label: "one".into(),
                provider: Provider::Claude,
                default_model: "model-one".into(),
                enabled: true,
                env: BTreeMap::new(),
                capabilities: vec![],
                command: None,
            },
            "2026-01-01T00:00:00.000Z",
        )
        .expect("profile");
    let dispatcher = Dispatcher::new(store.clone(), oga_runner::ProviderRunner::default())
        .with_worktrees_root(worktrees.clone());
    Fixture {
        _directory: directory,
        store,
        dispatcher,
        repo,
        worktrees,
    }
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

fn task(id: &str, cwd: &str, state: TaskState) -> Task {
    Task {
        id: id.into(),
        kind: Some(TaskKind::Delegated),
        profile_id: "one".into(),
        model: "model-one".into(),
        prompt: "work".into(),
        cwd: cwd.into(),
        state,
        created_at: "2026-01-01T00:00:00.000Z".into(),
        updated_at: "2026-01-01T00:00:00.000Z".into(),
        scope: TaskScope {
            read: vec!["**".into()],
            write: vec!["**".into()],
        },
        ..Task::default()
    }
}

/// Records a task that already ran in `checkout`, the way dispatch leaves it.
fn record_task(
    fixture: &Fixture,
    id: &str,
    checkout: &oga_domain::TaskWorktree,
    state: TaskState,
    archived: bool,
) {
    let mut fixture_task = task(id, &checkout.origin_cwd, state);
    fixture_task.branch = Some(checkout.branch.clone());
    fixture_task.worktree = Some(checkout.clone());
    fixture_task.archived_at = archived.then(|| "2026-01-02T00:00:00.000Z".to_owned());
    fixture
        .store
        .repositories()
        .tasks()
        .insert(&fixture_task)
        .expect("task");
    fixture
        .store
        .transaction(|tx| {
            tx.execute(
                "UPDATE tasks SET origin_cwd=?,worktree_path=?,worktree_branch=? WHERE id=?",
                rusqlite::params![&checkout.origin_cwd, &checkout.path, &checkout.branch, id,],
            )?;
            Ok(())
        })
        .expect("checkout recorded");
}

fn set_updated_at(fixture: &Fixture, id: &str, updated_at: &str) {
    fixture
        .store
        .transaction(|tx| {
            tx.execute(
                "UPDATE tasks SET updated_at=? WHERE id=?",
                rusqlite::params![updated_at, id],
            )?;
            Ok(())
        })
        .expect("updated_at");
}

async fn make_checkout(fixture: &Fixture, task_id: &str, title: &str) -> oga_domain::TaskWorktree {
    create_task_worktree_at(
        &fixture.worktrees,
        &fixture.repo,
        task_id,
        &WorktreeRequest::default(),
        Some(title),
    )
    .await
    .expect("worktree created")
    .worktree
}

async fn sweep(fixture: &Fixture) -> oga_service::CheckoutSweep {
    sweep_checkouts(&fixture.dispatcher, CUTOFF, true)
        .await
        .expect("sweep")
}

#[tokio::test]
async fn a_finished_archived_checkout_is_collected_and_its_branch_stays() {
    let fixture = fixture();
    let checkout = make_checkout(&fixture, "solo", "solo work").await;
    record_task(&fixture, "solo", &checkout, TaskState::Completed, true);

    let report = sweep(&fixture).await;

    assert_eq!(report.removed, vec!["solo".to_owned()]);
    assert!(report.kept.is_empty(), "{:?}", report.kept);
    assert!(!Path::new(&checkout.path).exists());
    assert!(
        branch_exists(&fixture.repo, &checkout.branch)
            .await
            .expect("branch inspected"),
        "collection keeps the branch"
    );
}

#[tokio::test]
async fn a_settled_unarchived_sibling_does_not_hold_a_shared_checkout() {
    let fixture = fixture();
    let checkout = make_checkout(&fixture, "owner", "shared work").await;
    record_task(&fixture, "owner", &checkout, TaskState::Completed, true);
    record_task(&fixture, "reader", &checkout, TaskState::Completed, false);

    let report = sweep(&fixture).await;

    assert_eq!(report.removed, vec!["owner".to_owned()]);
    assert!(!Path::new(&checkout.path).exists());
}

#[tokio::test]
async fn an_unarchived_finished_checkout_is_collected_when_the_setting_allows_it() {
    let fixture = fixture();
    let checkout = make_checkout(&fixture, "loose", "loose work").await;
    record_task(&fixture, "loose", &checkout, TaskState::Completed, false);

    // Archived-only, the default, leaves unarchived work alone.
    let archived_only = sweep_checkouts(&fixture.dispatcher, CUTOFF, true)
        .await
        .expect("sweep");
    assert!(archived_only.removed.is_empty(), "{archived_only:?}");
    assert!(Path::new(&checkout.path).exists());

    let report = sweep_checkouts(&fixture.dispatcher, CUTOFF, false)
        .await
        .expect("sweep");
    assert_eq!(report.removed, vec!["loose".to_owned()]);
    assert!(!Path::new(&checkout.path).exists());
}

#[tokio::test]
async fn a_settled_checkout_younger_than_the_retention_stays() {
    let fixture = fixture();
    let fresh = make_checkout(&fixture, "fresh", "fresh work").await;
    let old = make_checkout(&fixture, "old", "old work").await;
    record_task(&fixture, "fresh", &fresh, TaskState::Completed, true);
    record_task(&fixture, "old", &old, TaskState::Completed, true);
    set_updated_at(&fixture, "fresh", "2026-09-01T00:00:00.000Z");
    set_updated_at(&fixture, "old", "2026-01-01T00:00:00.000Z");

    let report = sweep_checkouts(&fixture.dispatcher, "2026-06-01T00:00:00.000Z", true)
        .await
        .expect("sweep");

    assert_eq!(report.removed, vec!["old".to_owned()]);
    assert!(!Path::new(&old.path).exists());
    assert!(
        Path::new(&fresh.path).exists(),
        "a recent task keeps its checkout"
    );
}

#[tokio::test]
async fn a_checkout_a_live_task_still_uses_stays_with_a_reason() {
    let fixture = fixture();
    let checkout = make_checkout(&fixture, "owner", "live work").await;
    record_task(&fixture, "owner", &checkout, TaskState::Completed, true);
    record_task(&fixture, "writer", &checkout, TaskState::Running, false);

    let report = sweep(&fixture).await;

    assert!(report.removed.is_empty(), "{:?}", report.removed);
    assert_eq!(
        report.kept,
        vec![oga_service::CheckoutKept {
            subject: "owner".into(),
            reason: "kept: shared with a live task still using this checkout (writer)".into(),
        }]
    );
    assert!(Path::new(&checkout.path).exists());
}

#[tokio::test]
async fn a_checkout_with_uncommitted_work_is_kept_and_says_why() {
    let fixture = fixture();
    let checkout = make_checkout(&fixture, "dirty", "dirty work").await;
    fs::write(
        Path::new(&checkout.path).join("unfinished.txt"),
        "unfinished\n",
    )
    .expect("uncommitted file");
    record_task(&fixture, "dirty", &checkout, TaskState::Completed, true);

    let report = sweep(&fixture).await;

    assert!(report.removed.is_empty(), "{:?}", report.removed);
    assert_eq!(report.kept.len(), 1);
    assert!(
        report.kept[0].reason.contains("uncommitted changes"),
        "{:?}",
        report.kept
    );
    assert!(Path::new(&checkout.path).exists());
}

#[tokio::test]
async fn a_removal_a_crashed_broker_left_behind_is_finished() {
    let fixture = fixture();
    let checkout = make_checkout(&fixture, "stuck", "stuck work").await;
    record_task(
        &fixture,
        "stuck",
        &checkout,
        TaskState::RemovingCheckout,
        true,
    );
    fixture
        .store
        .transaction(|tx| {
            tx.execute(
                "UPDATE tasks SET checkout_state='completed' WHERE id='stuck'",
                [],
            )?;
            Ok(())
        })
        .expect("checkout state");

    let report = sweep(&fixture).await;

    assert_eq!(report.removed, vec!["stuck".to_owned()]);
    assert!(!Path::new(&checkout.path).exists());
    assert_eq!(
        fixture.dispatcher.task("stuck").expect("task").state,
        TaskState::Completed
    );
}

#[tokio::test]
async fn an_unowned_checkout_left_by_an_earlier_run_is_collected() {
    let fixture = fixture();
    let checkout = make_checkout(&fixture, "stranded", "stranded work").await;
    // No task row references it, as if the row was pruned from the store.
    assert!(Path::new(&checkout.path).exists());

    let report = sweep(&fixture).await;

    assert_eq!(report.orphans_removed, vec![checkout.path.clone()]);
    assert!(!Path::new(&checkout.path).exists());
}

#[tokio::test]
async fn an_unowned_checkout_with_work_is_left_alone() {
    let fixture = fixture();
    let checkout = make_checkout(&fixture, "stranded", "stranded work").await;
    fs::write(
        Path::new(&checkout.path).join("unfinished.txt"),
        "unfinished\n",
    )
    .expect("uncommitted file");

    let report = sweep(&fixture).await;

    assert!(
        report.orphans_removed.is_empty(),
        "{:?}",
        report.orphans_removed
    );
    assert_eq!(report.kept.len(), 1);
    assert!(Path::new(&checkout.path).exists());
    assert!(
        report.kept[0].reason.contains("uncommitted changes"),
        "{:?}",
        report.kept
    );
}

#[tokio::test]
async fn an_unmerged_branch_survives_its_checkout() {
    let fixture = fixture();
    let checkout = make_checkout(&fixture, "unmerged", "unmerged work").await;
    fs::write(Path::new(&checkout.path).join("tracked.txt"), "one\ntwo\n").expect("edit");
    git(Path::new(&checkout.path), &["add", "tracked.txt"]);
    git(
        Path::new(&checkout.path),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "-m",
            "unmerged",
        ],
    );
    record_task(&fixture, "unmerged", &checkout, TaskState::Completed, true);

    // The checkout is clean, so it goes; the branch has commits main does not,
    // and collection never deletes a branch at all.
    let report = sweep(&fixture).await;

    assert_eq!(report.removed, vec!["unmerged".to_owned()]);
    assert!(
        branch_exists(&fixture.repo, &checkout.branch)
            .await
            .expect("branch inspected")
    );
    let outcome = oga_worktree::remove_task_branch_safely(&checkout)
        .await
        .expect("branch inspected");
    assert_eq!(outcome.outcome, BranchOutcome::Kept);
}
