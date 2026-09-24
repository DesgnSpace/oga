use std::{collections::HashSet, fs, path::PathBuf};

use oga_domain::{Provider, SubagentRole, TaskEvent, TaskState};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredEvent {
    id: i64,
    event_type: String,
    state: TaskState,
    created_at: String,
    payload: serde_json::Map<String, serde_json::Value>,
}

impl From<StoredEvent> for TaskEvent {
    fn from(event: StoredEvent) -> Self {
        Self {
            id: event.id,
            task_id: "subagent-fixture".into(),
            kind: event.event_type,
            state: event.state,
            payload: event.payload.into_iter().collect(),
            created_at: event.created_at,
            turn_id: None,
        }
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn provider(driver: &str) -> Provider {
    match driver {
        "claude" => Provider::Claude,
        "codex" => Provider::Codex,
        "opencode" => Provider::OpenCode,
        "opencode2" => Provider::OpenCode2,
        "antigravity" => Provider::Antigravity,
        "pi" => Provider::Pi,
        "fx" => Provider::Fx,
        _ => unreachable!("unknown fixture driver: {driver}"),
    }
}

fn events(driver: &str) -> Vec<TaskEvent> {
    let path = root().join(format!(
        "rust/crates/oga-events/tests/fixtures/subagents/{driver}.jsonl"
    ));
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<StoredEvent>(line).unwrap().into())
        .collect()
}

#[test]
fn subagent_fixtures_are_normalized_for_the_web() {
    for driver in [
        "claude",
        "codex",
        "opencode",
        "opencode2",
        "antigravity",
        "pi",
        "fx",
    ] {
        let views = oga_events::event_views(&events(driver), provider(driver));
        let json = format!("{}\n", serde_json::to_string_pretty(&views).unwrap());
        let output = root().join(format!(
            "web/src/domain/activity/fixtures/subagents/{driver}.json"
        ));
        if std::env::var_os("UPDATE_SUBAGENT_FIXTURES").is_some() {
            fs::create_dir_all(output.parent().unwrap()).unwrap();
            fs::write(&output, &json).unwrap();
        } else {
            assert_eq!(
                fs::read_to_string(&output).unwrap(),
                json,
                "refresh with UPDATE_SUBAGENT_FIXTURES=1"
            );
        }

        let launches = views
            .iter()
            .filter(|view| {
                view.subagents
                    .iter()
                    .any(|link| link.role == SubagentRole::Launch)
            })
            .filter(|view| {
                driver != "fx"
                    || view
                        .subagents
                        .iter()
                        .any(|link| link.role == SubagentRole::Launch && link.report.is_some())
            })
            .filter_map(|view| view.action_id.as_ref())
            .collect::<HashSet<_>>();
        let expected = match driver {
            "antigravity" => 1,
            "pi" => 0,
            _ => 3,
        };
        assert_eq!(launches.len(), expected, "{driver} launch count");
        if driver == "fx" {
            let failed = views
                .iter()
                .filter(|view| view.phase == oga_domain::EventPhase::Failed)
                .filter(|view| {
                    view.subagents
                        .iter()
                        .any(|link| link.role == SubagentRole::Launch)
                })
                .filter_map(|view| view.action_id.as_ref())
                .collect::<HashSet<_>>();
            assert_eq!(failed.len(), 3, "fx failed launches stay marked");
        }

        if matches!(driver, "claude" | "opencode2") {
            let launch_ids = views
                .iter()
                .flat_map(|view| &view.subagents)
                .filter(|link| link.role == SubagentRole::Launch)
                .map(|link| &link.id)
                .collect::<HashSet<_>>();
            for member in views
                .iter()
                .flat_map(|view| &view.subagents)
                .filter(|link| link.role == SubagentRole::Member)
            {
                assert!(
                    launch_ids.contains(&member.id),
                    "{driver} member {} has no launch",
                    member.id
                );
            }
        }
    }
}
