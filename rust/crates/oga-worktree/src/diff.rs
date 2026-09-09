//! Reading a task's real diff out of git.
//!
//! Every command here only reads: no index writes, no checkout, no refs
//! moved. Output is capped on both sides — a per-file line budget and a
//! ceiling on how much of git's stdout is consumed at all — so a checkout
//! holding a generated tree cannot turn one panel into a whole-repo read.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use oga_domain::{Task, TaskDiff, TaskDiffBasis, TaskDiffFile, TaskDiffFileStatus};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::{WorktreeError, repository_root, require_worktree_paths};

/// How many files one read carries before the rest is reported as truncated.
const MAX_FILES: usize = 400;
/// How many diff lines a single file carries before its body is dropped.
const MAX_FILE_LINES: usize = 2_000;
/// How much of git's stdout is consumed before the read stops.
const MAX_PATCH_BYTES: usize = 4 * 1024 * 1024;
/// How large an untracked file can be before its contents are dropped.
const MAX_UNTRACKED_BYTES: u64 = 256 * 1024;
/// How many untracked files one read carries.
const MAX_UNTRACKED_FILES: usize = 200;

/// Reads what a task actually changed in the checkout it ran in.
pub async fn task_diff(task: &Task) -> Result<TaskDiff, WorktreeError> {
    let cwd = PathBuf::from(&task.cwd);
    match task.worktree.as_ref() {
        Some(worktree) => {
            require_worktree_paths(worktree)?;
            let base = branch_base(&cwd, &worktree.branch).await;
            let against = base.clone().unwrap_or_else(|| "HEAD".to_owned());
            read_diff(&cwd, TaskDiffBasis::Branch, &against, base, &[]).await
        }
        None => {
            require_repository(&cwd).await?;
            let pathspecs = scope_pathspecs(&task.scope.write);
            read_diff(&cwd, TaskDiffBasis::WorkingTree, "HEAD", None, &pathspecs).await
        }
    }
}

async fn require_repository(cwd: &Path) -> Result<(), WorktreeError> {
    if repository_root(cwd).await?.is_some() {
        return Ok(());
    }
    Err(WorktreeError::Message(format!(
        "this task did not run in a git repository: {}",
        cwd.display()
    )))
}

/// The commit a task branch was cut from, taken from its first reflog entry.
///
/// A branch's oldest reflog line is the one that created it, and its recorded
/// value is the commit it started at. Reflogs can be pruned or switched off,
/// in which case the caller falls back to `HEAD` and shows only uncommitted
/// work rather than guessing at a base.
async fn branch_base(cwd: &Path, branch: &str) -> Option<String> {
    let (output, _) = git_capped(
        cwd,
        &[
            "reflog".into(),
            "show".into(),
            "--no-abbrev".into(),
            branch.into(),
        ],
        64 * 1024,
    )
    .await
    .ok()?;
    let commit = output.lines().last()?.split_whitespace().next()?;
    let looks_like_a_commit =
        commit.len() == 40 && commit.chars().all(|byte| byte.is_ascii_hexdigit());
    looks_like_a_commit.then(|| commit.to_owned())
}

/// Turns a task's write scope into git pathspecs. An unrestricted scope
/// narrows to nothing extra: the task's own directory is already the limit.
fn scope_pathspecs(rules: &[String]) -> Vec<String> {
    let mut pathspecs = Vec::new();
    for rule in rules {
        let rule = rule.trim().replace('\\', "/");
        let rule = rule.trim_start_matches("./").trim_end_matches('/');
        if rule == "**" {
            return Vec::new();
        }
        let base = rule.strip_suffix("/**").unwrap_or(rule);
        if base.is_empty() || base.contains("..") || base.starts_with('/') || base.contains('*') {
            return Vec::new();
        }
        pathspecs.push(base.to_owned());
    }
    pathspecs
}

async fn read_diff(
    cwd: &Path,
    basis: TaskDiffBasis,
    against: &str,
    base: Option<String>,
    pathspecs: &[String],
) -> Result<TaskDiff, WorktreeError> {
    let mut args: Vec<String> = vec![
        "-c".into(),
        "core.quotePath=false".into(),
        "diff".into(),
        "--patch".into(),
        "--find-renames".into(),
        "--no-color".into(),
        "--no-ext-diff".into(),
        "--relative".into(),
        against.to_owned(),
    ];
    if !pathspecs.is_empty() {
        args.push("--".into());
        args.extend(pathspecs.iter().cloned());
    }
    let (patch, over_cap) = git_capped(cwd, &args, MAX_PATCH_BYTES).await?;

    let mut files = split_patch(&patch);
    let mut truncated = over_cap;
    if files.len() > MAX_FILES {
        files.truncate(MAX_FILES);
        truncated = true;
    }
    let room = MAX_FILES
        .saturating_sub(files.len())
        .min(MAX_UNTRACKED_FILES);
    if room > 0 {
        let untracked = untracked_files(cwd, pathspecs, room).await?;
        truncated = truncated || untracked.len() == room;
        files.extend(untracked);
    }
    Ok(TaskDiff {
        basis,
        base,
        files,
        truncated,
    })
}

