use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use oga_domain::{
    BranchOutcome, Task, TaskDiffBasis, TaskDiffFileStatus, TaskScope, TaskState, WorktreeOption,
    WorktreeRequest,
};
use oga_worktree::{
    WorktreeError, WorktreeJoinCode, active_checkout_tasks, branch_exists, create_task_worktree_at,
    joined_worktree_of, recreate_task_worktree, remove_task_branch, remove_task_worktree,
    require_worktree_paths, unsettled_checkout_writers, validate_join_request,
    worktree_has_uncommitted_work, worktree_request,
};
use tempfile::TempDir;

fn git(cwd: &Path, args: &[&str]) {
    // A developer whose git signs commits — through a hardware key or an
    // agent — cannot run these otherwise: the fixture repository is not what
    // is under test, and signing it only couples the suite to a machine.
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

fn repository() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let repo = temp.path().join("project");
    fs::create_dir(&repo).expect("repository directory");
    git(&repo, &["init", "-b", "main"]);
    fs::write(repo.join("tracked.txt"), "one\n").expect("tracked file");
    fs::write(
        repo.join(".gitignore"),
        "node_modules/\nweb/node_modules/\nvendor/\nrust/target/\nswift/.build/\n.env*\n",
    )
    .expect("ignore rules");
    git(&repo, &["add", "tracked.txt", ".gitignore"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "first",
        ],
    );
    (temp, repo)
}

