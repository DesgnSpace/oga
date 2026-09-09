use std::{collections::BTreeMap, fs, path::Path, time::Duration};

use oga_domain::Provider;
use oga_runner::{
    Liveness, OutputStream, ProviderRunner, RunRequest, RunnerConfig, RunnerEvent, Termination,
};
use tempfile::TempDir;
use tokio::time::sleep;

fn fake(mode: &str, cwd: &Path) -> RunRequest {
    fake_for(Provider::Claude, mode, cwd)
}

fn fake_for(provider: Provider, mode: &str, cwd: &Path) -> RunRequest {
    RunRequest::new(
        provider,
        vec![
            std::env::var("CARGO_BIN_EXE_fake-provider").expect("fake provider binary"),
            mode.into(),
        ],
        cwd,
    )
}

#[tokio::test]
async fn codex_events_arrive_before_the_process_exits() {
    let temp = TempDir::new().expect("temporary directory");
    let process = runner(8 * 1024, 64 * 1024)
        .spawn(
            fake_for(Provider::Codex, "codex-stream", temp.path())
                .with_env(env(&[("FAKE_PROVIDER_DELAY_MS", "100")])),
        )
        .await
        .expect("provider spawned");
    let mut events = process
        .take_provider_events()
        .expect("provider event stream");

    let first = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("first Codex event before process exit")
        .expect("Codex event");
    assert_eq!(first.provider, Provider::Codex);
    assert_eq!(first.payload["type"], "thread.started");
    assert!(matches!(process.liveness(), Liveness::Alive));

    let second = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("second Codex event before process exit")
        .expect("Codex event");
    assert_eq!(second.payload["type"], "item.started");

    let result = process.wait().await.expect("provider result");
    assert_eq!(result.events.len(), 4);
}

fn runner(max_output_bytes: usize, max_line_bytes: usize) -> ProviderRunner {
    ProviderRunner::new(RunnerConfig {
        max_output_bytes,
        max_line_bytes,
        max_events: 64,
        event_buffer: 64,
        interrupt_grace: Duration::from_millis(20),
        terminate_grace: Duration::from_millis(20),
    })
    .expect("runner configuration")
}

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect()
}

#[tokio::test]
async fn captures_provider_events_session_usage_and_liveness() {
    let temp = TempDir::new().expect("temporary directory");
    let process = runner(8 * 1024, 64 * 1024)
        .spawn(fake("basic", temp.path()))
        .await
        .expect("provider spawned");
    assert_eq!(process.identity().pgid, process.identity().pid as i32);
    assert!(matches!(process.liveness(), Liveness::Alive));

    let result = process.wait().await.expect("provider result");

    assert_eq!(result.termination, Termination::Exited);
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.session_id.as_deref(), Some("fake-session"));
    assert_eq!(result.usage.tokens_in, Some(11.0));
    assert_eq!(result.usage.tokens_out, Some(7.0));
    assert_eq!(result.usage.cost_usd, Some(0.25));
    assert_eq!(result.usage.turns, Some(1.0));
    assert!(result.stdout.contains("fake provider completed"));
    assert!(
        matches!(process.liveness(), Liveness::Gone),
        "{:?}",
        process.liveness()
    );
}

#[tokio::test]
async fn provider_event_stream_keeps_burst_events() {
    let temp = TempDir::new().expect("temporary directory");
    let process = ProviderRunner::default()
        .spawn(fake("burst", temp.path()).with_env(env(&[("FAKE_PROVIDER_LINES", "1000")])))
        .await
        .expect("provider spawned");
    let mut events = process
        .take_provider_events()
        .expect("provider event stream");
    let mut observed = 0;
    while events.recv().await.is_some() {
        observed += 1;
    }
    let result = process.wait().await.expect("provider result");

    assert_eq!(observed, 1002);
    assert_eq!(observed, result.events.len());
    assert_eq!(result.events_dropped, 0);
}

