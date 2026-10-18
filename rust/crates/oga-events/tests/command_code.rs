use std::{fs, path::PathBuf};

use oga_domain::{EventPhase, Provider, TaskEvent, TaskEventView};
use serde_json::Value;

/// A real Command Code run, trimmed to the calls that read badly: its task
/// tools, a skill load, a multi-file read, searches, edits and shell runs.
fn views() -> Vec<TaskEventView> {
    views_as(Provider::CommandCode)
}

fn views_as(provider: Provider) -> Vec<TaskEventView> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/command-code/events.jsonl");
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

fn shell(views: &[TaskEventView], command: &str) -> TaskEventView {
    views
        .iter()
        .rfind(|view| {
            view.complete == Some(true)
                && view
                    .presentation
                    .as_ref()
                    .and_then(|presentation| presentation.command.as_deref())
                    .is_some_and(|run| run.starts_with(command))
        })
        .cloned()
        .unwrap_or_else(|| panic!("no finished shell call starting `{command}`"))
}

fn steps(view: &TaskEventView) -> Vec<(String, String)> {
    let raw: Value = serde_json::from_str(view.raw_text.as_deref().unwrap()).unwrap();
    raw["rawInput"]["todos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| {
            (
                step["content"].as_str().unwrap().to_owned(),
                step["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn the_task_tools_build_one_checklist_that_moves_on_with_each_call() {
    let views = views();
    let tasks = views
        .iter()
        .filter(|view| view.action_id.as_deref() == Some("task-list"))
        .collect::<Vec<_>>();

    // Eighteen calls, one row to fold them into.
    assert_eq!(
        tasks
            .iter()
            .filter_map(|view| view.source_id.clone().flatten())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        18
    );
    assert!(tasks.iter().all(|view| view.phase == EventPhase::Completed));

    let last = steps(tasks.last().unwrap());
    assert_eq!(last.len(), 7);
    assert_eq!(last[0].0, "Load refactor skill (naming mode)");
    assert!(last.iter().all(|(_, status)| status == "completed"));
    let presentation = tasks.last().unwrap().presentation.as_ref().unwrap();
    assert_eq!(presentation.total, Some(7));
    assert_eq!(presentation.completed, Some(7));
    assert_eq!(tasks.last().unwrap().result.as_deref(), Some("all done"));

    // Midway the list is the steps made so far, not the finished one.
    let progress = tasks
        .iter()
        .map(|view| {
            let presentation = view.presentation.as_ref().unwrap();
            (presentation.completed.unwrap(), presentation.total.unwrap())
        })
        .collect::<Vec<_>>();
    assert!(progress.contains(&(0, 1)), "the first step opens the list");
    assert!(
        progress
            .iter()
            .any(|(done, total)| *done == 3 && *total == 7)
    );
    assert!(progress.windows(2).all(|pair| pair[0].1 <= pair[1].1));
}

#[test]
fn a_run_that_prints_its_failure_reads_as_failed_and_a_clean_one_does_not() {
    let views = views();

    let failed = shell(&views, "cargo test -p oga-routing");
    assert_eq!(failed.phase, EventPhase::Failed);
    assert!(
        failed
            .result
            .as_deref()
            .unwrap()
            .starts_with("Failed \u{b7} test result: FAILED. 36 passed; 4 failed"),
        "{:?}",
        failed.result
    );

    // Warnings and passing tests are not failures.
    for command in ["cargo test -p oga-http", "cargo clippy", "cargo fmt"] {
        assert_eq!(
            shell(&views, command).phase,
            EventPhase::Completed,
            "{command}"
        );
    }
}

#[test]
fn another_agents_output_is_not_read_for_failure() {
    // Only Command Code lacks an exit status; for the rest, a line starting
    // with `error:` in a command's output (a `cat` of a log) is just output.
    let views = views_as(Provider::Claude);
    let run = shell(&views, "cargo test -p oga-routing");
    assert_eq!(run.phase, EventPhase::Completed);
    assert!(
        !run.result
            .as_deref()
            .unwrap_or_default()
            .starts_with("Failed")
    );
}

#[test]
fn a_skill_load_names_the_skill_and_what_it_was_asked() {
    let views = views();
    let load = call(&views, "call_07_bdsyqmou706hx1rj6db1fmq0");
    let done = load.last().unwrap();
    assert_eq!(done.title, "Load skill");
    assert_eq!(done.verb.as_deref(), Some("Loaded"));
    assert_eq!(done.target.as_deref(), Some("refactor \u{b7} naming"));
    assert_eq!(done.result, None);
}

#[test]
fn a_multi_file_read_names_its_files_and_a_search_names_what_it_looked_for() {
    let views = views();
    let read = call(&views, "call_02_d4ugyh87s1p27h2wmzsuvq6z");
    let target = read[0].target.as_deref().unwrap();
    assert_eq!(
        target,
        "4 files \u{b7} deslop.md, refactor.md, review.md +1 more"
    );

    let search = call(&views, "call_02_5tw6235wpxl4igj1oz4jkgpr");
    assert_eq!(search[0].verb.as_deref(), Some("Searching"));
    assert_eq!(search[0].target.as_deref(), Some("cursor_models in rust"));
}

#[test]
fn an_edit_does_not_say_its_path_twice() {
    let views = views();
    let edit = call(&views, "call_00_3lnx4uznpijlku7c724mhq5v");
    let done = edit.last().unwrap();
    assert_eq!(
        done.target.as_deref(),
        Some("/Users/malico/desgn/oga/rust/crates/oga-routing/src/catalog.rs")
    );
    assert_eq!(done.result.as_deref(), Some("1 replacement"));
}

/// Rewrites the rows the web tests read: `cargo test -p oga-events --test
/// command_code -- --ignored`.
#[test]
#[ignore = "rewrites web/src/domain/activity/fixtures/command-code.json"]
fn refresh_the_web_fixture() {
    let json = format!("{}\n", serde_json::to_string_pretty(&views()).unwrap());
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../web/src/domain/activity/fixtures/command-code.json");
    fs::write(output, json).unwrap();
}
