//! Which files the index reads. A file is a candidate when an adapter owns
//! its extension, git is not ignoring it, and it is not a lockfile.

use std::collections::HashMap;
use std::fs::{self, DirEntry, Metadata};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use rayon::prelude::*;

use crate::lang;

#[derive(Debug, Clone)]
pub struct WalkFile {
    pub path: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub ctime_ms: i64,
}

#[derive(Debug, Default)]
pub struct WalkResult {
    pub files: Vec<WalkFile>,
    /// True when the walk stopped at `max_files` before seeing the whole tree.
    pub partial: bool,
}

const EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    "target",
    ".build",
    ".git",
    ".next",
    ".venv",
    "vendor",
];

const LOCKFILES: &[&str] = &[
    "package-lock.json",
    "bun.lock",
    "bun.lockb",
    "pnpm-lock.yaml",
    "yarn.lock",
    "Cargo.lock",
    "Podfile.lock",
    "Gemfile.lock",
    "poetry.lock",
    "uv.lock",
];

#[derive(Debug, Clone)]
struct IgnoreRule {
    negated: bool,
    dir_only: bool,
    regex: regex::Regex,
}

#[derive(Debug, Clone)]
struct IgnoreGroup {
    base: String,
    rules: Vec<IgnoreRule>,
}

/// Directories are read in parallel. The files kept are the first
/// `max_files` in the order a one-thread walk meets them, so a partial walk
/// keeps the same files every time.
pub fn walk_files(cwd: &Path, max_files: usize) -> WalkResult {
    let groups = repository_ignore_groups(cwd);
    let ignores = groups.iter().collect::<Vec<_>>();
    let walk = Walk {
        cwd,
        found: AtomicUsize::new(0),
        give_up_after: max_files.saturating_mul(GIVE_UP_FACTOR),
    };
    let mut files = walk.directory(cwd, &ignores);
    let partial = files.len() > max_files;
    files.truncate(max_files);
    files.sort_by(|left, right| left.path.cmp(&right.path));
    WalkResult { files, partial }
}

/// A tree this many times past the file cap stops being read. Below it the
/// whole tree is read, so which files a partial walk keeps never depends on
/// which thread finished first.
const GIVE_UP_FACTOR: usize = 4;

struct Walk<'a> {
    cwd: &'a Path,
    found: AtomicUsize,
    give_up_after: usize,
}

fn repository_ignore_groups(cwd: &Path) -> Vec<IgnoreGroup> {
    let mut groups = Vec::new();
    if let Some(path) = global_ignore_path(cwd) {
        add_ignore_group(&path, &mut groups);
    }
    if let Some(path) = git_info_exclude_path(cwd) {
        add_ignore_group(&path, &mut groups);
    }
    groups
}

fn add_ignore_group(path: &Path, groups: &mut Vec<IgnoreGroup>) {
    if let Ok(source) = fs::read_to_string(path) {
        groups.push(IgnoreGroup {
            base: String::new(),
            rules: parse_ignore_rules(&source),
        });
    }
}

/// Asking git spawns a process, so each project asks once per process.
fn global_ignore_path(cwd: &Path) -> Option<PathBuf> {
    static RESOLVED: OnceLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> = OnceLock::new();
    RESOLVED
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(cwd.to_path_buf())
        .or_insert_with(|| excludes_file(cwd))
        .clone()
}

fn excludes_file(cwd: &Path) -> Option<PathBuf> {
    if let Ok(output) = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["config", "--path", "--get", "core.excludesFile"])
        .output()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config_home.join("git/ignore"))
}

fn git_info_exclude_path(cwd: &Path) -> Option<PathBuf> {
    Some(git_dir(cwd)?.join("info/exclude"))
}