/// Splits one `git diff` body into a file per `diff --git` header.
fn split_patch(patch: &str) -> Vec<TaskDiffFile> {
    let mut files = Vec::new();
    let mut chunk: Vec<&str> = Vec::new();
    for line in patch.lines() {
        if line.starts_with("diff --git ") {
            if let Some(file) = patch_file(&chunk) {
                files.push(file);
            }
            chunk.clear();
        }
        chunk.push(line);
    }
    if let Some(file) = patch_file(&chunk) {
        files.push(file);
    }
    files
}

fn patch_file(chunk: &[&str]) -> Option<TaskDiffFile> {
    let header = chunk.first()?.strip_prefix("diff --git ")?;
    let mut new_path = None;
    let mut old_path = None;
    let mut status = TaskDiffFileStatus::Modified;
    let mut added = 0u32;
    let mut removed = 0u32;
    let mut body = Vec::new();
    for line in &chunk[1..] {
        if let Some(path) = line.strip_prefix("+++ ") {
            new_path = header_path(path);
            continue;
        }
        if let Some(path) = line.strip_prefix("--- ") {
            old_path = header_path(path);
            continue;
        }
        if line.starts_with("new file mode ") {
            status = TaskDiffFileStatus::Added;
            continue;
        }
        if line.starts_with("deleted file mode ") {
            status = TaskDiffFileStatus::Deleted;
            continue;
        }
        if let Some(path) = line.strip_prefix("rename from ") {
            status = TaskDiffFileStatus::Renamed;
            old_path = Some(unquote(path));
            continue;
        }
        if let Some(path) = line.strip_prefix("rename to ") {
            status = TaskDiffFileStatus::Renamed;
            new_path = Some(unquote(path));
            continue;
        }
        if line.starts_with("index ")
            || line.starts_with("old mode ")
            || line.starts_with("new mode ")
            || line.starts_with("similarity index ")
        {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
        body.push(*line);
    }
    let path = new_path
        .or_else(|| old_path.clone())
        .or_else(|| pair_path(header))?;
    let too_large = body.len() > MAX_FILE_LINES;
    Some(TaskDiffFile {
        path,
        old_path: old_path.filter(|_| status == TaskDiffFileStatus::Renamed),
        status,
        added,
        removed,
        patch: (!too_large && !body.is_empty()).then(|| body.join("\n")),
        too_large,
    })
}

/// The path off a `--- a/x` or `+++ b/x` line, absent for the `/dev/null` side.
fn header_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw == "/dev/null" {
        return None;
    }
    let unquoted = unquote(raw);
    Some(
        unquoted
            .strip_prefix("a/")
            .or_else(|| unquoted.strip_prefix("b/"))
            .unwrap_or(&unquoted)
            .to_owned(),
    )
}

/// Last resort for a chunk with no usable `---`/`+++`, which is how a binary
/// or mode-only change arrives: the `b/` half of the `diff --git` line.
fn pair_path(header: &str) -> Option<String> {
    let at = header
        .rfind(" b/")
        .or_else(|| header.rfind(" \"b/"))
        .map(|at| at + 1)?;
    header_path(&header[at..])
}

fn unquote(raw: &str) -> String {
    let Some(inner) = raw.strip_prefix('"').and_then(|raw| raw.strip_suffix('"')) else {
        return raw.to_owned();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(escaped) => out.push(escaped),
            None => {}
        }
    }
    out
}

/// Files in the checkout git has never been told about. They carry no diff of
/// their own, so their contents stand in as one added block.
async fn untracked_files(
    cwd: &Path,
    pathspecs: &[String],
    room: usize,
) -> Result<Vec<TaskDiffFile>, WorktreeError> {
    let mut args: Vec<String> = vec![
        "-c".into(),
        "core.quotePath=false".into(),
        "ls-files".into(),
        "--others".into(),
        "--exclude-standard".into(),
        "-z".into(),
    ];
    if !pathspecs.is_empty() {
        args.push("--".into());
        args.extend(pathspecs.iter().cloned());
    }
    let (listing, _) = git_capped(cwd, &args, 1024 * 1024).await?;
    Ok(listing
        .split('\0')
        .filter(|path| !path.is_empty())
        .take(room)
        .map(|path| untracked_file(cwd, path))
        .collect())
}

