//! Which files the index reads. A file is a candidate when an adapter owns
//! its extension, git is not ignoring it, and it is not a lockfile.

use std::fs::{self, DirEntry, Metadata};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::lang;

#[derive(Debug, Clone)]
pub struct WalkFile {
    pub path: String,
    pub size: u64,
    pub mtime_ms: i64,
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

pub fn walk_files(cwd: &Path, max_files: usize) -> WalkResult {
    let mut result = WalkResult::default();
    let mut ignores = Vec::new();
    walk_directory(cwd, cwd, max_files, &mut ignores, &mut result);
    result
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    result
}

/// True when some adapter would parse this path.
pub fn is_indexable(path: &str) -> bool {
    lang::adapter_for(path).is_some()
}

fn walk_directory(
    cwd: &Path,
    directory: &Path,
    max_files: usize,
    ignores: &mut Vec<IgnoreGroup>,
    result: &mut WalkResult,
) {
    if result.partial {
        return;
    }
    let Ok(entries) = read_sorted(directory) else {
        return;
    };
    let group_start = ignores.len();
    if let Ok(source) = fs::read_to_string(directory.join(".gitignore")) {
        ignores.push(IgnoreGroup {
            base: relative_path(cwd, directory),
            rules: parse_ignore_rules(&source),
        });
    }

    let mut directories = Vec::new();
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let relative = relative_path(cwd, &path);
        if metadata.is_dir() {
            if !EXCLUDED_DIRS.contains(&name.as_ref()) && !is_ignored(ignores, &relative, true) {
                directories.push(path);
            }
            continue;
        }
        if !metadata.is_file()
            || !is_indexable(&relative)
            || LOCKFILES.contains(&name.as_ref())
            || is_ignored(ignores, &relative, false)
        {
            continue;
        }
        if result.files.len() == max_files {
            result.partial = true;
            break;
        }
        result.files.push(WalkFile {
            path: relative,
            size: metadata.len(),
            mtime_ms: mtime_ms(&metadata),
        });
    }

    if !result.partial {
        for directory in directories {
            walk_directory(cwd, &directory, max_files, ignores, result);
            if result.partial {
                break;
            }
        }
    }
    ignores.truncate(group_start);
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
            other => expression.push_str(&regex::escape(&other.to_string())),
        }
    }
    let prefix = if anchored { "^" } else { "^(?:.*/)?" };
    regex::Regex::new(&format!("{prefix}{expression}$")).ok()
}

fn is_ignored(groups: &[IgnoreGroup], path: &str, is_dir: bool) -> bool {
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

pub fn absolute_path(cwd: &Path, relative: &str) -> PathBuf {
    cwd.join(relative)
}
