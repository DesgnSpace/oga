//! Opening a database is what upgrades it, so that is what these exercise.
//!
//! The fixture is built at the shape v47 left behind — the code index without
//! a change-time column, learned routes with nowhere to record that a checkout
//! could not find one — and then handed to `Store::open_writable`, which is the
//! same call the broker makes at startup.

mod common;

use oga_store::{LATEST_SCHEMA_VERSION, Store};
use rusqlite::Connection;

use common::TestDatabase;

/// The tables these migrations touch, as v47 left them, with one indexed file,
/// one learned route, and one task already in place.
const V47: &str = r#"BEGIN IMMEDIATE;
    CREATE TABLE schema_migrations (
      version INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    CREATE TABLE context_files (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      cwd TEXT NOT NULL,
      path TEXT NOT NULL,
      lang TEXT NOT NULL,
      digest TEXT NOT NULL,
      size INTEGER NOT NULL,
      mtime_ms INTEGER NOT NULL,
      lines INTEGER NOT NULL,
      updated_at TEXT NOT NULL
    );
    CREATE TABLE context_learned_routes (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      cwd TEXT NOT NULL,
      aliases TEXT NOT NULL,
      hint_keys TEXT NOT NULL,
      learned_path TEXT NOT NULL,
      learned_symbol TEXT NOT NULL,
      source_digest TEXT NOT NULL,
      task_id TEXT NOT NULL,
      attempt INTEGER NOT NULL,
      profile_id TEXT NOT NULL,
      model TEXT NOT NULL,
      created_at TEXT NOT NULL,
      last_confirmed_at TEXT NOT NULL,
      UNIQUE(cwd, learned_path, learned_symbol)
    );
    INSERT INTO context_files(cwd,path,lang,digest,size,mtime_ms,lines,updated_at)
      VALUES('/project','src/billing.ts','typescript','a440f4bdacabb860',64,1789417052088,3,'2026-01-01T00:00:00.000Z');
    INSERT INTO context_learned_routes(cwd,aliases,hint_keys,learned_path,learned_symbol,
      source_digest,task_id,attempt,profile_id,model,created_at,last_confirmed_at)
      VALUES('/project','tak money charg card','card charg money tak','src/billing.ts','chargeCard',
      'faa7ad94b913774e','',0,'user','','2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z');
    CREATE TABLE profiles (
      id TEXT PRIMARY KEY,
      label TEXT NOT NULL,
      provider TEXT NOT NULL CHECK(provider IN ('claude','codex','opencode','opencode-2','antigravity','pi')),
      default_model TEXT NOT NULL,
      enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
      env_json TEXT NOT NULL CHECK(json_valid(env_json)),
      capabilities_json TEXT NOT NULL CHECK(json_valid(capabilities_json)),
      command_json TEXT CHECK(command_json IS NULL OR json_valid(command_json)),
      created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
      updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
      deleted_at TEXT
    );
    INSERT INTO profiles(id,label,provider,default_model,enabled,env_json,capabilities_json,created_at,updated_at)
      VALUES('claude','Claude','claude','sonnet',1,'{}','[]','2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z');
    CREATE TABLE tasks (
      id TEXT PRIMARY KEY,
      state TEXT NOT NULL,
      session_id TEXT
    );
    INSERT INTO tasks(id,state,session_id) VALUES('before-acp','completed','claude-session-1');
    INSERT INTO schema_migrations(version, name) VALUES (47, 'project-scoped code search');
    COMMIT;"#;

fn write_v47(path: &std::path::Path) {
    let connection = Connection::open(path).expect("fixture database opens");
    connection.execute_batch(V47).expect("v47 fixture writes");
}

fn columns(store: &Store, table: &str) -> Vec<String> {
    store
        .with_connection(|connection| {
            let mut statement =
                connection.prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))?;
            Ok(statement
                .query_map([], |row| row.get(0))?
                .collect::<Result<Vec<String>, _>>()?)
        })
        .expect("columns read")
}

