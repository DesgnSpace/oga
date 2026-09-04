use std::fs;

use oga_domain::TaskScope;
use oga_runner::{
    Capability, ConfinementBackend, ConfinementError, ConfinementMode, ConfinementRequest,
    LinuxBubblewrap, MacSeatbelt, NoConfinement,
};
use tempfile::TempDir;

fn request(temp: &TempDir, scope: TaskScope) -> ConfinementRequest {
    ConfinementRequest::new(
        vec!["/bin/echo".into(), "safe;argument".into()],
        temp.path(),
        scope,
    )
}

#[test]
fn confinement_policy_is_deny_by_default_and_scopes_reads_and_writes() {
    let temp = TempDir::new().expect("temporary directory");
    fs::create_dir(temp.path().join("src")).expect("source directory");
    let profile = MacSeatbelt::default()
        .profile(&request(
            &temp,
            TaskScope {
                read: vec!["src/**".into()],
                write: vec!["result.txt".into()],
            },
        ))
        .expect("Seatbelt profile");

    assert!(profile.contains("(deny default)"));
    assert!(profile.contains("file-read*"));
    assert!(profile.contains("file-write*"));
    let src = fs::canonicalize(temp.path().join("src")).expect("canonical source directory");
    assert!(profile.contains(src.to_string_lossy().as_ref()));
    assert!(profile.contains(temp.path().join("result.txt").to_string_lossy().as_ref()));
}

#[test]
fn confinement_rejects_parent_and_absolute_scope_rules() {
    let temp = TempDir::new().expect("temporary directory");
    for rule in ["../outside", "/tmp/outside"] {
        let result = MacSeatbelt::default().profile(&request(
            &temp,
            TaskScope {
                read: vec![rule.into()],
                write: vec![],
            },
        ));
        assert!(matches!(result, Err(ConfinementError::InvalidScope { .. })));
    }
}

#[cfg(unix)]
#[test]
fn confinement_rejects_symlink_scope_escape() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("temporary directory");
    let outside = TempDir::new().expect("outside directory");
    symlink(outside.path(), temp.path().join("link")).expect("scope symlink");
    let result = MacSeatbelt::default().profile(&request(
        &temp,
        TaskScope {
            read: vec!["link/**".into()],
            write: vec![],
        },
    ));

    assert!(matches!(result, Err(ConfinementError::InvalidScope { .. })));
}

#[test]
fn no_confinement_preserves_literal_provider_arguments() {
    let temp = TempDir::new().expect("temporary directory");
    let request = request(
        &temp,
        TaskScope {
            read: vec!["**".into()],
            write: vec!["**".into()],
        },
    );
    let prepared = NoConfinement.prepare(&request).expect("unconfined command");
    assert_eq!(prepared.argv, request.argv);
    assert!(NoConfinement.probe().is_available());
}

#[test]
fn unavailable_backend_never_falls_back_to_unconfined_execution() {
    let temp = TempDir::new().expect("temporary directory");
    let backend = LinuxBubblewrap::with_executable(temp.path().join("missing-bwrap"));
    let result = backend.prepare(&request(&temp, TaskScope::default()));

    assert!(matches!(
        result,
        Err(ConfinementError::Unavailable {
            backend: ConfinementMode::LinuxBubblewrap,
            ..
        })
    ));
    assert!(matches!(backend.probe(), Capability::Unavailable { .. }));
}

#[test]
fn mode_defaults_to_unconfined_and_reports_backend_capability() {
    assert_eq!(ConfinementMode::default(), ConfinementMode::None);
    assert!(ConfinementMode::None.probe().is_available());
}

#[cfg(target_os = "linux")]
#[test]
fn bubblewrap_capability_runs_a_namespaced_process() {
    use std::process::Command;

    let cwd = TempDir::new().expect("working directory");
    let backend = LinuxBubblewrap::default();
    let capability = backend.probe();
    assert!(
        capability.is_available(),
        "Linux CI requires Bubblewrap: {}",
        capability.reason().unwrap_or("unknown reason")
    );
    let prepared = backend
        .prepare(&ConfinementRequest::new(
            vec!["/bin/true".into()],
            cwd.path(),
            TaskScope::default(),
        ))
        .expect("Bubblewrap command");
    let output = Command::new(&prepared.argv[0])
        .args(&prepared.argv[1..])
        .output()
        .expect("Bubblewrap process");
    assert!(
        output.status.success(),
        "Bubblewrap namespace probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "macos")]
#[test]
fn seatbelt_confinement_blocks_writes_outside_the_grant() {
    use std::process::Command;

    let cwd = TempDir::new().expect("working directory");
    let outside =
        TempDir::new_in(cwd.path().parent().expect("temporary parent")).expect("outside directory");
    let allowed = cwd.path().join("allowed");
    fs::create_dir(&allowed).expect("allowed directory");
    let allowed_file = allowed.join("inside");
    let denied_file = outside.path().join("outside");
    let script = format!(
        "touch '{}' '{}'",
        allowed_file.display(),
        denied_file.display()
    );
    let prepared = MacSeatbelt::default()
        .prepare(&ConfinementRequest::new(
            vec!["/bin/sh".into(), "-c".into(), script],
            cwd.path(),
            TaskScope {
                read: vec![],
                write: vec!["allowed/**".into()],
            },
        ))
        .expect("Seatbelt command");
    let _status = Command::new(&prepared.argv[0])
        .args(&prepared.argv[1..])
        .current_dir(cwd.path())
        .output()
        .expect("Seatbelt process");

    assert!(allowed_file.is_file());
    assert!(!denied_file.exists());
}