fn untracked_file(cwd: &Path, path: &str) -> TaskDiffFile {
    let absolute = cwd.join(path);
    let oversized = fs::metadata(&absolute)
        .map(|meta| meta.len() > MAX_UNTRACKED_BYTES)
        .unwrap_or(true);
    let contents = if oversized {
        None
    } else {
        fs::read(&absolute).ok().filter(|bytes| !bytes.contains(&0))
    };
    let lines: Vec<String> = contents
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .lines()
                .map(|line| format!("+{line}"))
                .collect()
        })
        .unwrap_or_default();
    let too_large = oversized || lines.len() > MAX_FILE_LINES;
    TaskDiffFile {
        path: path.to_owned(),
        old_path: None,
        status: TaskDiffFileStatus::Untracked,
        added: lines.len() as u32,
        removed: 0,
        patch: (!too_large && !lines.is_empty())
            .then(|| format!("@@ -0,0 +1,{} @@\n{}", lines.len(), lines.join("\n"))),
        too_large,
    }
}

/// Runs git and reads at most `cap` bytes of its stdout, stopping it once the
/// cap is reached. The flag reports whether output was left behind.
async fn git_capped(
    cwd: &Path,
    args: &[String],
    cap: usize,
) -> Result<(String, bool), WorktreeError> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| WorktreeError::GitStart {
            cwd: cwd.to_path_buf(),
            source,
        })?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut buffer = Vec::new();
    let mut window = vec![0u8; 64 * 1024];
    let mut over_cap = false;
    while let Ok(read) = stdout.read(&mut window).await {
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&window[..read]);
        if buffer.len() >= cap {
            buffer.truncate(cap);
            over_cap = true;
            break;
        }
    }
    if over_cap {
        let _ = child.kill().await;
    } else {
        let _ = child.wait().await;
    }
    Ok((String::from_utf8_lossy(&buffer).into_owned(), over_cap))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_modified_file_carries_its_hunks_and_counts() {
        let patch = "diff --git a/web/app.ts b/web/app.ts\nindex 111..222 100644\n--- a/web/app.ts\n+++ b/web/app.ts\n@@ -1,3 +1,3 @@\n one\n-two\n+three\n four\n";

        let files = split_patch(patch);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "web/app.ts");
        assert_eq!(files[0].status, TaskDiffFileStatus::Modified);
        assert_eq!((files[0].added, files[0].removed), (1, 1));
        assert!(files[0].patch.as_deref().unwrap().contains("+three"));
        assert!(!files[0].too_large);
    }

    #[test]
    fn added_deleted_and_renamed_files_keep_their_status() {
        let patch = concat!(
            "diff --git a/new.ts b/new.ts\nnew file mode 100644\n--- /dev/null\n+++ b/new.ts\n@@ -0,0 +1 @@\n+hello\n",
            "diff --git a/gone.ts b/gone.ts\ndeleted file mode 100644\n--- a/gone.ts\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n",
            "diff --git a/from.ts b/to.ts\nsimilarity index 100%\nrename from from.ts\nrename to to.ts\n",
        );

        let files = split_patch(patch);

        assert_eq!(files.len(), 3);
        assert_eq!(files[0].status, TaskDiffFileStatus::Added);
        assert_eq!(files[0].path, "new.ts");
        assert_eq!(files[1].status, TaskDiffFileStatus::Deleted);
        assert_eq!(files[1].path, "gone.ts");
        assert_eq!(files[2].status, TaskDiffFileStatus::Renamed);
        assert_eq!(files[2].path, "to.ts");
        assert_eq!(files[2].old_path.as_deref(), Some("from.ts"));
    }

    #[test]
    fn a_file_past_the_line_budget_arrives_without_its_body() {
        let mut patch =
            String::from("diff --git a/big.ts b/big.ts\n--- a/big.ts\n+++ b/big.ts\n@@ -1 +1 @@\n");
        for index in 0..MAX_FILE_LINES + 10 {
            patch.push_str(&format!("+line {index}\n"));
        }

        let files = split_patch(&patch);

        assert!(files[0].too_large);
        assert!(files[0].patch.is_none());
        assert_eq!(files[0].added as usize, MAX_FILE_LINES + 10);
    }

    #[test]
    fn a_write_scope_narrows_to_its_directories() {
        assert_eq!(
            scope_pathspecs(&["web/**".into(), "rust/crates".into()]),
            vec!["web".to_owned(), "rust/crates".to_owned()]
        );
        assert!(scope_pathspecs(&["**".into()]).is_empty());
        assert!(scope_pathspecs(&[]).is_empty());
    }

    #[test]
    fn quoted_paths_read_back_verbatim() {
        let patch = "diff --git \"a/we b.ts\" \"b/we b.ts\"\n--- \"a/we b.ts\"\n+++ \"b/we b.ts\"\n@@ -1 +1 @@\n+one\n";

        assert_eq!(split_patch(patch)[0].path, "we b.ts");
    }
}