fn version(store: &Store) -> i64 {
    store
        .with_connection(|connection| {
            Ok(
                connection.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .expect("version reads")
}

#[test]
fn a_provider_the_old_check_refused_is_addable_after_the_upgrade() {
    let database = TestDatabase::new();
    write_v47(&database.path());

    let store = database.open_writable();

    store
        .with_transaction(|transaction| {
            Ok(transaction.execute(
                "INSERT INTO profiles(id,label,provider,default_model,enabled,env_json,capabilities_json,created_at,updated_at) \
                 VALUES('fx','fx','fx','openai/gpt-5.2',1,'{}','[]','2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z')",
                [],
            )?)
        })
        .expect("a provider named only in code is addable");

    let kept: String = store
        .with_connection(|connection| {
            Ok(connection.query_row("SELECT label FROM profiles WHERE id='claude'", [], |row| {
                row.get(0)
            })?)
        })
        .expect("the profile from before the upgrade reads");
    assert_eq!(kept, "Claude");
}

#[test]
fn a_fresh_database_opens_at_the_current_schema() {
    let database = TestDatabase::new();
    let store = database.open_writable();

    assert_eq!(version(&store), LATEST_SCHEMA_VERSION);
    assert!(columns(&store, "context_files").contains(&"ctime_ms".to_owned()));
    assert!(
        columns(&store, "context_learned_routes").contains(&"missing_since".to_owned()),
        "a fresh install must not need a migration to reach the current shape"
    );
}

#[test]
fn opening_a_v47_database_upgrades_it_and_keeps_what_was_taught() {
    let database = TestDatabase::new();
    write_v47(&database.path());

    let store = database.open_writable();

    assert_eq!(version(&store), LATEST_SCHEMA_VERSION);
    assert!(columns(&store, "context_files").contains(&"ctime_ms".to_owned()));
    assert!(columns(&store, "context_learned_routes").contains(&"missing_since".to_owned()));

    let ctime: i64 = store
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT ctime_ms FROM context_files WHERE path='src/billing.ts'",
                [],
                |row| row.get(0),
            )?)
        })
        .expect("indexed file reads");
    // A file indexed before the upgrade has no change time on record, and zero
    // is what makes it compare unequal to a real one and be read again.
    assert_eq!(ctime, 0);

    let route = store
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT aliases, learned_path, learned_symbol, source_digest, missing_since \
                 FROM context_learned_routes",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )?)
        })
        .expect("learned route reads");
    assert_eq!(
        route,
        (
            "tak money charg card".to_owned(),
            "src/billing.ts".to_owned(),
            "chargeCard".to_owned(),
            "faa7ad94b913774e".to_owned(),
            None,
        ),
        "an upgraded route keeps every word it was taught, and starts resolved"
    );
}

#[test]
fn a_database_that_already_has_the_columns_upgrades_anyway() {
    // The shape a database coming up through the v42 rebuild arrives in: the
    // tables carry the current columns while the ledger has not reached 48.
    let database = TestDatabase::new();
    write_v47(&database.path());
    drop(database.open_writable());
    let store = database.open_writable();
    store
        .transaction(|transaction| {
            Ok(transaction.execute("DELETE FROM schema_migrations WHERE version=48", [])?)
        })
        .expect("ledger rewinds");
    drop(store);

    let store = database.open_writable();

    assert_eq!(version(&store), LATEST_SCHEMA_VERSION);
    let routes: i64 = store
        .with_connection(|connection| {
            Ok(
                connection.query_row("SELECT COUNT(*) FROM context_learned_routes", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .expect("routes counted");
    assert_eq!(routes, 1);
}

#[test]
fn reopening_an_upgraded_database_changes_nothing() {
    let database = TestDatabase::new();
    write_v47(&database.path());
    drop(database.open_writable());

    let store = database.open_writable();

    assert_eq!(version(&store), LATEST_SCHEMA_VERSION);
    let recorded: i64 = store
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version=?",
                [LATEST_SCHEMA_VERSION],
                |row| row.get(0),
            )?)
        })
        .expect("ledger counted");
    assert_eq!(
        recorded, 1,
        "a migration must be recorded once, not per open"
    );
}

#[test]
fn a_task_from_before_acp_stays_on_the_command_line() {
    let database = TestDatabase::new();
    write_v47(&database.path());

    let store = database.open_writable();

    let (session, transport): (Option<String>, serde_json::Value) = store
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT session_id, transport_json FROM tasks WHERE id='before-acp'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        serde_json::from_str(&row.get::<_, String>(1)?).expect("transport JSON"),
                    ))
                },
            )?)
        })
        .expect("task reads");
    assert_eq!(session.as_deref(), Some("claude-session-1"));
    assert_eq!(transport["kind"], "cli");
    assert_eq!(transport["reason"], "legacy");
    assert!(transport.get("acpSessionId").is_none());
}