#[tokio::test]
async fn seeds_the_checkout_and_ignores_the_seeded_path_as_work() {
    let (temp, repo) = repository();
    let dependencies = repo.join("node_modules").join("left-pad");
    fs::create_dir_all(&dependencies).expect("dependency directory");
    fs::write(dependencies.join("index.js"), "module.exports = 1\n").expect("dependency file");
    let request = WorktreeRequest {
        link: Some(vec!["node_modules".into()]),
        ..WorktreeRequest::default()
    };

    let created = create_task_worktree_at(
        &temp.path().join("worktrees"),
        &repo,
        "task-id",
        &request,
        Some("It's ready, don’t stop"),
    )
    .await
    .expect("worktree created");

    assert_eq!(created.worktree.branch, "oga/its-ready-dont-stop");
    assert_eq!(created.cwd, temp.path().join("worktrees/task-id"));
    assert!(created.cwd.join("tracked.txt").exists());
    assert_eq!(
        fs::read_to_string(created.cwd.join("node_modules/left-pad/index.js"))
            .expect("seeded dependency file"),
        "module.exports = 1\n"
    );
    assert!(
        !worktree_has_uncommitted_work(&created.worktree)
            .await
            .unwrap()
    );

    fs::write(created.cwd.join("scratch.txt"), "unfinished\n").expect("scratch file");
    assert!(
        worktree_has_uncommitted_work(&created.worktree)
            .await
            .unwrap()
    );
    fs::remove_file(created.cwd.join("scratch.txt")).expect("scratch removal");
    assert!(
        !worktree_has_uncommitted_work(&created.worktree)
            .await
            .unwrap()
    );

    let paths = require_worktree_paths(&created.worktree).expect("worktree pointer");
    assert_eq!(paths.root, created.cwd);
    assert_eq!(paths.git_ref, "refs/heads/oga/its-ready-dont-stop");
    assert_eq!(paths.links, vec![repo.join("node_modules")]);

    remove_task_worktree(&created.worktree)
        .await
        .expect("checkout removed");
    assert!(!created.cwd.exists());
    assert!(repo.join("node_modules/left-pad/index.js").exists());
    assert_eq!(
        remove_task_branch(&created.worktree).await.unwrap(),
        BranchOutcome::Deleted
    );
    assert!(
        !branch_exists(&repo, &created.worktree.branch)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn discovers_ignored_paths_and_skips_nested_duplicates() {
    let (temp, repo) = repository();
    fs::create_dir_all(repo.join("rust/src")).expect("tracked source directory");
    fs::create_dir_all(repo.join("web/src")).expect("tracked source directory");
    fs::write(repo.join("rust/src/lib.rs"), "").expect("tracked source file");
    fs::write(repo.join("web/src/index.ts"), "").expect("tracked source file");
    git(&repo, &["add", "rust/src/lib.rs", "web/src/index.ts"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "sources",
        ],
    );
    for path in [
        "node_modules/package/index.js",
        "web/node_modules/package/index.js",
        "vendor/autoload.php",
        "rust/target/debug/app",
        "swift/.build/debug/app",
    ] {
        let path = repo.join(path);
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("ignored directory");
        fs::write(path, "fixture\n").expect("ignored file");
    }
    fs::write(repo.join(".env.test"), "APP_ENV=testing\n").expect("environment file");

    let created = create_task_worktree_at(
        &temp.path().join("worktrees"),
        &repo,
        "default-links",
        &WorktreeRequest::default(),
        None,
    )
    .await
    .expect("worktree created");

    assert_eq!(
        created.worktree.links,
        Some(vec![
            ".env.test".into(),
            "node_modules".into(),
            "rust/target".into(),
            "swift".into(),
            "vendor".into(),
            "web/node_modules".into(),
        ])
    );
    assert!(created.cwd.join("node_modules/package/index.js").exists());
    assert!(created.cwd.join("rust/target/debug/app").exists());
    assert!(created.cwd.join("swift/.build/debug/app").exists());
    assert!(created.cwd.join(".env.test").exists());
    for seeded in [
        ".env.test",
        "node_modules",
        "rust/target",
        "swift",
        "vendor",
    ] {
        assert!(
            !fs::symlink_metadata(created.cwd.join(seeded))
                .expect("seeded path")
                .file_type()
                .is_symlink(),
            "{seeded} must be the checkout's own, not a symlink"
        );
    }
}

#[tokio::test]
async fn every_seeded_path_resolves_inside_the_checkout() {
    let (temp, repo) = repository();
    let vendor = repo.join("vendor");
    fs::create_dir_all(vendor.join("bin")).expect("binary directory");
    fs::create_dir_all(vendor.join("pestphp/pest/bin")).expect("package directory");
    fs::write(vendor.join("autoload.php"), "<?php\n").expect("entry point");
    fs::write(vendor.join("pestphp/pest/bin/pest"), "#!/usr/bin/env php\n")
        .expect("package binary");
    std::os::unix::fs::symlink("../pestphp/pest/bin/pest", vendor.join("bin/pest"))
        .expect("relative binary link");
    fs::write(repo.join(".env"), "APP_KEY=original\n").expect("environment file");

    let created = create_task_worktree_at(
        &temp.path().join("worktrees"),
        &repo,
        "seeded-task",
        &WorktreeRequest::default(),
        None,
    )
    .await
    .expect("worktree created");

    let checkout_root = fs::canonicalize(&created.cwd).expect("checkout root");
    for seeded in [
        "vendor/autoload.php",
        "vendor/pestphp/pest/bin/pest",
        ".env",
    ] {
        assert!(
            fs::canonicalize(created.cwd.join(seeded))
                .expect("seeded path")
                .starts_with(&checkout_root),
            "{seeded} must resolve inside the checkout"
        );
    }
    assert_eq!(
        fs::read_link(created.cwd.join("vendor/bin/pest")).expect("preserved link"),
        PathBuf::from("../pestphp/pest/bin/pest")
    );
    assert!(
        !worktree_has_uncommitted_work(&created.worktree)
            .await
            .unwrap()
    );

    fs::write(
        created.cwd.join("vendor/autoload.php"),
        "<?php // rewritten\n",
    )
    .expect("rewrite in place");
    fs::write(created.cwd.join(".env"), "APP_KEY=worker\n").expect("rewrite in place");
    assert_eq!(
        fs::read_to_string(vendor.join("autoload.php")).expect("original entry point"),
        "<?php\n"
    );
    assert_eq!(
        fs::read_to_string(repo.join(".env")).expect("original environment file"),
        "APP_KEY=original\n"
    );

    remove_task_worktree(&created.worktree)
        .await
        .expect("checkout removed");
    assert!(vendor.join("pestphp/pest/bin/pest").exists());
    assert!(repo.join(".env").exists());
}

#[tokio::test]
async fn starts_from_a_commit_and_recreates_the_removed_checkout() {
    let (temp, repo) = repository();
    let first = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git is installed")
            .stdout,
    )
    .expect("git output")
    .trim()
    .to_owned();
    fs::write(repo.join("later.txt"), "later\n").expect("later file");
    git(&repo, &["add", "later.txt"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "second",
        ],
    );
    let request = WorktreeRequest {
        from: Some(first.clone()),
        branch: Some("review/from-first".into()),
        ..WorktreeRequest::default()
    };
    let created = create_task_worktree_at(
        &temp.path().join("worktrees"),
        &repo,
        "task-id",
        &request,
        None,
    )
    .await
    .expect("worktree created");
    assert_eq!(head(&created.cwd), first);
    assert!(!created.cwd.join("later.txt").exists());

    remove_task_worktree(&created.worktree)
        .await
        .expect("checkout removed");
    recreate_task_worktree(&created.worktree)
        .await
        .expect("checkout recreated");
    assert_eq!(head(&created.cwd), first);
    assert!(created.cwd.join("tracked.txt").exists());
}

#[tokio::test]
async fn refuses_bad_links_before_creating_a_checkout() {
    let (temp, repo) = repository();
    fs::create_dir(repo.join("node_modules")).expect("untracked directory");
    let root = temp.path().join("worktrees");
    for links in [
        vec!["missing".into()],
        vec!["tracked.txt".into()],
        vec!["../node_modules".into()],
        vec!["node_modules".into(), "node_modules".into()],
    ] {
        let request = WorktreeRequest {
            link: Some(links),
            ..WorktreeRequest::default()
        };
        assert!(
            create_task_worktree_at(&root, &repo, "task-id", &request, None)
                .await
                .is_err()
        );
        assert!(!root.exists());
    }
}

#[tokio::test]
async fn recreating_a_missing_branch_is_refused() {
    let (temp, repo) = repository();
    let request = WorktreeRequest::default();
    let created = create_task_worktree_at(
        &temp.path().join("worktrees"),
        &repo,
        "task-id",
        &request,
        Some("recreate"),
    )
    .await
    .expect("worktree created");
    remove_task_worktree(&created.worktree)
        .await
        .expect("checkout removed");
    assert_eq!(
        remove_task_branch(&created.worktree).await.unwrap(),
        BranchOutcome::Deleted
    );
    let error = recreate_task_worktree(&created.worktree).await.unwrap_err();
    assert!(error.to_string().contains("no longer exists"));
}

#[test]
fn join_and_active_writer_helpers_preserve_task_rules() {
    assert_eq!(worktree_request(None), None);
    assert_eq!(
        worktree_request(Some(&WorktreeOption::Bare(true))),
        Some(WorktreeRequest::default())
    );
    assert!(
        validate_join_request(&WorktreeRequest {
            join: Some("owner".into()),
            branch: Some("other".into()),
            ..WorktreeRequest::default()
        })
        .is_err()
    );

    let worktree = oga_domain::TaskWorktree {
        origin_cwd: "/project".into(),
        path: "/tmp/worktree".into(),
        branch: "task/owner".into(),
        links: None,
    };
    let owner = Task {
        id: "owner".into(),
        state: TaskState::Completed,
        worktree: Some(worktree.clone()),
        ..Task::default()
    };
    let writer = Task {
        id: "writer".into(),
        state: TaskState::Running,
        scope: TaskScope {
            read: vec!["**".into()],
            write: vec!["**".into()],
        },
        worktree: Some(worktree.clone()),
        ..Task::default()
    };
    let tasks = vec![owner, writer];
    assert_eq!(
        active_checkout_tasks(&tasks, Path::new("/tmp/worktree"), None).len(),
        1
    );
    assert_eq!(
        unsettled_checkout_writers(&tasks, Path::new("/tmp/worktree")).len(),
        1
    );
}

#[test]
fn joining_requires_a_live_checkout() {
    let task = Task {
        id: "owner".into(),
        worktree: Some(oga_domain::TaskWorktree {
            origin_cwd: "/project".into(),
            path: "/tmp/missing-oga-worktree".into(),
            branch: "task/owner".into(),
            links: None,
        }),
        ..Task::default()
    };
    let error = joined_worktree_of(&task).unwrap_err();
    assert!(matches!(
        error,
        WorktreeError::Join(error) if error.code == WorktreeJoinCode::NotFound
    ));
    assert_eq!(WorktreeJoinCode::NotFound.as_str(), "worktree_not_found");
}

#[tokio::test]
async fn a_worktree_task_diffs_its_branch_against_where_it_started() {
    let (temp, repo) = repository();
    let created = create_task_worktree_at(
        &temp.path().join("worktrees"),
        &repo,
        "task-id",
        &WorktreeRequest::default(),
        Some("read the checkout"),
    )
    .await
    .expect("worktree created");
    let base = head(&repo);

    fs::write(created.cwd.join("tracked.txt"), "one\ntwo\n").expect("edit tracked file");
    git(&created.cwd, &["add", "tracked.txt"]);
    git(
        &created.cwd,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "committed work",
        ],
    );
    fs::write(created.cwd.join("tracked.txt"), "one\ntwo\nthree\n").expect("uncommitted edit");
    fs::write(created.cwd.join("fresh.txt"), "new\n").expect("untracked file");

    let task = Task {
        id: "task-id".into(),
        cwd: created.cwd.to_string_lossy().into_owned(),
        worktree: Some(created.worktree.clone()),
        ..Task::default()
    };
    let diff = oga_worktree::task_diff(&task).await.expect("task diff");

    assert_eq!(diff.basis, TaskDiffBasis::Branch);
    assert_eq!(diff.base.as_deref(), Some(base.as_str()));
    assert!(!diff.truncated);

    let tracked = diff
        .files
        .iter()
        .find(|file| file.path == "tracked.txt")
        .expect("tracked file in the diff");
    assert_eq!(tracked.status, TaskDiffFileStatus::Modified);
    assert_eq!((tracked.added, tracked.removed), (2, 0));

    let fresh = diff
        .files
        .iter()
        .find(|file| file.path == "fresh.txt")
        .expect("untracked file in the diff");
    assert_eq!(fresh.status, TaskDiffFileStatus::Untracked);
    assert_eq!(fresh.patch.as_deref(), Some("@@ -0,0 +1,1 @@\n+new"));
}

#[tokio::test]
async fn a_task_in_the_callers_checkout_diffs_only_what_it_could_write() {
    let (_temp, repo) = repository();
    fs::create_dir(repo.join("web")).expect("scoped directory");
    fs::write(repo.join("web/inside.txt"), "in\n").expect("scoped file");
    fs::write(repo.join("outside.txt"), "out\n").expect("unscoped file");
    fs::write(repo.join("tracked.txt"), "one\nedited\n").expect("unscoped edit");

    let task = Task {
        id: "task-id".into(),
        cwd: repo.to_string_lossy().into_owned(),
        scope: TaskScope {
            read: vec!["**".into()],
            write: vec!["web/**".into()],
        },
        ..Task::default()
    };
    let diff = oga_worktree::task_diff(&task).await.expect("task diff");

    assert_eq!(diff.basis, TaskDiffBasis::WorkingTree);
    assert_eq!(diff.base, None);
    assert_eq!(
        diff.files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec!["web/inside.txt"]
    );
}

fn head(cwd: &Path) -> String {
    String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git is installed")
            .stdout,
    )
    .expect("git output")
    .trim()
    .to_owned()
}
