//! Git worktree and branch lifecycle.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::{Arc, Mutex, OnceLock};

use oga_domain::{BranchOutcome, Task, TaskState, TaskWorktree, WorktreeOption, WorktreeRequest};
use thiserror::Error;
use tokio::process::Command;
use tokio::sync::Mutex as AsyncMutex;

mod diff;

pub use diff::task_diff;

pub const DEFAULT_BRANCH_SLUG_LENGTH: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedWorktree {
    pub worktree: TaskWorktree,
    pub cwd: PathBuf,
}

/// A checkout whose destination and branch are known before files are copied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedWorktree {
    pub created: CreatedWorktree,
    root: PathBuf,
    base: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeGitPaths {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub git_ref: String,
    pub links: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRemoval {
    pub outcome: BranchOutcome,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeJoinCode {
    NotFound,
    Busy,
}

impl WorktreeJoinCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "worktree_not_found",
            Self::Busy => "worktree_busy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct WorktreeJoinError {
    pub code: WorktreeJoinCode,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum WorktreeError {
    #[error("could not start git in {cwd}: {source}")]
    GitStart {
        cwd: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not {operation} {path}: {source}")]
    FileSystem {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    Join(#[from] WorktreeJoinError),
}

#[derive(Debug)]
struct GitRun {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl GitRun {
    fn succeeded(&self) -> bool {
        self.status.success()
    }
}

type RepositoryLock = Arc<AsyncMutex<()>>;

static REPOSITORY_LOCKS: OnceLock<Mutex<HashMap<PathBuf, RepositoryLock>>> = OnceLock::new();

fn repository_lock(root: &Path) -> RepositoryLock {
    let locks = REPOSITORY_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().expect("repository lock map is not poisoned");
    locks
        .entry(root.to_path_buf())
        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
        .clone()
}

async fn run_git(cwd: &Path, args: &[String]) -> Result<GitRun, WorktreeError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .await
        .map_err(|source| WorktreeError::GitStart {
            cwd: cwd.to_path_buf(),
            source,
        })?;
    Ok(GitRun {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

async fn git_output(cwd: &Path, args: &[String]) -> Result<Option<String>, WorktreeError> {
    let run = run_git(cwd, args).await?;
    if !run.succeeded() {
        return Ok(None);
    }
    let output = run.stdout.trim();
    Ok((!output.is_empty()).then(|| output.to_owned()))
}

fn file_system_error(operation: &'static str, path: &Path, source: io::Error) -> WorktreeError {
    WorktreeError::FileSystem {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, WorktreeError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    env::current_dir()
        .map(|cwd| cwd.join(path))
        .map_err(|source| file_system_error("resolve", path, source))
}

async fn repository_root(cwd: &Path) -> Result<Option<PathBuf>, WorktreeError> {
    let run = run_git(cwd, &["rev-parse".into(), "--show-toplevel".into()]).await?;
    if run.succeeded() {
        let root = run.stdout.trim();
        return Ok((!root.is_empty()).then(|| PathBuf::from(root)));
    }
    let detail = run.stderr.trim();
    if detail.contains("not a git repository") {
        return Ok(None);
    }
    Err(WorktreeError::Message(format!(
        "could not inspect repository {}: {detail}",
        cwd.display()
    )))
}

fn checkout_cwd(checkout: &Path, root: &Path, origin_cwd: &Path) -> Result<PathBuf, WorktreeError> {
    let root =
        fs::canonicalize(root).map_err(|source| file_system_error("resolve", root, source))?;
    let origin = fs::canonicalize(origin_cwd)
        .map_err(|source| file_system_error("resolve", origin_cwd, source))?;
    let offset = origin.strip_prefix(&root).map_err(|_| {
        WorktreeError::Message(format!(
            "worktree cwd is outside the repository: {}",
            origin_cwd.display()
        ))
    })?;
    Ok(checkout.join(offset))
}

fn create_directory(path: &Path) -> Result<(), WorktreeError> {
    fs::create_dir_all(path).map_err(|source| file_system_error("create directory", path, source))
}

#[cfg(unix)]
fn create_symlink(target: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, destination)
}

#[cfg(windows)]
fn create_symlink(target: &Path, destination: &Path) -> io::Result<()> {
    let resolved = destination
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(target);
    if resolved.is_dir() {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

/// Every loader that resolves a path — PHP's `__DIR__`, Node's module
/// resolution, a virtualenv's `sys.prefix` — resolves it through symlinks, so a
/// borrowed directory puts the original repository back in the checkout's own
/// paths. The checkout holds its own entries instead, and `fs::copy` shares the
/// storage behind them wherever the filesystem can clone rather than duplicate.
fn copy_path(source: &Path, destination: &Path) -> Result<(), WorktreeError> {
    let metadata =
        fs::metadata(source).map_err(|error| file_system_error("inspect", source, error))?;
    if metadata.is_dir() {
        return copy_tree(source, destination);
    }
    fs::copy(source, destination)
        .map(|_| ())
        .map_err(|error| file_system_error("copy", source, error))
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), WorktreeError> {
    create_directory(destination)?;
    let entries = fs::read_dir(source).map_err(|error| file_system_error("read", source, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| file_system_error("read", source, error))?;
        let entry_source = entry.path();
        let entry_destination = destination.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|error| file_system_error("inspect", &entry_source, error))?;
        if kind.is_symlink() {
            let target = fs::read_link(&entry_source)
                .map_err(|error| file_system_error("read", &entry_source, error))?;
            create_symlink(&target, &entry_destination)
                .map_err(|error| file_system_error("link", &entry_destination, error))?;
        } else {
            copy_path(&entry_source, &entry_destination)?;
        }
    }
    Ok(())
}

fn setup_checkout(
    checkout: &Path,
    root: &Path,
    origin_cwd: &Path,
    links: &[String],
) -> Result<PathBuf, WorktreeError> {
    let cwd = checkout_cwd(checkout, root, origin_cwd)?;
    create_directory(&cwd)?;
    let origin = absolute_path(origin_cwd)?;
    for link in links {
        let destination = cwd.join(link);
        if let Some(parent) = destination.parent() {
            create_directory(parent)?;
        }
        copy_path(&origin.join(link), &destination)?;
    }
    Ok(cwd)
}

fn normalize_link(raw: &str) -> String {
    let mut link = raw.trim().replace('\\', "/");
    if let Some(stripped) = link.strip_prefix("./") {
        link = stripped.to_owned();
    }
    while link.ends_with('/') {
        link.pop();
    }
    link
}

async fn planned_links(
    origin_cwd: &Path,
    requested: &[String],
) -> Result<Vec<String>, WorktreeError> {
    let mut links: Vec<String> = Vec::with_capacity(requested.len());
    for raw in requested {
        let link = normalize_link(raw);
        if link.is_empty() {
            return Err(WorktreeError::Message(
                "worktree.link contains an empty path".into(),
            ));
        }
        if Path::new(&link).is_absolute() || link.split('/').any(|part| part == "..") {
            return Err(WorktreeError::Message(format!(
                "worktree.link must stay inside cwd: {raw}"
            )));
        }
        let source = origin_cwd.join(&link);
        if !source.exists() {
            return Err(WorktreeError::Message(format!(
                "worktree.link names a path that is not in {}: {raw}",
                origin_cwd.display()
            )));
        }
        let tracked =
            git_output(origin_cwd, &["ls-files".into(), "--".into(), link.clone()]).await?;
        if tracked.is_some() {
            return Err(WorktreeError::Message(format!(
                "worktree.link cannot link a path git tracks: {raw} — the checkout has its own copy already"
            )));
        }
        if links.iter().any(|existing| existing == &link) {
            return Err(WorktreeError::Message(format!(
                "worktree.link names the same path twice: {raw}"
            )));
        }
        if let Some(overlapping) = links.iter().find(|existing| {
            link.starts_with(&format!("{existing}/")) || existing.starts_with(&format!("{link}/"))
        }) {
            return Err(WorktreeError::Message(format!(
                "worktree.link cannot link both {raw} and {overlapping}: one sits inside the other"
            )));
        }
        links.push(link);
    }
    Ok(links)
}

fn is_default_link(path: &str, is_directory: bool) -> bool {
    let components: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    if components.iter().any(|component| {
        matches!(
            *component,
            ".claude"
                | ".agents"
                | ".DS_Store"
                | ".plans"
                | ".malico"
                | "target"
                | "node_modules"
                | "dist"
                | "build"
                | ".venv"
        ) || component.ends_with(".bun-build")
    }) {
        return false;
    }
    !components.is_empty() && is_directory
}

fn deduplicate_default_links(candidates: Vec<String>) -> Vec<String> {
    let mut links = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if links.iter().any(|existing: &String| {
            candidate == *existing || candidate.starts_with(&format!("{existing}/"))
        }) {
            continue;
        }
        links.push(candidate);
    }
    links
}

async fn default_links(origin_cwd: &Path) -> Result<Vec<String>, WorktreeError> {
    let listed = git_output(
        origin_cwd,
        &[
            "ls-files".into(),
            "--others".into(),
            "--ignored".into(),
            "--exclude-standard".into(),
            "--directory".into(),
            "-z".into(),
        ],
    )
    .await?;
    let candidates = listed
        .unwrap_or_default()
        .split('\0')
        .map(|path| path.trim_end_matches('/'))
        .filter(|path| {
            !path.is_empty()
                && *path != ".git"
                && !path.starts_with(".git/")
                && is_default_link(path, origin_cwd.join(path).is_dir())
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    Ok(deduplicate_default_links(candidates))
}

fn default_branch_slug(title: Option<&str>, task_id: &str) -> String {
    let title = title.unwrap_or_default();
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in title.to_lowercase().chars() {
        if character == '\'' || character == '’' {
            continue;
        }
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_dash = false;
        } else if character == '-' {
            if !previous_dash {
                slug.push(character);
            }
            previous_dash = true;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug.truncate(DEFAULT_BRANCH_SLUG_LENGTH);
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        task_id.to_owned()
    } else {
        slug
    }
}

async fn branch_exists_locked(root: &Path, branch: &str) -> Result<bool, WorktreeError> {
    let args = vec![
        "show-ref".into(),
        "--verify".into(),
        "--quiet".into(),
        format!("refs/heads/{branch}"),
    ];
    let run = run_git(root, &args).await?;
    match run.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(WorktreeError::Message(format!(
            "could not inspect branch {branch}: {}",
            run.stderr.trim()
        ))),
    }
}

async fn available_default_branch(
    root: &Path,
    task_id: &str,
    title: Option<&str>,
) -> Result<String, WorktreeError> {
    let base = format!("oga/{}", default_branch_slug(title, task_id));
    if !branch_exists_locked(root, &base).await? {
        return Ok(base);
    }
    let suffix: String = task_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .take(8)
        .collect();
    let suffix = if suffix.is_empty() { "task" } else { &suffix };
    let disambiguated = format!("{base}-{suffix}");
    if !branch_exists_locked(root, &disambiguated).await? {
        return Ok(disambiguated);
    }
    for attempt in 2.. {
        let branch = format!("{disambiguated}-{attempt}");
        if !branch_exists_locked(root, &branch).await? {
            return Ok(branch);
        }
    }
    unreachable!()
}

async fn remove_task_worktree_locked(
    root: Option<&Path>,
    worktree: &TaskWorktree,
    force: bool,
) -> Result<(), WorktreeError> {
    let Some(root) = root else {
        if Path::new(&worktree.path).exists() {
            return Err(WorktreeError::Message(format!(
                "could not verify the checkout's repository: {}",
                worktree.origin_cwd
            )));
        }
        return Ok(());
    };
    let mut remove_args = vec!["worktree".into(), "remove".into()];
    if force {
        remove_args.push("--force".into());
    }
    remove_args.push(worktree.path.clone());
    let removed = run_git(root, &remove_args).await?;
    if !removed.succeeded() && Path::new(&worktree.path).exists() {
        return Err(WorktreeError::Message(format!(
            "could not remove the checkout: {}",
            removed.stderr.trim()
        )));
    }
    let pruned = run_git(root, &["worktree".into(), "prune".into()]).await?;
    if !pruned.succeeded() {
        return Err(WorktreeError::Message(format!(
            "could not prune git worktrees: {}",
            pruned.stderr.trim()
        )));
    }
    Ok(())
}

async fn remove_task_branch_locked(
    root: &Path,
    worktree: &TaskWorktree,
) -> Result<BranchRemoval, WorktreeError> {
    if !branch_exists_locked(root, &worktree.branch).await? {
        return Ok(BranchRemoval {
            outcome: BranchOutcome::AlreadyGone,
            reason: None,
        });
    }
    if let Some(path) = checked_out_worktree_locked(root, &worktree.branch).await? {
        return Ok(BranchRemoval {
            outcome: BranchOutcome::Kept,
            reason: Some(format!("branch is checked out at {path}")),
        });
    }
    let args = vec!["branch".into(), "-d".into(), worktree.branch.clone()];
    let run = run_git(root, &args).await?;
    if run.succeeded() {
        return Ok(BranchRemoval {
            outcome: BranchOutcome::Deleted,
            reason: None,
        });
    }
    let detail = run
        .stderr
        .trim()
        .strip_prefix("error: ")
        .unwrap_or(run.stderr.trim());
    let reason = if detail.to_ascii_lowercase().contains("not fully merged") {
        "branch has unmerged commits".into()
    } else if detail.is_empty() {
        "git refused to delete the branch".into()
    } else {
        format!("git refused to delete the branch: {detail}")
    };
    Ok(BranchRemoval {
        outcome: BranchOutcome::Kept,
        reason: Some(reason),
    })
}

async fn checked_out_worktree_locked(
    root: &Path,
    branch: &str,
) -> Result<Option<String>, WorktreeError> {
    let run = run_git(
        root,
        &["worktree".into(), "list".into(), "--porcelain".into()],
    )
    .await?;
    if !run.succeeded() {
        return Err(WorktreeError::Message(format!(
            "could not list git worktrees: {}",
            run.stderr.trim()
        )));
    }
    let reference = format!("refs/heads/{branch}");
    let mut path = None;
    for line in run.stdout.lines() {
        if let Some(value) = line.strip_prefix("worktree ") {
            path = Some(value.to_owned());
        } else if line
            .strip_prefix("branch ")
            .is_some_and(|value| value == reference)
        {
            return Ok(path);
        } else if line.is_empty() {
            path = None;
        }
    }
    Ok(None)
}

pub fn worktrees_root() -> PathBuf {
    match env::var_os("OGA_DB").filter(|value| !value.is_empty()) {
        Some(database) => worktrees_root_from_database(Path::new(&database)),
        None => env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".oga")
            .join("worktrees"),
    }
}

pub fn worktrees_root_from_database(database: &Path) -> PathBuf {
    let database = absolute_path(database).expect("current directory is available");
    database
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("worktrees")
}

pub fn project_cwd(task: &Task) -> PathBuf {
    task.worktree
        .as_ref()
        .map(|worktree| PathBuf::from(&worktree.origin_cwd))
        .unwrap_or_else(|| PathBuf::from(&task.cwd))
}

pub async fn current_branch(cwd: &Path) -> Result<Option<String>, WorktreeError> {
    git_output(cwd, &["branch".into(), "--show-current".into()]).await
}

pub fn worktree_label(project_path: &Path, worktree_path: &Path) -> String {
    let project = project_path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let worktree = worktree_path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    format!(
        "{}-{}",
        project,
        worktree.split('-').next().unwrap_or_default()
    )
}

pub fn worktree_request(option: Option<&WorktreeOption>) -> Option<WorktreeRequest> {
    match option {
        None | Some(WorktreeOption::Bare(false)) => None,
        Some(WorktreeOption::Bare(true)) => Some(WorktreeRequest::default()),
        Some(WorktreeOption::Request(request)) => Some(request.clone()),
    }
}

pub fn worktree_active(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Queued
            | TaskState::PreparingCheckout
            | TaskState::RemovingCheckout
            | TaskState::Pending
            | TaskState::Running
            | TaskState::NeedsInput
            | TaskState::Answered
    )
}

pub fn validate_join_request(request: &WorktreeRequest) -> Result<(), WorktreeError> {
    if request.join.is_some()
        && (request.branch.is_some()
            || request.from.is_some()
            || request.link.as_ref().is_some_and(|links| !links.is_empty()))
    {
        return Err(WorktreeError::Message(
            "worktree.join cannot be combined with worktree.branch, worktree.from or worktree.link — the checkout being joined already decided all three".into(),
        ));
    }
    Ok(())
}

pub fn joined_worktree_of(task: &Task) -> Result<TaskWorktree, WorktreeError> {
    let Some(worktree) = task.worktree.as_ref() else {
        return Err(WorktreeJoinError {
            code: WorktreeJoinCode::NotFound,
            message: format!(
                "no checkout to join on task {} — it never had one, or its checkout is already removed",
                task.id
            ),
        }
        .into());
    };
    if !Path::new(&worktree.path).join(".git").exists() {
        return Err(WorktreeJoinError {
            code: WorktreeJoinCode::NotFound,
            message: format!(
                "no checkout to join on task {} — it never had one, or its checkout is already removed",
                task.id
            ),
        }
        .into());
    }
    Ok(worktree.clone())
}

pub async fn create_task_worktree(
    origin_cwd: &Path,
    task_id: &str,
    request: &WorktreeRequest,
    title: Option<&str>,
) -> Result<CreatedWorktree, WorktreeError> {
    create_task_worktree_at(&worktrees_root(), origin_cwd, task_id, request, title).await
}

pub async fn create_task_worktree_at(
    root_dir: &Path,
    origin_cwd: &Path,
    task_id: &str,
    request: &WorktreeRequest,
    title: Option<&str>,
) -> Result<CreatedWorktree, WorktreeError> {
    let planned = plan_task_worktree_at(root_dir, origin_cwd, task_id, request, title).await?;
    prepare_task_worktree(&planned).await?;
    Ok(planned.created)
}

pub async fn plan_task_worktree(
    origin_cwd: &Path,
    task_id: &str,
    request: &WorktreeRequest,
    title: Option<&str>,
) -> Result<PlannedWorktree, WorktreeError> {
    plan_task_worktree_at(&worktrees_root(), origin_cwd, task_id, request, title).await
}

pub async fn plan_task_worktree_at(
    root_dir: &Path,
    origin_cwd: &Path,
    task_id: &str,
    request: &WorktreeRequest,
    title: Option<&str>,
) -> Result<PlannedWorktree, WorktreeError> {
    let root_dir = absolute_path(root_dir)?;
    if request.join.is_some() {
        return Err(WorktreeError::Message(
            "worktree.join must enter an existing checkout".into(),
        ));
    }
    let root = repository_root(origin_cwd).await?.ok_or_else(|| {
        WorktreeError::Message(format!(
            "worktree needs a git repository: {} is not inside one",
            origin_cwd.display()
        ))
    })?;
    let from = request.from.as_deref().unwrap_or("HEAD");
    let base_ref = format!("{from}^{{commit}}");
    let base = git_output(
        &root,
        &[
            "rev-parse".into(),
            "--verify".into(),
            "--quiet".into(),
            base_ref,
        ],
    )
    .await?;
    let base = base.ok_or_else(|| {
        WorktreeError::Message(if request.from.is_some() {
            format!(
                "worktree base is not a commit in this repository: {from} — name a branch, tag or commit {} already has",
                root.display()
            )
        } else {
            format!(
                "worktree needs a commit to branch from: {} has no commits yet — commit once, then delegate",
                root.display()
            )
        })
    })?;
    let requested_links = match request.link.as_deref() {
        Some(links) => links.to_vec(),
        None => default_links(origin_cwd).await?,
    };
    let links = planned_links(origin_cwd, &requested_links).await?;
    let checkout = root_dir.join(task_id);
    let branch = match request.branch.as_deref() {
        Some(branch) => branch.to_owned(),
        None => available_default_branch(&root, task_id, title).await?,
    };
    let branch_check = run_git(
        &root,
        &["check-ref-format".into(), "--branch".into(), branch.clone()],
    )
    .await?;
    if !branch_check.succeeded() {
        return Err(WorktreeError::Message(format!(
            "invalid worktree branch: {branch}"
        )));
    }
    let worktree = TaskWorktree {
        origin_cwd: origin_cwd.to_string_lossy().into_owned(),
        path: checkout.to_string_lossy().into_owned(),
        branch,
        links: (!links.is_empty()).then_some(links),
    };
    Ok(PlannedWorktree {
        created: CreatedWorktree {
            cwd: checkout_cwd(&checkout, &root, origin_cwd)?,
            worktree,
        },
        root,
        base,
    })
}

/// Create a planned checkout. The repository lock belongs only to this
/// detached operation, including its potentially slow file copy.
pub async fn prepare_task_worktree(planned: &PlannedWorktree) -> Result<(), WorktreeError> {
    let lock = repository_lock(&planned.root);
    let _guard = lock.lock().await;
    if branch_exists_locked(&planned.root, &planned.created.worktree.branch).await? {
        return Err(WorktreeError::Message(format!(
            "branch already exists: {}",
            planned.created.worktree.branch
        )));
    }
    let checkout = PathBuf::from(&planned.created.worktree.path);
    let parent = checkout
        .parent()
        .ok_or_else(|| WorktreeError::Message("worktree path has no parent".into()))?;
    create_directory(parent)?;
    let add_args = vec![
        "worktree".into(),
        "add".into(),
        "-b".into(),
        planned.created.worktree.branch.clone(),
        planned.created.worktree.path.clone(),
        planned.base.clone(),
    ];
    let added = run_git(&planned.root, &add_args).await?;
    if !added.succeeded() {
        return Err(WorktreeError::Message(format!(
            "could not create a worktree for this task: {}",
            added.stderr.trim()
        )));
    }
    let root = planned.root.clone();
    let origin = PathBuf::from(&planned.created.worktree.origin_cwd);
    let links = planned.created.worktree.links.clone().unwrap_or_default();
    let copied =
        tokio::task::spawn_blocking(move || setup_checkout(&checkout, &root, &origin, &links))
            .await
            .map_err(|error| WorktreeError::Message(format!("checkout copy stopped: {error}")))?;
    match copied {
        Ok(_) => Ok(()),
        Err(error) => {
            let _ =
                remove_task_worktree_locked(Some(&planned.root), &planned.created.worktree, true)
                    .await;
            let _ = remove_task_branch_locked(&planned.root, &planned.created.worktree).await;
            Err(error)
        }
    }
}

pub async fn worktree_has_uncommitted_work(worktree: &TaskWorktree) -> Result<bool, WorktreeError> {
    let run = run_git(
        Path::new(&worktree.path),
        &[
            "status".into(),
            "--porcelain".into(),
            "--ignored".into(),
            "-z".into(),
        ],
    )
    .await?;
    if !run.succeeded() {
        return Err(WorktreeError::Message(format!(
            "could not inspect worktree status: {}",
            run.stderr.trim()
        )));
    }
    if run.stdout.is_empty() {
        return Ok(false);
    }
    let links = worktree_linked_status_paths(worktree).await?;
    for record in run.stdout.split('\0').filter(|record| !record.is_empty()) {
        if (record.starts_with("?? ") || record.starts_with("!! "))
            && links.contains(record[3..].trim_end_matches('/'))
        {
            continue;
        }
        return Ok(true);
    }
    Ok(false)
}

async fn worktree_linked_status_paths(
    worktree: &TaskWorktree,
) -> Result<HashSet<String>, WorktreeError> {
    let mut links = HashSet::new();
    let root = repository_root(Path::new(&worktree.origin_cwd)).await?;
    let offset = root
        .and_then(|root| {
            let root = fs::canonicalize(root).ok()?;
            let origin = fs::canonicalize(&worktree.origin_cwd).ok()?;
            Some(
                origin
                    .strip_prefix(root)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/"),
            )
        })
        .filter(|offset| !offset.is_empty());
    for link in worktree.links.as_deref().unwrap_or_default() {
        let link = link.trim_end_matches('/').replace('\\', "/");
        links.insert(match offset.as_deref() {
            Some(offset) => format!("{offset}/{link}"),
            None => link,
        });
    }
    Ok(links)
}

pub async fn remove_task_worktree(worktree: &TaskWorktree) -> Result<(), WorktreeError> {
    let root = repository_root(Path::new(&worktree.origin_cwd)).await?;
    let lock = root.as_deref().map(repository_lock);
    if let Some(lock) = lock {
        let _guard = lock.lock().await;
        remove_task_worktree_locked(root.as_deref(), worktree, false).await
    } else {
        remove_task_worktree_locked(None, worktree, false).await
    }
}

pub async fn remove_task_branch(worktree: &TaskWorktree) -> Result<BranchOutcome, WorktreeError> {
    Ok(remove_task_branch_safely(worktree).await?.outcome)
}

pub async fn remove_task_branch_safely(
    worktree: &TaskWorktree,
) -> Result<BranchRemoval, WorktreeError> {
    let Some(root) = repository_root(Path::new(&worktree.origin_cwd)).await? else {
        return Ok(BranchRemoval {
            outcome: BranchOutcome::Kept,
            reason: Some("could not verify the task's repository".into()),
        });
    };
    let lock = repository_lock(&root);
    let _guard = lock.lock().await;
    remove_task_branch_locked(&root, worktree).await
}

pub async fn branch_exists(root: &Path, branch: &str) -> Result<bool, WorktreeError> {
    branch_exists_locked(root, branch).await
}

pub async fn branch_recreatable(worktree: &TaskWorktree) -> Result<bool, WorktreeError> {
    let Some(root) = repository_root(Path::new(&worktree.origin_cwd)).await? else {
        return Ok(false);
    };
    let lock = repository_lock(&root);
    let _guard = lock.lock().await;
    if !branch_exists_locked(&root, &worktree.branch).await? {
        return Ok(false);
    }
    Ok(checked_out_worktree_locked(&root, &worktree.branch)
        .await?
        .is_none_or(|path| Path::new(&path) == Path::new(&worktree.path)))
}

pub fn require_task_worktree(task: &Task) -> Result<Option<WorktreeGitPaths>, WorktreeError> {
    let Some(worktree) = task.worktree.as_ref() else {
        return Ok(None);
    };
    require_worktree_paths(worktree).map(Some)
}

pub fn require_worktree_paths(worktree: &TaskWorktree) -> Result<WorktreeGitPaths, WorktreeError> {
    let marker = Path::new(&worktree.path).join(".git");
    if !marker.exists() {
        return Err(WorktreeError::Message(format!(
            "the git worktree for this task is gone: {} — its work is on branch {} in {}; delegate a fresh task instead",
            worktree.path, worktree.branch, worktree.origin_cwd
        )));
    }
    let marker_contents =
        fs::read_to_string(&marker).map_err(|source| file_system_error("read", &marker, source))?;
    let pointer = marker_contents
        .lines()
        .find_map(|line| {
            line.strip_prefix("gitdir:")
                .map(str::trim)
                .filter(|line| !line.is_empty())
        })
        .ok_or_else(|| {
            WorktreeError::Message(format!(
                "{} is no longer a git worktree pointer",
                marker.display()
            ))
        })?;
    let git_dir = Path::new(&worktree.path).join(pointer);
    let common_file = git_dir.join("commondir");
    let common_dir = if common_file.exists() {
        let common = fs::read_to_string(&common_file)
            .map_err(|source| file_system_error("read", &common_file, source))?;
        git_dir.join(common.trim())
    } else {
        git_dir
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                WorktreeError::Message("git worktree pointer has no common directory".into())
            })?
    };
    let origin = absolute_path(Path::new(&worktree.origin_cwd))?;
    let links = worktree
        .links
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|link| origin.join(link))
        .collect();
    Ok(WorktreeGitPaths {
        root: PathBuf::from(&worktree.path),
        git_dir,
        common_dir,
        git_ref: format!("refs/heads/{}", worktree.branch),
        links,
    })
}

pub async fn recreate_task_worktree(worktree: &TaskWorktree) -> Result<(), WorktreeError> {
    let origin = Path::new(&worktree.origin_cwd);
    let Some(root) = repository_root(origin).await? else {
        return Err(WorktreeError::Message(format!(
            "cannot recreate checkout: {} is not a git repository",
            worktree.origin_cwd
        )));
    };
    if Path::new(&worktree.path).join(".git").exists() {
        return Ok(());
    }
    let lock = repository_lock(&root);
    let _guard = lock.lock().await;
    if !branch_exists_locked(&root, &worktree.branch).await? {
        return Err(WorktreeError::Message(format!(
            "branch {} no longer exists in {} — cannot recreate checkout for this task",
            worktree.branch, worktree.origin_cwd
        )));
    }
    if let Some(parent) = Path::new(&worktree.path).parent() {
        create_directory(parent)?;
    }
    let add_args = vec![
        "worktree".into(),
        "add".into(),
        worktree.path.clone(),
        worktree.branch.clone(),
    ];
    let added = run_git(&root, &add_args).await?;
    if !added.succeeded() {
        return Err(WorktreeError::Message(format!(
            "could not recreate checkout on branch {}: {}",
            worktree.branch,
            added.stderr.trim()
        )));
    }
    let links: Vec<String> = worktree.links.as_deref().unwrap_or_default().to_vec();
    if let Err(error) = setup_checkout(Path::new(&worktree.path), &root, origin, &links) {
        let _ = remove_task_worktree_locked(Some(&root), worktree, true).await;
        return Err(error);
    }
    Ok(())
}

pub fn active_checkout_tasks<'a>(
    tasks: &'a [Task],
    checkout: &Path,
    exclude_task_id: Option<&str>,
) -> Vec<&'a Task> {
    tasks
        .iter()
        .filter(|task| {
            exclude_task_id != Some(task.id.as_str())
                && worktree_active(task.state)
                && task
                    .worktree
                    .as_ref()
                    .is_some_and(|worktree| Path::new(&worktree.path) == checkout)
        })
        .collect()
}

pub fn unsettled_checkout_writers<'a>(tasks: &'a [Task], checkout: &Path) -> Vec<&'a Task> {
    active_checkout_tasks(tasks, checkout, None)
        .into_iter()
        .filter(|task| !task.scope.write.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{deduplicate_default_links, is_default_link};

    #[test]
    fn default_links_skip_nested_paths() {
        assert_eq!(
            deduplicate_default_links(
                vec!["swift".into(), "swift/.build".into(), "vendor".into(),]
            ),
            vec!["swift", "vendor"]
        );
    }

    #[test]
    fn default_link_selection_accepts_supported_paths_only() {
        assert!(!is_default_link("web/node_modules", true));
        assert!(!is_default_link("rust/target", true));
        assert!(!is_default_link("output/dist", true));
        assert!(!is_default_link("tools/.venv", true));
        assert!(!is_default_link("web/.env.test", false));
        assert!(is_default_link("generated-out", true));
        assert!(!is_default_link(".claude/node_modules", true));
        assert!(!is_default_link("cache.bun-build", true));
        assert!(!is_default_link(".env.backup.txt", false));
    }
}
