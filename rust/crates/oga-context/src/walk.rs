use std::fs::{self, DirEntry, Metadata};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use oga_domain::SourceLang;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericLanguage {
    Python,
    Go,
    Rust,
    Java,
    Kotlin,
    Php,
    Ruby,
    C,
    Cpp,
    Csharp,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageEntry {
    pub lang: SourceLang,
    pub generic: Option<GenericLanguage>,
}

#[derive(Debug, Clone)]
pub struct ContextWalkFile {
    pub path: String,
    pub entry: LanguageEntry,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, Copy)]
pub struct WalkOptions {
    pub max_files: usize,
    pub budget: Duration,
}

#[derive(Debug, Default)]
pub struct WalkResult {
    pub files: Vec<ContextWalkFile>,
    pub partial: bool,
}

const EXCLUDED_DIRS: &[&str] = &["node_modules", "dist", ".build", ".git"];
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

pub fn walk_context_files(cwd: &Path, options: WalkOptions) -> WalkResult {
    let started = Instant::now();
    let mut result = WalkResult::default();
    let mut ignores = Vec::new();
    let mut walked_files = 0;
    walk_directory(
        cwd,
        cwd,
        options,
        started,
        &mut ignores,
        &mut walked_files,
        &mut result,
    );
    result
}

pub fn mapped_extension(path: &str) -> Option<LanguageEntry> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())?
        .to_ascii_lowercase();
    let entry = match extension.as_str() {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => LanguageEntry {
            lang: SourceLang::Ts,
            generic: None,
        },
        "swift" => LanguageEntry {
            lang: SourceLang::Swift,
            generic: None,
        },
        "py" => generic(GenericLanguage::Python),
        "rb" => generic(GenericLanguage::Ruby),
        "go" => generic(GenericLanguage::Go),
        "rs" => generic(GenericLanguage::Rust),
        "java" => generic(GenericLanguage::Java),
        "kt" => generic(GenericLanguage::Kotlin),
        "php" => generic(GenericLanguage::Php),
        "c" | "h" => generic(GenericLanguage::C),
        "cpp" | "cc" | "hpp" | "hh" => generic(GenericLanguage::Cpp),
        "cs" => generic(GenericLanguage::Csharp),
        _ => return None,
    };
    Some(entry)
}

fn generic(language: GenericLanguage) -> LanguageEntry {
    LanguageEntry {
        lang: SourceLang::Generic,
        generic: Some(language),
    }
}

fn walk_directory(
    cwd: &Path,
    directory: &Path,
    options: WalkOptions,
    started: Instant,
    ignores: &mut Vec<IgnoreGroup>,
    walked_files: &mut usize,
    result: &mut WalkResult,
) {
    if result.partial {
        return;
    }
    if started.elapsed() > options.budget {
        result.partial = true;
        return;
    }

    let entries = match read_sorted(directory) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let group_start = ignores.len();
    let ignore_path = directory.join(".gitignore");
    if let Ok(source) = fs::read_to_string(&ignore_path) {
        let base = relative_path(cwd, directory);
        ignores.push(IgnoreGroup {
            base,
            rules: parse_ignore_rules(&source),
        });
    }

    let mut directories = Vec::new();
    for entry in entries {
        if result.partial || started.elapsed() > options.budget {
            result.partial = true;
            break;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let relative = relative_path(cwd, &path);
        if metadata.is_dir() {
            if !EXCLUDED_DIRS.iter().any(|excluded| *excluded == name)
                && !is_ignored(ignores, &relative, true)
            {
                directories.push(path);
            }
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        *walked_files += 1;
        if *walked_files > options.max_files {
            result.partial = true;
            break;
        }
        let Some(entry) = mapped_extension(&relative) else {
            continue;
        };
        if LOCKFILES.iter().any(|lockfile| *lockfile == name)
            || is_ignored(ignores, &relative, false)
        {
            continue;
        }
        result.files.push(ContextWalkFile {
            path: relative,
            entry,
            metadata,
        });
    }

    if !result.partial && started.elapsed() > options.budget {
        result.partial = true;
    }
    if !result.partial {
        for directory in directories {
            walk_directory(
                cwd,
                &directory,
                options,
                started,
                ignores,
                walked_files,
                result,
            );
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

pub fn mtime_ms(metadata: &Metadata) -> i64 {
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
