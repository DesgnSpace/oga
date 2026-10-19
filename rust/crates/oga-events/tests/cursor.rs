use std::{fs, path::PathBuf};

use oga_domain::{EventPhase, Provider, TaskEvent, TaskEventView};
use serde_json::Value;

/// A real Cursor run over ACP, trimmed to the calls that read badly: a new
/// file, edits, a file search, a text search, ranged reads and shell runs.
fn views() -> Vec<TaskEventView> {
    views_as(Provider::Cursor)
}

fn views_as(provider: Provider) -> Vec<TaskEventView> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cursor/events.jsonl");
    let events = fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<TaskEvent>(line).unwrap())
        .collect();
    oga_events::event_views(events, &oga_events::ProviderTimeline::single(provider))
}

fn call<'a>(views: &'a [TaskEventView], id: &str) -> Vec<&'a TaskEventView> {
    views
        .iter()
        .filter(|view| view.source_id.as_ref().and_then(Option::as_deref) == Some(id))
        .collect()
}

fn diff(view: &TaskEventView) -> Value {
    let raw: Value = serde_json::from_str(view.raw_text.as_deref().unwrap()).unwrap();
    raw["content"][0].clone()
}

#[test]
fn a_new_file_shows_only_its_own_lines() {
    let views = views();
    let created = call(&views, "tool_ad012f98-7470-4805-abea-9ba7d27208f");
    let done = created.last().unwrap();

    let change = diff(done);
    assert!(change.get("oldText").is_none(), "{change}");
    assert!(
        change["newText"]
            .as_str()
            .unwrap()
            .starts_with("This module resolves which binary"),
        "{change}"
    );
    assert!(!done.raw_text.as_deref().unwrap().contains("/dev/null"));
    assert_eq!(
        done.presentation.as_ref().unwrap().change.as_deref(),
        Some("5 lines")
    );

    // Before the path arrives, the call is only "Edit File": no target yet.
    assert_eq!(created[0].target, None);
    assert_eq!(
        done.target.as_deref(),
        Some("/home/dev/desgn/oga/.oga_tmp/cursor-edit-test/notes.md")
    );
}

#[test]
fn an_edit_of_an_existing_file_says_how_many_lines_it_changed() {
    let views = views();
    let edit = call(&views, "tool_edadd85f-a225-4190-82bb-5fb5e5b43dc");
    let done = edit.last().unwrap();
    assert_eq!(done.phase, EventPhase::Completed);
    assert_eq!(
        done.presentation.as_ref().unwrap().change.as_deref(),
        Some("+3 \u{2212}2")
    );
    // The before and after are the file's own text, untouched.
    assert!(
        diff(done)["oldText"]
            .as_str()
            .unwrap()
            .starts_with("#!/usr/bin/env bash")
    );
}

#[test]
fn another_agents_new_file_is_left_as_it_was_sent() {
    let views = views_as(Provider::Claude);
    let created = call(&views, "tool_ad012f98-7470-4805-abea-9ba7d27208f");
    assert_eq!(diff(created.last().unwrap())["oldText"], "-- /dev/null");
}

#[test]
fn searches_say_how_many_they_found_and_where() {
    let views = views();
    let find = call(&views, "tool_7da32222-3a55-4584-b503-8a8430db1e0");
    let done = find.last().unwrap();
    assert_eq!(done.title, "Find files");
    assert_eq!(
        done.target.as_deref(),
        Some("**/* in /home/dev/desgn/oga/rust/crates/oga-routing")
    );
    assert_eq!(done.result.as_deref(), Some("12 files"));

    let grep = call(&views, "tool_d2691e05-beaf-450d-a689-1e44ef0d225");
    assert_eq!(grep.last().unwrap().result.as_deref(), Some("694+ matches"));
}

#[test]
fn a_ranged_read_keeps_a_clean_path_and_names_its_lines() {
    let views = views();
    let read = call(&views, "tool_7b75210d-f1b1-444c-8cb8-44a480f4b08");
    let done = read.last().unwrap();
    let presentation = done.presentation.as_ref().unwrap();
    assert_eq!(
        presentation.path.as_deref(),
        Some("/home/dev/desgn/oga/rust/crates/oga-routing/src/selection.rs")
    );
    assert_eq!(done.result.as_deref(), Some("lines 1\u{2013}200"));
}

#[test]
fn a_command_shows_what_it_printed_and_why_it_failed() {
    let views = views();
    let query = call(&views, "tool_abe46705-d5bc-4f8f-9d42-137efaf1092");
    let done = query.last().unwrap();
    assert_eq!(done.phase, EventPhase::Completed);
    assert!(
        done.result
            .as_deref()
            .unwrap()
            .starts_with("rust/crates/oga-routing/src/selection.rs:1282#NamedRouteAudit"),
        "{:?}",
        done.result
    );

    let relearn = call(&views, "tool_9285cf1f-bca7-47d1-9580-b6aa9a282ff");
    let failed = relearn.last().unwrap();
    assert_eq!(failed.phase, EventPhase::Failed);
    assert!(
        failed
            .result
            .as_deref()
            .unwrap()
            .starts_with("Exited with code 2 \u{b7} error: route 5"),
        "{:?}",
        failed.result
    );
}

#[test]
fn the_model_reads_without_its_settings() {
    let views = views();
    let spawned = views
        .iter()
        .find(|view| view.title == "Worker started" && view.detail.is_some())
        .unwrap();
    assert_eq!(spawned.detail.as_deref(), Some("Cursor Composer 2.5"));
}

/// Rewrites the rows the web tests read: `cargo test -p oga-events --test
/// cursor -- --ignored`.
#[test]
#[ignore = "rewrites web/src/domain/activity/fixtures/cursor.json"]
fn refresh_the_web_fixture() {
    let json = format!("{}\n", serde_json::to_string_pretty(&views()).unwrap());
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../web/src/domain/activity/fixtures/cursor.json");
    fs::write(output, json).unwrap();
}
