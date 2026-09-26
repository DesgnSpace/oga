//! A worker spawned while the login shell is still reporting its PATH waits
//! for it, so a command only that shell knows about is found.
//!
//! This binary holds one test: it points `SHELL` at a script for the whole
//! process.

use std::{fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};

use oga_domain::Provider;
use oga_runner::{ProviderRunner, RunRequest, worker_path};
use tempfile::TempDir;

fn executable(path: &Path, body: &str) {
    fs::write(path, body).expect("script written");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("script executable");
}

#[tokio::test]
async fn a_spawn_during_the_login_shell_capture_gets_the_captured_path() {
    let temp = TempDir::new().expect("temporary directory");
    let only_the_shell_knows = temp.path().join("login-bin");
    fs::create_dir(&only_the_shell_knows).expect("login bin");
    executable(
        &only_the_shell_knows.join("oga-login-only-worker"),
        "#!/bin/sh\necho found through the login shell\n",
    );
    let started = temp.path().join("shell-started");
    executable(
        &temp.path().join("slow-shell"),
        &format!(
            "#!/bin/sh\ntouch '{}'\nsleep 1\nprintf '__OGA_PATH__%s__OGA_END__' '{}'\n",
            started.display(),
            only_the_shell_knows.display()
        ),
    );
    // SAFETY: set before any other thread of this test binary reads the
    // environment.
    unsafe { std::env::set_var("SHELL", temp.path().join("slow-shell")) };

    // The broker starts the capture as it starts serving, on another thread
    // than the one that later spawns workers.
    std::thread::spawn(worker_path::warm_login_path);
    for _ in 0..200 {
        if started.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(started.exists(), "the login shell never started");

    let result = ProviderRunner::default()
        .spawn(RunRequest::new(
            Provider::Claude,
            vec!["oga-login-only-worker".into()],
            temp.path(),
        ))
        .await
        .expect("worker found on the login shell's PATH")
        .wait()
        .await
        .expect("worker result");
    assert!(
        result.stdout.contains("found through the login shell"),
        "{}",
        result.stdout
    );
}