/// When git last rewrote this checkout's index. A switch, reset, commit, or
/// stash each rewrites it, so an unchanged stamp means git moved nothing.
pub fn checkout_stamp(cwd: &Path) -> Option<SystemTime> {
    fs::metadata(git_dir(cwd)?.join("index"))
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn git_dir(cwd: &Path) -> Option<PathBuf> {
    let git_path = cwd.join(".git");
    if git_path.is_dir() {
        return Some(git_path);
    }
    let gitfile = fs::read_to_string(git_path).ok()?;
    let path = PathBuf::from(gitfile.strip_prefix("gitdir: ")?.trim());
    Some(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

/// True when some adapter would parse this path.
pub fn is_indexable(path: &str) -> bool {
    lang::adapter_for(path).is_some()
}

impl Walk<'_> {
    /// This directory's files, then each subdirectory's, in name order.
    fn directory(&self, directory: &Path, inherited: &[&IgnoreGroup]) -> Vec<WalkFile> {
        if self.found.load(Ordering::Relaxed) > self.give_up_after {
            return Vec::new();
        }
        let Ok(entries) = read_sorted(directory) else {
            return Vec::new();
        };
        let own = fs::read_to_string(directory.join(".gitignore"))
            .ok()
            .map(|source| IgnoreGroup {
                base: relative_path(self.cwd, directory),
                rules: parse_ignore_rules(&source),
            });
        let mut ignores = inherited.to_vec();
        ignores.extend(own.as_ref());

        let mut files = Vec::new();
        let mut directories = Vec::new();
        for entry in entries {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let relative = relative_path(self.cwd, &path);
            if metadata.is_dir() {
                if !EXCLUDED_DIRS.contains(&name.as_ref()) && !is_ignored(&ignores, &relative, true)
                {
                    directories.push(path);
                }
                continue;
            }
            if !metadata.is_file()
                || !is_indexable(&relative)
                || LOCKFILES.contains(&name.as_ref())
                || is_ignored(&ignores, &relative, false)
            {
                continue;
            }
            files.push(WalkFile {
                path: relative,
                size: metadata.len(),
                mtime_ms: mtime_ms(&metadata),
                ctime_ms: ctime_ms(&metadata),
            });
        }
        self.found.fetch_add(files.len(), Ordering::Relaxed);

        let nested = directories
            .par_iter()
            .map(|directory| self.directory(directory, &ignores))
            .collect::<Vec<_>>();
        files.extend(nested.into_iter().flatten());
        files
    }
}

fn read_sorted(directory: &Path) -> std::io::Result<Vec<DirEntry>> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn relative_path(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn parse_ignore_rules(source: &str) -> Vec<IgnoreRule> {
    source
        .lines()
        .filter_map(|raw| {
            let line = raw.trim_end();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let negated = line.starts_with('!');
            let mut pattern = if negated { &line[1..] } else { line };
            let dir_only = pattern.ends_with('/');
            if dir_only {
                pattern = pattern.trim_end_matches('/');
            }
            let anchored = pattern.starts_with('/') || pattern.contains('/');
            pattern = pattern.trim_start_matches('/');
            if pattern.is_empty() {
                return None;
            }
            Some(IgnoreRule {
                negated,
                dir_only,
                regex: ignore_regex(pattern, anchored)?,
            })
        })
        .collect()
}

fn ignore_regex(pattern: &str, anchored: bool) -> Option<regex::Regex> {
    let mut expression = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(char) = chars.next() {
        match char {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    chars.next();
                }
                expression.push_str("(?:.*/)?");
            }
            '*' => expression.push_str("[^/]*"),
            '?' => expression.push_str("[^/]"),
            '[' => match bracket_class(&mut chars) {
                Some(class) => expression.push_str(&class),
                None => expression.push_str(r"\["),
            },
            other => expression.push_str(&regex::escape(&other.to_string())),
        }
    }
    let prefix = if anchored { "^" } else { "^(?:.*/)?" };
    regex::Regex::new(&format!("{prefix}{expression}$")).ok()
}

/// A `[...]` class, read just past its `[`, as a regex class that never
/// matches `/`: `[cod]`, `[a-z]`, and `[!0-9]` or `[^0-9]` for the negation.
/// `None` when the class is never closed, and nothing is consumed.
fn bracket_class(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    let mut ahead = chars.clone();
    let negated = ahead.next_if(|char| matches!(char, '!' | '^')).is_some();
    let mut members = String::new();
    loop {
        match ahead.next()? {
            ']' if !members.is_empty() => break,
            char @ ('\\' | '[' | ']' | '^' | '&' | '~') => {
                members.push('\\');
                members.push(char);
            }
            char => members.push(char),
        }
    }
    *chars = ahead;
    Some(if negated {
        format!("[^/{members}]")
    } else {
        format!("[{members}]")
    })
}

fn is_ignored(groups: &[&IgnoreGroup], path: &str, is_dir: bool) -> bool {
    let mut verdict = false;
    for group in groups {
        let relative = if group.base.is_empty() {
            path
        } else if let Some(relative) = path.strip_prefix(&format!("{}/", group.base)) {
            relative
        } else {
            continue;
        };
        for rule in &group.rules {
            if rule.dir_only && !is_dir {
                continue;
            }
            if rule.regex.is_match(relative) {
                verdict = !rule.negated;
            }
        }
    }
    verdict
}

fn mtime_ms(metadata: &Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

/// On Unix, inode change time catches same-size, same-mtime restores.
/// If unavailable, leave size and modification time to decide.
#[cfg(unix)]
fn ctime_ms(metadata: &Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    metadata
        .ctime()
        .saturating_mul(1_000)
        .saturating_add(i64::from(metadata.ctime_nsec() as i32) / 1_000_000)
}

#[cfg(not(unix))]
fn ctime_ms(_metadata: &Metadata) -> i64 {
    0
}

pub fn absolute_path(cwd: &Path, relative: &str) -> PathBuf {
    cwd.join(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    #[test]
    fn skips_files_from_global_and_info_exclude() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let project = tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(project.path())
            .status()
            .unwrap();
        fs::write(project.path().join(".git/info/exclude"), "info.rs\n").unwrap();
        fs::write(project.path().join("info.rs"), "fn info() {}\n").unwrap();
        fs::write(project.path().join("global.rs"), "fn global() {}\n").unwrap();
        let global = tempdir().unwrap();
        fs::write(global.path().join("ignore"), "global.rs\n").unwrap();
        unsafe {
            std::env::set_var("GIT_CONFIG_GLOBAL", global.path().join("config"));
        }
        fs::write(
            global.path().join("config"),
            format!(
                "[core]\n\texcludesFile = {}\n",
                global.path().join("ignore").display()
            ),
        )
        .unwrap();

        let result = walk_files(project.path(), 10);

        unsafe {
            std::env::remove_var("GIT_CONFIG_GLOBAL");
        }
        assert!(result.files.is_empty(), "{:?}", result.files);
    }
}
