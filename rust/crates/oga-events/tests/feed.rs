use std::{
    collections::BTreeMap,
    sync::Arc,
    time::Duration,
};

use oga_domain::{Profile, Provider, Task, TaskEvent, TaskKind, TaskScope, TaskState};
use oga_events::EventFeed;
use oga_store::Store;
use tempfile::TempDir;

struct Fixture {
    _directory: TempDir,
    store: Arc<Store>,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        store
            .repositories()
            .profiles()
            .insert(
                &Profile {
                    id: "profile".into(),
                    label: "Fixture".into(),
                    provider: Provider::Claude,
                    default_model: "fake".into(),
                    enabled: true,
                    env: BTreeMap::new(),
                    capabilities: Vec::new(),
                    command: None,
                },
                "2026-01-01T00:00:00.000Z",
            )
            .expect("profile");
        store
            .repositories()
            .tasks()
            .insert(&Task {
                id: "task".into(),
                kind: Some(TaskKind::Delegated),
                profile_id: "profile".into(),
                model: "fake".into(),
                prompt: "measure event delivery".into(),
                cwd: directory.path().display().to_string(),
                state: TaskState::Running,
                created_at: "2026-01-01T00:00:00.000Z".into(),
                updated_at: "2026-01-01T00:00:00.000Z".into(),
                output: String::new(),
                scope: TaskScope {
                    read: vec!["**".into()],
                    write: vec!["**".into()],
                },
                ..Task::default()
            })
            .expect("task");
        Self {
            _directory: directory,
            store,
        }
    }

}

#[tokio::test]
async fn feed_reports_existing_task_events_after_startup() {
    let fixture = Fixture::new();
    fixture
        .store
        .repositories()
        .events()
        .append(&TaskEvent {
            id: 0,
            task_id: "task".into(),
            kind: "progress".into(),
            state: TaskState::Running,
            payload: BTreeMap::new(),
            created_at: "2026-01-01T00:00:01.000Z".into(),
            turn_id: None,
        })
        .expect("event");
    let feed = EventFeed::with_poll_interval(fixture.store, Duration::from_millis(1));

    assert!(
        feed.wait_for_change(0, &[String::from("task")], Duration::from_millis(20),)
            .await
            .expect("feed read")
    );
}

