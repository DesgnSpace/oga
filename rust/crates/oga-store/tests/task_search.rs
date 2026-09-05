mod common;

use oga_domain::{Profile, Provider, Task, TaskListQuery, TaskMatch, TaskState};

use common::TestDatabase;

fn insert_profile(store: &oga_store::Store) {
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
}

fn insert_task(
    store: &oga_store::Store,
    id: &str,
    title: Option<&str>,
    tldr: Option<&str>,
    prompt: &str,
    updated_at: &str,
) {
    store
        .repositories()
        .tasks()
        .insert(&Task {
            id: id.into(),
            profile_id: "profile".into(),
            model: "model".into(),
            prompt: prompt.into(),
            cwd: "/project".into(),
            state: TaskState::Completed,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: updated_at.into(),
            ..Task::default()
        })
        .expect("task inserts");
    store
        .transaction(|connection| {
            connection.execute(
                "UPDATE tasks SET title=?, tldr=? WHERE id=?",
                rusqlite::params![title, tldr, id],
            )?;
            Ok(())
        })
        .expect("labels update");
}

#[test]
fn task_search_ranks_title_then_tldr_then_prompt_case_insensitively() {
    let database = TestDatabase::new();
    let store = database.open_writable();
    insert_profile(&store);
    insert_task(
        &store,
        "prompt",
        None,
        None,
        "Review the FILTER command implementation",
        "2026-01-03T00:00:00.000Z",
    );
    insert_task(
        &store,
        "tldr",
        None,
        Some("Filter the recent task list"),
        "Unrelated prompt",
        "2026-01-02T00:00:00.000Z",
    );
    insert_task(
        &store,
        "title",
        Some("Filter task history"),
        None,
        "Unrelated prompt",
        "2026-01-01T00:00:00.000Z",
    );

    let matches = store
        .repositories()
        .tasks()
        .search(&TaskListQuery {
            query: Some("fIlTeR".into()),
            ..TaskListQuery::default()
        })
        .expect("search succeeds");

    assert_eq!(
        matches
            .iter()
            .map(|task| (task.id.as_str(), task.matched))
            .collect::<Vec<_>>(),
        vec![
            ("title", TaskMatch::Title),
            ("tldr", TaskMatch::Tldr),
            ("prompt", TaskMatch::Prompt),
        ]
    );
}
