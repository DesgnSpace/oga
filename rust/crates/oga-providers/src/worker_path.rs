//! PATH for spawned workers, merged with the login shell's own.

use std::{
    env,
    io::Read,
    process::{Child, Command, Stdio},
    sync::{Condvar, Mutex, mpsc},
    thread,
    time::Duration,
};

/// Rc files that load plugin managers take 3–15 s on a busy machine; one that
/// hangs is killed after this.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);
const START: &str = "__OGA_PATH__";
const END: &str = "__OGA_END__";

struct LoginPath {
    captured: Option<String>,
    capturing: bool,
    /// The capture the broker starts with is still running; spawns wait for it.
    startup_pending: bool,
}

static LOGIN_PATH: Mutex<LoginPath> = Mutex::new(LoginPath {
    captured: None,
    capturing: false,
    startup_pending: false,
});
static CAPTURE_SETTLED: Condvar = Condvar::new();

/// The broker outlives the session that launched it, so `PATH` is a snapshot
/// from whenever that was — and an app launched from Finder carries almost
/// nothing. A CLI on a directory only the user's shell knows about is then
/// invisible, and the spawn fails with "No such file or directory" on a
/// command the same user runs fine in a terminal. The login shell's own PATH
/// closes that gap.
pub fn worker_path() -> String {
    merge(
        &env::var("PATH").unwrap_or_default(),
        login_path().as_deref(),
    )
}

/// Capture the login shell's PATH on its own thread and return at once, so the
/// broker can serve while the shell loads. Spawns wait for this capture.
pub fn warm_login_path() {
    if let Ok(mut state) = LOGIN_PATH.lock() {
        state.startup_pending = true;
    }
    refresh_login_path();
}

/// Whether [`worker_path`] would wait on the broker's startup capture.
pub fn awaits_startup_capture() -> bool {
    LOGIN_PATH.lock().is_ok_and(|state| state.startup_pending)
}

/// Capture the login shell's PATH again on its own thread. Called when a spawn
/// cannot find its command, which may sit in a directory the shell gained since.
pub fn refresh_login_path() {
    let Ok(mut state) = LOGIN_PATH.lock() else {
        return;
    };
    if state.capturing {
        return;
    }
    state.capturing = true;
    drop(state);
    let spawned = thread::Builder::new()
        .name("login-path".into())
        .spawn(|| settle_capture(capture_login_path()));
    if spawned.is_err() {
        settle_capture(None);
    }
}

fn login_path() -> Option<String> {
    let state = LOGIN_PATH.lock().ok()?;
    let (state, _) = CAPTURE_SETTLED
        .wait_timeout_while(state, CAPTURE_TIMEOUT, |state| state.startup_pending)
        .ok()?;
    state.captured.clone()
}

fn settle_capture(captured: Option<String>) {
    if let Ok(mut state) = LOGIN_PATH.lock() {
        if captured.is_some() {
            state.captured = captured;
        }
        state.capturing = false;
        state.startup_pending = false;
    }
    CAPTURE_SETTLED.notify_all();
}

/// Login *and* interactive: PATH edits live in profile files and rc files
/// alike, and a shell that sources only one of them misses half the installs.
/// The value is fenced by sentinels because rc files print banners, and a
/// banner concatenated into PATH breaks every later lookup. The shell gets its
/// own session so it cannot stop on the broker's terminal, and can be killed whole.
fn capture_login_path() -> Option<String> {
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
    let mut command = Command::new(shell);
    command
        .args(["-lic", &format!("printf '{START}%s{END}' \"$PATH\"")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    detach_session(&mut command);
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take();
    let (sender, receiver) = mpsc::channel();
    if let Some(stdout) = stdout {
        thread::spawn(move || {
            if let Some(path) = read_fenced(stdout) {
                let _ = sender.send(path);
            }
        });
    }
    let captured = receiver.recv_timeout(CAPTURE_TIMEOUT).ok();
    kill_session(&mut child);
    captured
}

/// Reads until the fenced value is complete rather than to end of file: a
/// background job an rc file starts can hold the pipe open long after the
/// shell has printed.
fn read_fenced(mut stdout: impl Read) -> Option<String> {
    let mut seen = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let read = stdout.read(&mut chunk).ok().filter(|read| *read > 0)?;
        seen.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&seen);
        if let Some(start) = text.find(START).map(|index| index + START.len())
            && let Some(end) = text[start..].find(END)
        {
            let value = &text[start..start + end];
            return (!value.is_empty()).then(|| value.to_owned());
        }
    }
}

#[cfg(unix)]
fn detach_session(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `setsid` is async-signal-safe and touches no parent state.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(not(unix))]
fn detach_session(_command: &mut Command) {}

fn kill_session(child: &mut Child) {
    #[cfg(unix)]
    {
        // SAFETY: the shell leads its own process group and is not reaped
        // yet, so the group id still names it.
        unsafe {
            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = child.kill();
    let _ = child.wait();
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
