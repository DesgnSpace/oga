use std::{fs, path::PathBuf};

use oga_domain::{Provider, TaskEvent, TaskEventView};

/// A real Claude run over ACP, trimmed to two shell calls that open titled
/// only "Terminal" and receive their command in a later update.
fn views() -> Vec<TaskEventView> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude-acp/events.jsonl");
    let events = fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<TaskEvent>(line).unwrap())
        .collect();
    oga_events::event_views(
        events,
        &oga_events::ProviderTimeline::single(Provider::Claude),
    )
}

#[test]
fn a_finished_command_keeps_the_command_its_update_brought() {
    let views = views();
    let call: Vec<_> = views
        .iter()
        .filter(|view| {
            view.source_id.as_ref().and_then(Option::as_deref)
                == Some("toolu_01AFbRDHENCmaTrS2E6qLP6u")
        })
        .collect();
    let command = call
        .last()
        .unwrap()
        .presentation
        .as_ref()
        .unwrap()
        .command
        .as_deref()
        .unwrap();
    // The completing update carries only the output; the command must still
    // be the one the worker ran, never the "Terminal" title or the output.
    assert!(
        command.starts_with("grep -v '^#' /tmp/bub.txt"),
        "{command}"
    );
}

/// Rewrites the rows the web tests read: `cargo test -p oga-events --test
/// claude_acp -- --ignored`.
#[test]
#[ignore = "rewrites web/src/domain/activity/fixtures/claude-acp-terminal.json"]
fn refresh_the_web_fixture() {
    let json = format!("{}\n", serde_json::to_string_pretty(&views()).unwrap());
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../web/src/domain/activity/fixtures/claude-acp-terminal.json");
    fs::write(output, json).unwrap();
}
