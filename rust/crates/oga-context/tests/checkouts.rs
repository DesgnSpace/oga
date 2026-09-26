//! An answer belongs to the checkout it was asked in.
//!
//! Every fixture here is a disposable git repository in a temporary directory,
//! so switching branches under a lookup is something the tests can actually do.

use std::fs;
use std::path::Path;
use std::process::Command;

use oga_context::{BuildOptions, ContextIndex, ContextTarget, LearnRouteProposal, QuestionOptions};
use oga_domain::{Task, TaskScope, TaskWorktree};
use oga_store::Store;
use tempfile::{TempDir, tempdir};

struct Checkout {
    dir: TempDir,
}

impl Checkout {
    /// A repository holding one source file on `main`, already committed.
    fn new(body: &str) -> Self {
        let dir = tempdir().expect("checkout directory is creatable");
        let checkout = Self { dir };
        checkout.git(&["init", "--quiet", "--initial-branch=main"]);
        checkout.git(&["config", "user.email", "fixture@example.invalid"]);
        checkout.git(&["config", "user.name", "Fixture"]);
        // Whoever runs these tests may sign their own commits. A fixture has
        // no key and no reason to reach for one.
        checkout.git(&["config", "commit.gpgsign", "false"]);
        checkout.write("src/billing.ts", body);
        checkout.commit("first");
        checkout
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, path: &str, body: &str) {
        let file = self.path().join(path);
        fs::create_dir_all(file.parent().expect("a file has a parent"))
            .expect("source directory is creatable");
        fs::write(file, body).expect("fixture file writes");
    }

    fn git(&self, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(self.path())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

    fn commit(&self, message: &str) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "--message", message]);
    }

    fn target(&self) -> ContextTarget {
        ContextTarget::new(self.path(), everything())
    }
}

fn everything() -> TaskScope {
    TaskScope {
        read: vec!["**".into()],
        write: vec!["**".into()],
    }
}

fn store() -> (TempDir, Store) {
    let database = tempdir().expect("database directory is creatable");
    let store =
        Store::open_writable(database.path().join("oga.db")).expect("checkout fixture store opens");
    (database, store)
}

/// The anchors an answer offers, as `path#symbol`.
fn anchors(index: &ContextIndex<'_>, target: &ContextTarget, question: &str) -> Vec<String> {
    index
        .question_with_options(target, question, QuestionOptions::default())
        .expect("a question is answerable")
        .candidates
        .into_iter()
        .map(|candidate| match candidate.symbol {
            Some(symbol) => format!("{}#{symbol}", candidate.path),
            None => candidate.path,
        })
        .collect()
}

const ON_MAIN: &str = "export function chargeCard(amount: number) {\n  return amount;\n}\n";
const ON_BRANCH: &str = "export function refundCard(amount: number) {\n  return -amount;\n}\n";

/// A symbol only one branch carries is found there, and the symbol that branch
/// dropped stops being an answer.
#[test]
fn a_branch_answers_with_its_own_symbols() {
    let checkout = Checkout::new(ON_MAIN);
    let (_database, store) = store();
    let index = ContextIndex::new(&store);
    index
        .build(checkout.path(), BuildOptions::default())
        .expect("the checkout indexes");
    assert!(
        anchors(&index, &checkout.target(), "chargeCard")
            .contains(&"src/billing.ts#chargeCard".to_owned())
    );

    checkout.git(&["switch", "--quiet", "--create", "refunds"]);
    checkout.write("src/billing.ts", ON_BRANCH);
    checkout.commit("refunds");
    index
        .reconcile(checkout.path(), BuildOptions::default())
        .expect("the checkout reconciles");

    let found = anchors(&index, &checkout.target(), "refundCard");
    assert!(
        found.contains(&"src/billing.ts#refundCard".to_owned()),
        "{found:?}"
    );
    let gone = anchors(&index, &checkout.target(), "chargeCard");
    assert!(
        !gone.contains(&"src/billing.ts#chargeCard".to_owned()),
        "a symbol this branch deleted is still answering: {gone:?}"
    );
}

/// Two checkouts of one project hold two indexes, and neither answers with the
/// other's code.
#[test]
fn two_checkouts_of_one_project_do_not_share_an_index() {
    let main = Checkout::new(ON_MAIN);
    let branch = Checkout::new(ON_BRANCH);
    let (_database, store) = store();
    let index = ContextIndex::new(&store);
    for checkout in [&main, &branch] {
        index
            .build(checkout.path(), BuildOptions::default())
            .expect("the checkout indexes");
    }

    assert!(
        anchors(&index, &main.target(), "chargeCard")
            .contains(&"src/billing.ts#chargeCard".to_owned())
    );
    let crossed = anchors(&index, &main.target(), "refundCard");
    assert!(
        !crossed.contains(&"src/billing.ts#refundCard".to_owned()),
        "one checkout answered with another's symbol: {crossed:?}"
    );
    assert!(
        anchors(&index, &branch.target(), "refundCard")
            .contains(&"src/billing.ts#refundCard".to_owned())
    );
}

