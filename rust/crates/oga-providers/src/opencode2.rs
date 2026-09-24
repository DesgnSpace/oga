//! Which executable an `opencode-2` profile runs. OpenCode 1 and 2 both install
//! as `opencode`, so that name is trusted only once it reports version 2.

use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant, SystemTime},
};

use oga_domain::Profile;

use crate::{environment_for, worker_path::worker_path};

/// The profile environment key naming the OpenCode 2 executable outright.
pub const OPENCODE2_BIN: &str = "OPENCODE2_BIN";

const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether an executable reported OpenCode 2, keyed by its resolved path and
/// last change, so an upgrade in place is checked again.
static REPORTS_V2: Mutex<Option<HashMap<(PathBuf, SystemTime), bool>>> = Mutex::new(None);

/// The OpenCode 2 executable for a profile: the path its `OPENCODE2_BIN`
/// names, else `opencode2` on the path the worker is started with, else an
/// `opencode` there that reports version 2. With none of them the bare name
/// `opencode2` is returned, so a failed start names what is missing.
pub fn opencode2_executable(profile: &Profile) -> String {
    let env = environment_for(profile);
    if let Some(explicit) = env
        .get(OPENCODE2_BIN)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return explicit.to_owned();
    }
    let path = env.get("PATH").cloned().unwrap_or_else(worker_path);
    if let Some(found) = find_on_path("opencode2", &path) {
        return found.display().to_string();
    }
    find_on_path("opencode", &path)
        .filter(|candidate| reports_v2(candidate))
        .map_or_else(
            || "opencode2".to_owned(),
            |found| found.display().to_string(),
        )
}

fn find_on_path(program: &str, path: &str) -> Option<PathBuf> {
    env::split_paths(std::ffi::OsStr::new(path))
        .map(|directory| directory.join(program))
        .find(|candidate| is_executable(candidate))
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

fn reports_v2(executable: &Path) -> bool {
    let Ok(resolved) = std::fs::canonicalize(executable) else {
        return false;
    };
    let modified = std::fs::metadata(&resolved)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let key = (resolved, modified);
    if let Some(known) = REPORTS_V2
        .lock()
        .ok()
        .and_then(|cache| cache.as_ref()?.get(&key).copied())
    {
        return known;
    }
    let answer = version_of(executable).is_some_and(|version| is_v2(&version));
    if let Ok(mut cache) = REPORTS_V2.lock() {
        cache.get_or_insert_with(HashMap::new).insert(key, answer);
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
fn is_v2(version: &str) -> bool {
    version
        .split_whitespace()
        .last()
        .map(|word| word.trim_start_matches('v'))
        .and_then(|number| number.split('.').next())
        .is_some_and(|major| major == "2")
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt};

    use oga_domain::Provider;

    use super::*;

    fn profile(env: BTreeMap<String, String>) -> Profile {
        Profile {
            id: "oc2".into(),
            label: "OpenCode 2".into(),
            provider: Provider::OpenCode2,
            default_model: "opencode/space-bunny-free".into(),
            enabled: true,
            env,
            capabilities: vec![],
            command: None,
        }
    }

    fn script(directory: &Path, name: &str, version: &str) -> PathBuf {
        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\necho '{version}'\n")).expect("script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
        path
    }

    fn on_path(directory: &Path) -> BTreeMap<String, String> {
        BTreeMap::from([("PATH".into(), directory.display().to_string())])
    }

    #[test]
    fn reads_both_version_spellings() {
        assert!(is_v2("opencode v2.0.1\n"));
        assert!(is_v2("2.3.0"));
        assert!(!is_v2("1.18.32"));
        assert!(!is_v2("0.0.0-beta-18999"));
        assert!(!is_v2(""));
    }

    #[test]
    fn a_configured_path_wins_over_anything_on_the_path() {
        let bin = tempfile::tempdir().expect("bin");
        script(bin.path(), "opencode2", "opencode v2.0.1");
        let mut env = on_path(bin.path());
        env.insert(OPENCODE2_BIN.into(), "~/tools/opencode-next".into());

        assert_eq!(
            opencode2_executable(&profile(env)),
            format!("{}/tools/opencode-next", crate::home())
        );
    }

    #[test]
    fn opencode2_is_preferred_over_an_opencode_that_reports_v2() {
        let bin = tempfile::tempdir().expect("bin");
        let named = script(bin.path(), "opencode2", "opencode v2.0.1");
        script(bin.path(), "opencode", "opencode v2.0.1");

        assert_eq!(
            opencode2_executable(&profile(on_path(bin.path()))),
            named.display().to_string()
        );
    }

    #[test]
    fn opencode_is_used_only_when_it_reports_v2() {
        let v2 = tempfile::tempdir().expect("bin");
        let opencode = script(v2.path(), "opencode", "opencode v2.0.1");
        assert_eq!(
            opencode2_executable(&profile(on_path(v2.path()))),
            opencode.display().to_string()
        );

        let v1 = tempfile::tempdir().expect("bin");
        script(v1.path(), "opencode", "1.18.32");
        assert_eq!(
            opencode2_executable(&profile(on_path(v1.path()))),
            "opencode2",
            "an OpenCode 1 under the shared name is never run as OpenCode 2"
        );
    }
}