#[tokio::test]
async fn streams_stderr_and_bounds_malformed_and_oversized_output() {
    let temp = TempDir::new().expect("temporary directory");
    let process = runner(512, 512)
        .spawn(fake("oversized", temp.path()).with_env(env(&[
            ("FAKE_PROVIDER_SIZE", "4096"),
            ("FAKE_PROVIDER_DELAY_MS", "50"),
        ])))
        .await
        .expect("provider spawned");
    let mut events = process.subscribe();
    let result = process.wait().await.expect("provider result");

    assert!(result.stdout.len() <= 512);
    assert!(result.stdout_truncated);
    assert_eq!(result.oversized_lines, 1);
    assert!(result.events.len() >= 2);
    let mut saw_truncation = false;
    while let Ok(event) = events.try_recv() {
        if matches!(
            event,
            RunnerEvent::OutputTruncated {
                stream: OutputStream::Stdout,
                ..
            }
        ) {
            saw_truncation = true;
        }
    }
    assert!(saw_truncation);

    let malformed = runner(512, 512)
        .run(fake("malformed", temp.path()))
        .await
        .expect("malformed provider result");
    assert_eq!(malformed.malformed_lines, 1);

    let stderr = runner(512, 512)
        .run(fake("stderr", temp.path()))
        .await
        .expect("stderr provider result");
    assert!(stderr.stderr.contains("fake provider warning"));
    assert!(stderr.stdout.contains("fake provider completed"));
}

/// Some providers read meaning from whether a variable exists at all, so a
/// value the broker exports has to be taken away from the child rather than
/// overwritten.
#[tokio::test]
async fn env_remove_takes_an_inherited_variable_away_from_the_child() {
    let temp = TempDir::new().expect("temporary directory");
    // SAFETY: test-only env mutation; only the fake provider reads this.
    unsafe { std::env::set_var("FAKE_PROVIDER_PROBE", "from-the-broker") };

    let inherited = runner(512, 512)
        .run(fake("probe-env", temp.path()))
        .await
        .expect("probe result");
    assert!(
        inherited
            .stderr
            .contains("FAKE_PROVIDER_PROBE=from-the-broker")
    );

    let mut request = fake("probe-env", temp.path());
    request.env_remove.insert("FAKE_PROVIDER_PROBE".into());
    let removed = runner(512, 512).run(request).await.expect("probe result");
    assert!(removed.stderr.contains("FAKE_PROVIDER_PROBE=<absent>"));
}

#[tokio::test]
async fn timeout_escalates_and_returns_a_timed_out_result() {
    let temp = TempDir::new().expect("temporary directory");
    let result = runner(512, 128)
        .run(
            fake("sleep", temp.path())
                .with_env(env(&[("FAKE_PROVIDER_SLEEP_MS", "10_000")]))
                .with_timeout(Some(Duration::from_millis(40))),
        )
        .await
        .expect("timed out provider result");

    assert_eq!(result.termination, Termination::TimedOut);
    assert!(result.elapsed < Duration::from_secs(2));
    assert!(result.signal.is_some() || result.exit_code.is_some());
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_kills_grandchildren_in_the_detached_group() {
    let temp = TempDir::new().expect("temporary directory");
    let child_pid_file = temp.path().join("child.pid");
    let process = runner(512, 128)
        .spawn(fake("child", temp.path()).with_env(env(&[(
            "FAKE_PROVIDER_CHILD_PID_FILE",
            child_pid_file.to_str().expect("pid path"),
        )])))
        .await
        .expect("provider spawned");

    let child_pid = wait_for_pid(&child_pid_file).await;
    process.cancel().await;
    let result = process.wait().await.expect("cancelled provider result");

    assert_eq!(result.termination, Termination::Cancelled);
    assert!(wait_for_gone(child_pid).await);
    assert!(
        matches!(process.liveness(), Liveness::Gone),
        "{:?}",
        process.liveness()
    );
}

async fn wait_for_pid(path: &Path) -> u32 {
    for _ in 0..100 {
        if let Ok(value) = fs::read_to_string(path)
            && let Ok(pid) = value.trim().parse()
        {
            return pid;
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!("fake provider did not write a child pid");
}

async fn wait_for_gone(pid: u32) -> bool {
    for _ in 0..100 {
        let alive = unsafe { libc::kill(pid as libc::pid_t, 0) == 0 };
        if !alive {
            return true;
        }
        sleep(Duration::from_millis(10)).await;
    }
    false
}