/// A task worktree ranks against its own files while its learned routes stay
/// with the origin it was cut from.
#[test]
fn a_worktree_ranks_its_own_files_and_shares_the_origin_routes() {
    let origin = Checkout::new(ON_MAIN);
    let worktree = Checkout::new(ON_BRANCH);
    let (_database, store) = store();
    let index = ContextIndex::new(&store);
    index
        .build(origin.path(), BuildOptions::default())
        .expect("the origin indexes");
    index
        .build(worktree.path(), BuildOptions::default())
        .expect("the worktree indexes");

    let task = Task {
        id: "task-checkout".into(),
        profile_id: "profile-checkout".into(),
        model: "model-checkout".into(),
        cwd: worktree.path().display().to_string(),
        scope: everything(),
        worktree: Some(TaskWorktree {
            origin_cwd: origin.path().display().to_string(),
            path: worktree.path().display().to_string(),
            branch: "refunds".into(),
            links: None,
        }),
        ..Task::default()
    };
    index
        .learn_routes(
            &task,
            &[LearnRouteProposal {
                hints: vec!["money back".into()],
                path: "src/billing.ts".into(),
                symbol: Some("refundCard".into()),
            }],
        )
        .expect("the route saves");

    let target = ContextTarget::worktree(worktree.path(), origin.path(), everything());
    let found = anchors(&index, &target, "money back");
    assert_eq!(
        found.first().map(String::as_str),
        Some("src/billing.ts#refundCard"),
        "{found:?}"
    );

    // The same route reaches the origin's index, which has no such symbol, so
    // it answers with nothing rather than with the worktree's location.
    let crossed = anchors(&index, &origin.target(), "money back");
    assert!(
        !crossed.contains(&"src/billing.ts#refundCard".to_owned()),
        "a worktree's route leaked into the origin's answer: {crossed:?}"
    );
}

/// A worktree's own branch decides the answer, both ways: what that branch
/// added is findable, and what it deleted stops being offered — even though
/// the origin it was cut from still has it.
#[test]
fn a_worktree_answers_from_its_branch_not_the_origin() {
    let origin = Checkout::new(ON_MAIN);
    let worktree = Checkout::new(ON_BRANCH);
    let (_database, store) = store();
    let index = ContextIndex::new(&store);
    index
        .build(origin.path(), BuildOptions::default())
        .expect("the origin indexes");

    let target = ContextTarget::worktree(worktree.path(), origin.path(), everything());
    let added = anchors(&index, &target, "refundCard");
    assert!(
        added.contains(&"src/billing.ts#refundCard".to_owned()),
        "a branch-only symbol was invisible: {added:?}"
    );
    let deleted = anchors(&index, &target, "chargeCard");
    assert!(
        !deleted.contains(&"src/billing.ts#chargeCard".to_owned()),
        "a symbol this branch deleted answered from the origin: {deleted:?}"
    );
}

/// A route taught on one branch is still there after the checkout visits a
/// branch that does not carry its target.
#[test]
fn a_route_outlives_a_branch_that_dropped_its_target() {
    let checkout = Checkout::new(ON_MAIN);
    checkout.git(&["switch", "--quiet", "--create", "refunds"]);
    checkout.write("src/billing.ts", ON_BRANCH);
    checkout.commit("refunds");
    let (_database, store) = store();
    let index = ContextIndex::new(&store);
    index
        .build(checkout.path(), BuildOptions::default())
        .expect("the checkout indexes");
    index
        .learn_user_route(
            checkout.path(),
            &LearnRouteProposal {
                hints: vec!["money back".into()],
                path: "src/billing.ts".into(),
                symbol: Some("refundCard".into()),
            },
        )
        .expect("the route saves");

    checkout.git(&["switch", "--quiet", "main"]);
    let away = index
        .reconcile(checkout.path(), BuildOptions::default())
        .expect("the checkout reconciles");
    assert_eq!(away.routes_dropped, 1, "{away:?}");
    assert!(
        !anchors(&index, &checkout.target(), "money back")
            .contains(&"src/billing.ts#refundCard".to_owned()),
        "a branch answered with a symbol it does not carry"
    );

    // Staying on that branch drops nothing further, so the count stops
    // reporting a loss that already happened.
    let still_away = index
        .reconcile(checkout.path(), BuildOptions::default())
        .expect("the checkout reconciles");
    assert_eq!(still_away.routes_dropped, 0, "{still_away:?}");

    checkout.git(&["switch", "--quiet", "refunds"]);
    index
        .reconcile(checkout.path(), BuildOptions::default())
        .expect("the checkout reconciles");
    let found = anchors(&index, &checkout.target(), "money back");
    assert_eq!(
        found.first().map(String::as_str),
        Some("src/billing.ts#refundCard"),
        "the route did not survive the trip to another branch: {found:?}"
    );
}

