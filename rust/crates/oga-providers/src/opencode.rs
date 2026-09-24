//! Which executable an `opencode` profile runs, and the version check both
//! OpenCode providers share. OpenCode 1 and 2 both install as `opencode`, so
//! that name is trusted only once it reports the major version a profile runs.

use std::{
    collections::{BTreeMap, HashMap},
    env, fmt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant, SystemTime},
};

use oga_domain::{Profile, Provider};

use crate::{environment_for, home, worker_path::worker_path};

/// The profile environment key naming the OpenCode 1 executable outright.
pub const OPENCODE_BIN: &str = "OPENCODE_BIN";

/// Where OpenCode installers put `opencode`, searched after `PATH` for a
/// profile that does not set its own: an app started from Finder carries a
/// `PATH` that knows none of them.
const INSTALL_DIRECTORIES: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "~/.opencode/bin",
    "~/.bun/bin",
    "~/.local/bin",
];

const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

/// The version each executable reported, keyed by its resolved path and last
/// change, so an upgrade in place is checked again.
type ReportedVersions = HashMap<(PathBuf, SystemTime), Option<String>>;

static REPORTED: Mutex<Option<ReportedVersions>> = Mutex::new(None);

/// Every `opencode` this profile could run reports another major version than 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoOpenCode1 {
    /// Each `opencode` found, with the version it reported.
    pub found: Vec<(PathBuf, Option<String>)>,
}

impl fmt::Display for NoOpenCode1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let found = self
            .found
            .iter()
            .map(|(path, version)| match version {
                Some(version) => format!("{} is version {version}", path.display()),
                None => format!("{} did not report a version", path.display()),
            })
            .collect::<Vec<_>>()
            .join("; ");
        write!(
            f,
            "This worker runs OpenCode 1.x, and no `opencode` it can find is OpenCode 1: {found}. \
             Use an opencode-2 worker for OpenCode 2, or install OpenCode 1.x, or set \
             {OPENCODE_BIN} in this worker's environment to the full path of an OpenCode 1 install."
        )
    }
}

impl std::error::Error for NoOpenCode1 {}

/// The OpenCode 1 executable for a profile: the path its `OPENCODE_BIN` names,
/// else the first `opencode` that reports version 1, searching the path the
/// worker is started with and then, when the profile sets no `PATH`, the usual
/// install directories. With no `opencode` anywhere the bare name is returned,
/// so a failed start names what is missing.
pub fn opencode_executable(profile: &Profile) -> Result<String, NoOpenCode1> {
    let env = environment_for(profile);
    if let Some(explicit) = explicit(&env, OPENCODE_BIN) {
        return Ok(explicit);
    }
    let mut directories = env::split_paths(
        env.get("PATH")
            .cloned()
            .unwrap_or_else(worker_path)
            .as_str(),
    )
    .collect::<Vec<_>>();
    if !env.contains_key("PATH") {
        let home = home();
        directories.extend(
            INSTALL_DIRECTORIES
                .iter()
                .map(|directory| PathBuf::from(crate::expand_home(directory, &home))),
        );
    }
    first_reporting_v1(&directories)
}

/// Why a task on this profile is refused before it starts, when it is: an
/// OpenCode profile whose every `opencode` reports another version than 1.
pub fn cannot_start(profile: &Profile) -> Option<String> {
    (profile.provider == Provider::OpenCode && profile.command.is_none())
        .then(|| opencode_executable(profile).err())
        .flatten()
        .map(|missing| missing.to_string())
}

fn first_reporting_v1(directories: &[PathBuf]) -> Result<String, NoOpenCode1> {
    let found = all_named("opencode", directories);
    if found.is_empty() {
        return Ok("opencode".to_owned());
    }
    let mut reported = Vec::with_capacity(found.len());
    for candidate in found {
        let version = reported_version(&candidate);
        if version.as_deref().and_then(major) == Some(1) {
            return Ok(candidate.display().to_string());
        }
        reported.push((candidate, version));
    }
    Err(NoOpenCode1 { found: reported })
}

