//! PATH for spawned workers, merged with the login shell's own.

use std::{
    env,
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const REFRESH_TTL: Duration = Duration::from_secs(60);
const START: &str = "__OGA_PATH__";
const END: &str = "__OGA_END__";

static LOGIN_PATH: Mutex<Option<(String, Instant)>> = Mutex::new(None);
static REFRESHING: AtomicBool = AtomicBool::new(false);

/// The broker outlives the session that launched it, so `PATH` is a snapshot
/// from whenever that was — and an app launched from Finder carries almost
/// nothing. A CLI on a directory only the user's shell knows about is then
/// invisible, and the spawn fails with "No such file or directory" on a
/// command the same user runs fine in a terminal. The login shell's own PATH
/// closes that gap, and is read on demand so the broker heals without a
/// restart.
pub fn worker_path() -> String {
    merge(
        &env::var("PATH").unwrap_or_default(),
        login_path().as_deref(),
    )
}

/// Capture the login shell's PATH now, on the calling thread. The broker
/// calls this once at start so the first spawn already has it.
pub fn warm_login_path() {
    refresh_login_path();
}

/// The last captured login PATH. A stale value is handed back as it is and a
/// fresh capture starts on its own thread: the shell takes over a second to
/// start, and a spawn must never wait on it.
fn login_path() -> Option<String> {
    let cached = LOGIN_PATH.lock().ok()?.clone();
    let fresh = cached
        .as_ref()
        .is_some_and(|(_, read_at)| read_at.elapsed() < REFRESH_TTL);
    if !fresh && !REFRESHING.swap(true, Ordering::AcqRel) {
        thread::spawn(refresh_login_path);
    }
    cached.map(|(path, _)| path)
}

fn refresh_login_path() {
    let captured = capture_login_path();
    if let Some(captured) = captured
        && let Ok(mut cached) = LOGIN_PATH.lock()
    {
        *cached = Some((captured, Instant::now()));
    }
    REFRESHING.store(false, Ordering::Release);
}

/// Login *and* interactive: PATH edits live in profile files and rc files
/// alike, and a shell that sources only one of them misses half the installs.
/// The value is fenced by sentinels because rc files print banners, and a
/// banner concatenated into PATH breaks every later lookup.
fn capture_login_path() -> Option<String> {
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
    let output = Command::new(shell)
        .args(["-lic", &format!("printf '{START}%s{END}' \"$PATH\"")])
        .output()
        .ok()?;
    let captured = String::from_utf8_lossy(&output.stdout);
    let start = captured.find(START)? + START.len();
    let end = captured[start..].find(END)? + start;
    Some(captured[start..end].to_owned()).filter(|value: &String| !value.is_empty())
}

/// The broker's own entries stay in front, so they still win on ambiguity.
fn merge(base: &str, extra: Option<&str>) -> String {
    let Some(extra) = extra else {
        return base.to_owned();
    };
    let mut merged: Vec<&str> = base.split(':').filter(|entry| !entry.is_empty()).collect();
    for entry in extra.split(':').filter(|entry| !entry.is_empty()) {
        if !merged.contains(&entry) {
            merged.push(entry);
        }
    }
    merged.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_appends_only_unseen_entries_and_keeps_base_order() {
        assert_eq!(
            merge("/usr/bin:/bin", Some("/opt/homebrew/bin:/usr/bin")),
            "/usr/bin:/bin:/opt/homebrew/bin"
        );
        assert_eq!(merge("/usr/bin", None), "/usr/bin");
        assert_eq!(merge("", Some("/opt/bin")), "/opt/bin");
    }
}