/// An index left behind by an older layout is rebuilt, not merged into. A
/// merge would write today's rows beside yesterday's and then stamp the whole
/// thing as current.
#[test]
fn a_stale_index_layout_is_rebuilt_rather_than_merged() {
    let checkout = Checkout::new(ON_MAIN);
    let (_database, store) = store();
    let index = ContextIndex::new(&store);
    index
        .build(checkout.path(), BuildOptions::default())
        .expect("the checkout indexes");

    store
        .transaction(|transaction| {
            transaction.execute(
                "UPDATE context_index SET scheme=scheme-1 WHERE cwd=?",
                [checkout.path().display().to_string()],
            )?;
            Ok(())
        })
        .expect("the stored layout ages");

    let reconciled = index
        .reconcile(checkout.path(), BuildOptions::default())
        .expect("the checkout reconciles");
    assert_eq!(
        reconciled.refreshed, reconciled.file_count,
        "an older layout was merged into rather than rebuilt: {reconciled:?}"
    );
}

/// Code that was written but not committed is code someone can ask about.
#[test]
fn an_uncommitted_addition_is_answerable() {
    let checkout = Checkout::new(ON_MAIN);
    let (_database, store) = store();
    let index = ContextIndex::new(&store);
    index
        .build(checkout.path(), BuildOptions::default())
        .expect("the checkout indexes");

    checkout.write("src/dunning.ts", "export function retryDunning() {}\n");
    index
        .reconcile(checkout.path(), BuildOptions::default())
        .expect("the checkout reconciles");

    let found = anchors(&index, &checkout.target(), "retryDunning");
    assert!(
        found.contains(&"src/dunning.ts#retryDunning".to_owned()),
        "{found:?}"
    );
}

/// A restore that hands a file back the size and modification time it had is
/// still a different file, and the change time is what says so.
#[test]
#[cfg(unix)]
fn a_restored_file_of_the_same_size_is_re_read() {
    use std::os::unix::fs::MetadataExt;

    let checkout = Checkout::new(ON_MAIN);
    let (_database, store) = store();
    let index = ContextIndex::new(&store);
    index
        .build(checkout.path(), BuildOptions::default())
        .expect("the checkout indexes");

    let file = checkout.path().join("src/billing.ts");
    let stamp = checkout.path().join("billing.stamp");
    copy_preserving(&file, &stamp);
    let before = fs::metadata(&file).expect("the file is there");
    let swapped = ON_MAIN.replace("chargeCard", "chargeCarx");
    assert_eq!(
        swapped.len(),
        ON_MAIN.len(),
        "the swap has to keep the size"
    );
    fs::write(&file, &swapped).expect("the file rewrites");
    restore_mtime(&file, &stamp);

    let reconciled = index
        .reconcile(checkout.path(), BuildOptions::default())
        .expect("the checkout reconciles");
    assert_eq!(
        reconciled.refreshed, 1,
        "a same-size restore went unnoticed: {reconciled:?}"
    );
    let found = anchors(&index, &checkout.target(), "chargeCarx");
    assert!(
        found.contains(&"src/billing.ts#chargeCarx".to_owned()),
        "{found:?}"
    );

    // The change time is the only stamp that moved, so this is the test that
    // size and modification time alone would have failed.
    let after = fs::metadata(&file).expect("the file is there");
    assert_eq!(after.len(), before.len());
    assert_ne!(
        (after.ctime(), after.ctime_nsec()),
        (before.ctime(), before.ctime_nsec()),
        "whole seconds are coarser than the index reads, so compare what it reads"
    );
}

/// Keep a copy wearing the stamps the original had, the way a restore does.
#[cfg(unix)]
fn copy_preserving(file: &Path, stamp: &Path) {
    let status = Command::new("cp")
        .arg("-p")
        .arg(file)
        .arg(stamp)
        .status()
        .expect("cp runs");
    assert!(status.success());
}

#[cfg(unix)]
fn restore_mtime(file: &Path, stamp: &Path) {
    let status = Command::new("touch")
        .arg("-r")
        .arg(stamp)
        .arg(file)
        .status()
        .expect("touch runs");
    assert!(status.success());
}

/// Pruning keeps the index of a repository that is still there and drops the
/// ones no lookup reads: a checkout that is gone, and a folder inside a
/// repository that got an index of its own.
#[test]
fn pruning_keeps_live_repositories_and_drops_indexes_no_lookup_reads() {
    let kept = Checkout::new(ON_MAIN);
    let removed = Checkout::new(ON_BRANCH);
    let (_database, store) = store();
    let index = ContextIndex::new(&store);
    for folder in [kept.path(), removed.path(), &kept.path().join("src")] {
        index
            .build(folder, BuildOptions::default())
            .expect("the folder indexes");
    }
    let removed_path = removed.path().to_path_buf();
    drop(removed);

    assert_eq!(index.prune().expect("the index prunes"), 2);

    let folders = store
        .with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT cwd FROM context_index UNION SELECT cwd FROM context_files \
                 UNION SELECT cwd FROM context_symbols",
            )?;
            Ok(statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .expect("indexed folders read");
    assert_eq!(folders, vec![kept.path().display().to_string()]);
    assert!(!removed_path.exists());
    assert_eq!(
        anchors(&index, &kept.target(), "chargeCard")
            .first()
            .map(String::as_str),
        Some("src/billing.ts#chargeCard")
    );
}
