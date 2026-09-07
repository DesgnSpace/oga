use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

use oga_domain::{Profile, Provider, Task, TaskEvent, TaskKind, TaskScope, TaskState};
use oga_events::{EventFeed, MAX_TRACKED_EVENTS};
use oga_store::Store;
use rusqlite::params;
use tempfile::TempDir;

const EVENT_COUNT: i64 = 10_000;

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
                allow_questions: true,
                ..Task::default()
            })
            .expect("task");
        Self {
            _directory: directory,
            store,
        }
    }

    fn append_events(&self) {
        self.store
            .with_transaction(|transaction| {
                for _ in 0..EVENT_COUNT {
                    transaction.execute(
                        "INSERT INTO task_events(task_id,event_type,state,payload,created_at,turn_id) VALUES(?,?,?,?,?,?)",
                        params![
                            "task",
                            "progress",
                            "running",
                            "{}",
                            "2026-01-01T00:00:01.000Z",
                            None::<String>,
                        ],
                    )?;
                }
                Ok(())
            })
            .expect("events");
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

#[tokio::test]
#[ignore]
async fn measured_shared_feed_fixture() {
    let fixture = Fixture::new();
    let feed = EventFeed::with_poll_interval(fixture.store.clone(), Duration::from_millis(1));
    let _guard = feed.retain().expect("feed");
    let started = Instant::now();
    fixture.append_events();

    assert!(
        tokio::time::timeout(
            Duration::from_secs(5),
            feed.wait_for_change(0, &[String::from("task")], Duration::from_secs(5)),
        )
        .await
        .expect("feed notification")
        .expect("feed read")
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while feed.stats().observed_events < EVENT_COUNT as u64 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("feed drained the fixture");

    let elapsed = started.elapsed().as_secs_f64();
    let stats = feed.stats();
    println!(
        "events={} database_queries={} poll_queries={} notifications={} retained_events={} retained_capacity={} elapsed_ms={:.2} throughput_events_per_sec={:.0}",
        EVENT_COUNT,
        stats.database_queries,
        stats.poll_queries,
        stats.notifications,
        stats.retained_events,
        MAX_TRACKED_EVENTS,
        elapsed * 1_000.0,
        EVENT_COUNT as f64 / elapsed,
    );
    assert_eq!(stats.observed_events, EVENT_COUNT as u64);
    assert!(stats.retained_events <= MAX_TRACKED_EVENTS);
}
