mod common;

use std::fs;

use oga_store::{LATEST_SCHEMA_VERSION, Store, StoreError};
use rusqlite::Connection;

use common::TestDatabase;

fn refusals(store: Result<Store, StoreError>) -> String {
    match store {
        Ok(_) => panic!("observe open must refuse"),
        Err(StoreError::Refusal(message)) => message,
        Err(other) => panic!("expected a refusal, got {other:?}"),
    }
}

/// A plain writable connection for mutating the fixture outside the store
/// API — simulating what another binary would have done to the file.
fn raw_connection(path: &std::path::Path) -> Connection {
    let connection = Connection::open(path).expect("raw connection opens");
    connection.execute_batch("PRAGMA busy_timeout = 5000").ok();
    connection
}

#[test]
fn observe_mode_refuses_a_missing_database() {
    let database = TestDatabase::new();
    let message = refusals(Store::open_observe(database.path()));
    assert_eq!(
        message,
        format!(
            "cannot observe {}: no database at this path; run the broker once to create it, or fix OGA_DB",
            database.path().display()
        )
    );
    // Refusing must not create anything either.
    assert!(
        !database.path().exists(),
        "observe must not create the file"
    );
}

#[test]
fn observe_mode_refuses_a_non_oga_database() {
    let database = TestDatabase::new();
    raw_connection(&database.path())
        .execute("CREATE TABLE other (name TEXT)", [])
        .expect("foreign table creates");
    let message = refusals(Store::open_observe(database.path()));
    assert_eq!(
        message,
        format!(
            "cannot observe {}: not an oga database (no schema_migrations table)",
            database.path().display()
        )
    );
}

#[test]
fn maintenance_mode_refuses_a_missing_database() {
    let database = TestDatabase::new();
    let message = refusals(Store::open_maintenance(database.path()));
    assert_eq!(
        message,
        format!(
            "no database at {}; run the broker once to create it, or fix OGA_DB",
            database.path().display()
        )
    );
    assert!(
        !database.path().exists(),
        "maintenance must not create the file"
    );
}

#[test]
fn maintenance_mode_writes_without_startup_side_effects() {
    let database = TestDatabase::new();
    database.open_writable().close().expect("schema creates");
    let maintenance = Store::open_maintenance(database.path()).expect("maintenance opens");
    maintenance
        .with_transaction(|connection| {
            connection.execute(
                "INSERT INTO settings(key, value) VALUES ('maintenance', 'true')",
                [],
            )?;
            Ok(())
        })
        .expect("maintenance writes");
    maintenance.close().expect("maintenance closes");
    let observer = Store::open_observe(database.path()).expect("observer opens");
    let value: String = observer
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT value FROM settings WHERE key = 'maintenance'",
                [],
                |row| row.get(0),
            )?)
        })
        .expect("maintenance value reads");
    observer.close().expect("observer closes");
    assert_eq!(value, "true");
}

#[test]
fn observe_mode_refuses_a_newer_schema() {
    let database = TestDatabase::new();
    database.open_writable().close().expect("store closes");
    let raw = raw_connection(&database.path());
    raw.execute(
        "INSERT INTO schema_migrations(version, name) VALUES (?1, 'from the future')",
        [LATEST_SCHEMA_VERSION + 1],
    )
    .expect("future row inserts");
    drop(raw);
    let message = refusals(Store::open_observe(database.path()));
    assert!(
        message.contains(&format!(
            "database schema v{} is newer than this binary knows",
            LATEST_SCHEMA_VERSION + 1
        )),
        "unexpected refusal: {message}"
    );
    assert!(
        message.contains("`make install`"),
        "unexpected refusal: {message}"
    );
}

#[test]
fn observe_mode_refuses_an_older_schema() {
    let database = TestDatabase::new();
    database.open_writable().close().expect("store closes");
    let raw = raw_connection(&database.path());
    raw.execute(
        "DELETE FROM schema_migrations WHERE version = ?1",
        [LATEST_SCHEMA_VERSION],
    )
    .expect("current row deletes");
    drop(raw);
    let message = refusals(Store::open_observe(database.path()));
    assert!(
        message.contains(&format!("database schema v0 predates this binary")),
        "unexpected refusal: {message}"
    );
}

#[test]
fn observe_mode_reads_but_never_writes() {
    let database = TestDatabase::new();
    {
        let store = database.open_writable();
        store
            .with_transaction(|connection| {
                connection.execute(
                    "INSERT INTO profiles(id, label, provider, default_model, enabled, env_json, capabilities_json)
                     VALUES ('p1', 'Worker', 'claude', 'sonnet', 1, '{}', '{}')",
                    [],
                )?;
                connection.execute(
                    "INSERT INTO tasks(id, profile_id, model, prompt, cwd, state, created_at, updated_at) VALUES
                     ('t1', 'p1', 'm', 'p', '/x', 'running', '2026-01-01', '2026-01-01')",
                    [],
                )?;
                Ok(())
            })
            .expect("seed inserts run");
        store.close().expect("store closes");
    }

    let before = fs::read(database.path()).expect("database file reads");
    let observer = database.open_observe();
    let observed: i64 = observer
        .with_connection(|connection| {
            // WAL databases are readable from a read-only handle.
            assert_eq!(
                connection
                    .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
                    .expect("journal mode reads"),
                "wal"
            );
            Ok(connection.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))?)
        })
        .expect("count reads");
    observer.close().expect("observer closes");
    assert_eq!(observed, 1);

    let after = fs::read(database.path()).expect("database file reads");
    assert_eq!(before, after, "observing must not modify the file");
}

#[test]
fn observe_mode_fails_writes_loudly() {
    let database = TestDatabase::new();
    database.open_writable().close().expect("store closes");
    let observer = database.open_observe();
    let error = observer
        .with_connection::<()>(|connection| {
            connection.execute("INSERT INTO settings(key, value) VALUES ('k', 'v')", [])?;
            Ok(())
        })
        .expect_err("writes on an observe handle must fail");
    match error {
        StoreError::Sqlite(rusqlite::Error::SqliteFailure(failure, _)) => {
            assert_eq!(failure.code, rusqlite::ErrorCode::ReadOnly);
        }
        other => panic!("expected a read-only failure, got {other:?}"),
    }
}