/// A non-empty value the profile's environment gives `key`.
pub(crate) fn explicit(env: &BTreeMap<String, String>, key: &str) -> Option<String> {
    env.get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(crate) fn find_on_path(program: &str, path: &str) -> Option<PathBuf> {
    env::split_paths(std::ffi::OsStr::new(path))
        .map(|directory| directory.join(program))
        .find(|candidate| is_executable(candidate))
}

/// Every executable `program` in `directories`, in order, each install once
/// however many links lead to it.
fn all_named(program: &str, directories: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = Vec::new();
    let mut found = Vec::new();
    for candidate in directories.iter().map(|directory| directory.join(program)) {
        if !is_executable(&candidate) {
            continue;
        }
        let resolved = std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if !seen.contains(&resolved) {
            seen.push(resolved);
            found.push(candidate);
        }
    }
    found
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// The version `<executable> --version` reports, asked once per install.
pub(crate) fn reported_version(executable: &Path) -> Option<String> {
    let resolved = std::fs::canonicalize(executable).ok()?;
    let modified = std::fs::metadata(&resolved)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let key = (resolved, modified);
    if let Some(known) = REPORTED
        .lock()
        .ok()
        .and_then(|cache| cache.as_ref()?.get(&key).cloned())
    {
        return known;
    }
    let answer = version_of(executable).and_then(|printed| version_number(&printed));
    if let Ok(mut cache) = REPORTED.lock() {
        cache
            .get_or_insert_with(HashMap::new)
            .insert(key, answer.clone());
    }
    answer
}

/// What `<executable> --version` prints, or nothing when it fails or hangs.
fn version_of(executable: &Path) -> Option<String> {
    let mut child = Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait().ok()? {
            Some(status) if status.success() => break,
            Some(_) => return None,
            None if started.elapsed() > VERSION_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    }
    let output = child.wait_with_output().ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// OpenCode 2 prints `opencode v2.0.1`; OpenCode 1 prints a bare `1.18.32`.
fn version_number(printed: &str) -> Option<String> {
    printed
        .split_whitespace()
        .last()
        .map(|word| word.trim_start_matches('v').to_owned())
}

pub(crate) fn major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt};

    use oga_domain::Provider;

    use super::*;

    pub(crate) fn script(directory: &Path, name: &str, version: &str) -> PathBuf {
        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\necho '{version}'\n")).expect("script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
        path
    }

    fn profile(env: BTreeMap<String, String>) -> Profile {
        Profile {
            id: "oc".into(),
            label: "OpenCode".into(),
            provider: Provider::OpenCode,
            default_model: "opencode/space-bunny-free".into(),
            enabled: true,
            env,
            capabilities: vec![],
            command: None,
        }
    }

    fn on_path(directories: &[&Path]) -> BTreeMap<String, String> {
        let path = env::join_paths(directories).expect("path");
        BTreeMap::from([("PATH".into(), path.to_string_lossy().into_owned())])
    }

    #[test]
    fn reads_both_version_spellings() {
        let major_of = |printed: &str| version_number(printed).as_deref().and_then(major);
        assert_eq!(major_of("opencode v2.0.1\n"), Some(2));
        assert_eq!(major_of("2.3.0"), Some(2));
        assert_eq!(major_of("1.18.32\n"), Some(1));
        assert_eq!(
            version_number("opencode v2.0.1\n").as_deref(),
            Some("2.0.1")
        );
        assert_eq!(major_of("0.0.0-beta-18999"), Some(0));
        assert_eq!(major_of(""), None);
    }

    #[test]
    fn an_opencode_1_first_on_the_path_is_run() {
        let first = tempfile::tempdir().expect("bin");
        let later = tempfile::tempdir().expect("bin");
        let v1 = script(first.path(), "opencode", "1.18.32");
        script(later.path(), "opencode", "opencode v2.0.1");

        assert_eq!(
            opencode_executable(&profile(on_path(&[first.path(), later.path()]))),
            Ok(v1.display().to_string())
        );
    }

    #[test]
    fn an_opencode_2_first_on_the_path_is_passed_over_for_a_later_opencode_1() {
        let first = tempfile::tempdir().expect("bin");
        let later = tempfile::tempdir().expect("bin");
        script(first.path(), "opencode", "opencode v2.0.1");
        let v1 = script(later.path(), "opencode", "1.18.32");

        assert_eq!(
            opencode_executable(&profile(on_path(&[first.path(), later.path()]))),
            Ok(v1.display().to_string())
        );
    }

    #[test]
    fn only_opencode_2_is_refused_with_what_was_found() {
        let bin = tempfile::tempdir().expect("bin");
        let v2 = script(bin.path(), "opencode", "opencode v2.0.1");

        let refused = opencode_executable(&profile(on_path(&[bin.path()])))
            .expect_err("OpenCode 2 is never run as OpenCode 1");

        assert_eq!(refused.found, [(v2.clone(), Some("2.0.1".to_owned()))]);
        let message = refused.to_string();
        assert!(
            message.contains(&format!("{} is version 2.0.1", v2.display())),
            "{message}"
        );
        assert!(message.contains("opencode-2 worker"), "{message}");
        assert!(message.contains(OPENCODE_BIN), "{message}");
    }

    #[test]
    fn no_opencode_at_all_leaves_the_bare_name_to_fail_on_start() {
        let empty = tempfile::tempdir().expect("bin");

        assert_eq!(
            opencode_executable(&profile(on_path(&[empty.path()]))),
            Ok("opencode".to_owned())
        );
    }

    #[test]
    fn a_configured_path_wins_over_anything_on_the_path() {
        let bin = tempfile::tempdir().expect("bin");
        script(bin.path(), "opencode", "1.18.32");
        let mut env = on_path(&[bin.path()]);
        env.insert(OPENCODE_BIN.into(), "~/tools/opencode-1".into());

        assert_eq!(
            opencode_executable(&profile(env)),
            Ok(format!("{}/tools/opencode-1", crate::home()))
        );
    }
}
