mod common;

use oga_domain::{Profile, Provider, Task, TaskState, TaskTurnStatus};

use common::TestDatabase;

#[test]
fn sums_worker_runs_without_counting_the_wait_between_them() {
    let database = TestDatabase::new();
    let store = database.open_writable();
    store
        .repositories()
        .profiles()
        .insert(
            &Profile {
                id: "profile".into(),
                label: "Profile".into(),
                provider: Provider::Claude,
                default_model: "model".into(),
                enabled: true,
                env: Default::default(),
                capabilities: Vec::new(),
                command: None,
            },
            "2026-01-01T00:00:00.000Z",
        )
        .expect("profile inserts");
    store
        .repositories()
        .tasks()
        .insert(&Task {
            id: "task".into(),
            profile_id: "profile".into(),
            model: "model".into(),
            prompt: "work".into(),
            cwd: "/project".into(),
            state: TaskState::Completed,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:45.000Z".into(),
            ..Task::default()
        })
        .expect("task inserts");
    let first = store
        .repositories()
        .turns()
        .start("task", 1, "2026-01-01T00:00:30.000Z")
        .expect("first turn starts");
    store
        .repositories()
        .turns()
        .finish(first, TaskTurnStatus::Completed, "2026-01-01T00:00:45.000Z")
        .expect("first turn finishes");
    let task = store
        .repositories()
        .tasks()
        .get("task")
        .expect("task reads")
        .expect("task exists");
    assert_eq!(task.duration_ms, 15_000);
    let second = store
        .repositories()
        .turns()
        .start("task", 2, "2026-01-01T00:01:00.000Z")
        .expect("second turn starts");
    store
        .repositories()
        .turns()
        .finish(
            second,
            TaskTurnStatus::Completed,
            "2026-01-01T00:01:15.000Z",
        )
        .expect("second turn finishes");

    let task = store
        .repositories()
        .tasks()
        .get("task")
        .expect("task reads")
        .expect("task exists");
    assert_eq!(task.duration_ms, 30_000);
    assert_eq!(task.running_since, None);

    store
        .repositories()
        .turns()
        .start("task", 3, "2026-01-01T00:01:00.000Z")
        .expect("active turn starts");
    let task = store
        .repositories()
        .tasks()
        .get("task")
        .expect("task reads")
        .expect("task exists");
    assert_eq!(task.duration_ms, 30_000);
    assert_eq!(
        task.running_since.as_deref(),
        Some("2026-01-01T00:01:00.000Z")
    );
}
