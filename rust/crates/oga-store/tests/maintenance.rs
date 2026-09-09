mod common;

use oga_domain::{Profile, Provider, Task, TaskEvent, TaskState};
use serde_json::json;

use common::TestDatabase;

fn task(
    id: &str,
    state: TaskState,
    archived_at: Option<&str>,
    parent_task_id: Option<&str>,
) -> Task {
    Task {
        id: id.to_owned(),
        profile_id: "profile".to_owned(),
        state,
        archived_at: archived_at.map(str::to_owned),
        parent_task_id: parent_task_id.map(str::to_owned),
        created_at: "2025-01-01T00:00:00Z".to_owned(),
        updated_at: "2025-01-02T00:00:00Z".to_owned(),
        ..Task::default()
    }
}

fn insert_profile(store: &oga_store::Store) {
    store
        .repositories()
        .profiles()
        .insert(
            &Profile {
                id: "profile".to_owned(),
                label: "Test profile".to_owned(),
                provider: Provider::Claude,
                default_model: "sonnet".to_owned(),
                enabled: true,
                env: Default::default(),
                capabilities: Vec::new(),
                command: None,
            },
            "2025-01-01T00:00:00Z",
        )
        .expect("profile inserts");
}

fn event(task_id: &str, state: TaskState, created_at: &str) -> TaskEvent {
    TaskEvent {
        id: 0,
        task_id: task_id.to_owned(),
        kind: "agent.message".to_owned(),
        state,
        payload: [("text".to_owned(), json!("old activity"))]
            .into_iter()
            .collect(),
        created_at: created_at.to_owned(),
        turn_id: None,
    }
}

#[test]
fn cleanup_keeps_task_and_leaves_history_marker() {
    let database = TestDatabase::new();
    let store = database.open_writable();
    insert_profile(&store);
    store
        .repositories()
        .tasks()
        .insert(&task(
            "old",
            TaskState::Completed,
            Some("2025-01-03T00:00:00Z"),
            None,
        ))
        .expect("task inserts");
    store
        .repositories()
        .events()
        .append(&event("old", TaskState::Completed, "2025-01-02T00:00:00Z"))
        .expect("event inserts");

    let result = store
        .cleanup("2025-02-01T00:00:00Z", true, "2026-01-01T00:00:00Z")
        .expect("cleanup succeeds");

    assert_eq!(result.record.plan.tasks, 1);
    assert_eq!(result.record.plan.events, 1);
    assert!(
        store
            .repositories()
            .tasks()
            .get("old")
            .expect("task reads")
            .is_some()
    );
    let events = store
        .repositories()
        .events()
        .list("old")
        .expect("events read");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "history_dropped");
    assert_eq!(events[0].payload["dropped"], json!(1));
}

#[test]
fn cleanup_holds_back_active_children_and_viewed_tasks() {
    let database = TestDatabase::new();
    let store = database.open_writable();
    insert_profile(&store);
    let archived = Some("2025-01-03T00:00:00Z");
    for task in [
        task("viewed", TaskState::Completed, archived, None),
        task("parent", TaskState::Completed, archived, None),
        task("plain", TaskState::Completed, archived, None),
        task("child", TaskState::Running, archived, Some("parent")),
    ] {
        store
            .repositories()
            .tasks()
            .insert(&task)
            .expect("task inserts");
        store
            .repositories()
            .events()
            .append(&event(&task.id, task.state, "2025-01-02T00:00:00Z"))
            .expect("event inserts");
    }
    store.set_viewed_task("viewed");

    let result = store
        .cleanup("2025-02-01T00:00:00Z", true, "2026-01-01T00:00:00Z")
        .expect("cleanup succeeds");

    assert_eq!(result.record.plan.tasks, 1);
    assert_eq!(result.record.plan.events, 1);
    assert_eq!(result.record.plan.held_back, 1);
    assert_eq!(
        store
            .repositories()
            .events()
            .list("plain")
            .expect("plain events read")
            .len(),
        1
    );
    assert_eq!(
        store
            .repositories()
            .events()
            .list("viewed")
            .expect("viewed events read")
            .len(),
        1
    );
    assert_eq!(
        store
            .repositories()
            .events()
            .list("parent")
            .expect("parent events read")
            .len(),
        1
    );
    assert_eq!(
        store
            .repositories()
            .events()
            .list("child")
            .expect("child events read")
            .len(),
        1
    );
}
