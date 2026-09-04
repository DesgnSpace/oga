//! The version-37 database shape.
//!
//! [`create_fresh_schema`] produces the current database shape.

use rusqlite::Connection;

use crate::connection::StoreError;

/// The schema this binary can read.
pub const LATEST_SCHEMA_VERSION: i64 = 39;

/// Create the current schema on an empty database, in one transaction.
///
/// The schema is assembled transactionally so a partially created database is
/// never exposed to the broker.
pub fn create_fresh_schema(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    match create_fresh_schema_inner(conn) {
        Ok(()) => Ok(conn.execute_batch("COMMIT")?),
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn create_fresh_schema_inner(conn: &Connection) -> Result<(), StoreError> {
    let schema = [BASE_SCHEMA, CONTEXT_ENTITIES, ROUTE_HINTS_TABLE].concat();
    exec(conn, &schema)
}

fn exec(conn: &Connection, sql: &str) -> Result<(), StoreError> {
    Ok(conn.execute_batch(sql)?)
}

/// The founding tables, indexes, and ledger row 1.
const BASE_SCHEMA: &str = r#"    CREATE TABLE IF NOT EXISTS schema_migrations (
      version INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      applied_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE TABLE IF NOT EXISTS settings (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS profiles (
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
    CREATE TABLE IF NOT EXISTS tasks (
      id TEXT PRIMARY KEY,
      kind TEXT NOT NULL DEFAULT 'delegated' CHECK(kind IN ('delegated','orchestrator')),
      profile_id TEXT NOT NULL REFERENCES profiles(id),
      model TEXT NOT NULL,
       prompt TEXT NOT NULL,
       cwd TEXT NOT NULL,
       branch TEXT,
       state TEXT NOT NULL CHECK(state IN (
        'queued','pending','running','needs_input','answered','blocked','completed','failed','cancelled'
      )),
      output TEXT NOT NULL DEFAULT '',
      error TEXT,
      question TEXT,
      parent_task_id TEXT REFERENCES tasks(id),
      orchestrator_id TEXT,
      scope_json TEXT NOT NULL DEFAULT '{"read":["**"],"write":["**"]}' CHECK(json_valid(scope_json)),
      grant_id TEXT,
      allow_questions INTEGER NOT NULL DEFAULT 1 CHECK(allow_questions IN (0,1)),
      timeout_ms INTEGER,
      session_id TEXT,
      shipped_prompt TEXT,
      completion_json TEXT CHECK(completion_json IS NULL OR json_valid(completion_json)),
      attempts_json TEXT CHECK(attempts_json IS NULL OR json_valid(attempts_json)),
      cost_usd REAL,
      turns INTEGER,
      tokens_in INTEGER,
      tokens_out INTEGER,
      spend_at TEXT,
       archived_at TEXT,
       created_at TEXT NOT NULL,
       updated_at TEXT NOT NULL,
       effort TEXT,
       tldr TEXT,
       title TEXT,
       worker_json TEXT,
       selection_json TEXT,
       effort_actual TEXT,
       origin_cwd TEXT,
       worktree_path TEXT,
       worktree_branch TEXT,
       worktree_links_json TEXT,
       caller_id TEXT,
       cost_usd_estimated INTEGER
    );
    CREATE INDEX IF NOT EXISTS tasks_parent ON tasks(parent_task_id);
    CREATE INDEX IF NOT EXISTS tasks_updated_at ON tasks(updated_at DESC, id DESC);
    CREATE INDEX IF NOT EXISTS tasks_profile_updated ON tasks(profile_id, updated_at DESC);
    CREATE TABLE IF NOT EXISTS task_events (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
      event_type TEXT NOT NULL,
      state TEXT NOT NULL,
      payload TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(payload)),
       created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
       turn_id INTEGER REFERENCES task_turns(id)
    );
    CREATE INDEX IF NOT EXISTS task_events_task_id ON task_events(task_id, id);
    -- One round of work: a dispatch, a resume, a delivered follow-up, or a
    -- restarting handoff. Minted only where the broker actually spawns a
    -- worker process, never inferred from event contents.
    CREATE TABLE IF NOT EXISTS task_turns (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
      ordinal INTEGER NOT NULL,
      status TEXT NOT NULL CHECK(status IN (
        'running','completed','failed','needs_input','blocked','cancelled','interrupted'
      )),
      started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
      ended_at TEXT
    );
    CREATE INDEX IF NOT EXISTS task_turns_task_id ON task_turns(task_id, ordinal);
    CREATE TABLE IF NOT EXISTS profile_failures (
      profile_id TEXT NOT NULL REFERENCES profiles(id),
      code TEXT NOT NULL CHECK(code IN ('auth','billing','rate_limit','network')),
      message TEXT NOT NULL,
      failed_at TEXT NOT NULL,
      consecutive_failures INTEGER NOT NULL,
      retry_at TEXT,
      -- '' is the account-wide row (auth, billing, network); a named model is
      -- its own rate-limit row, since a provider metering models separately can
      -- have several live at once and one row could only ever hold the latest.
      model TEXT NOT NULL DEFAULT '',
      PRIMARY KEY(profile_id, model)
    );
    CREATE TABLE IF NOT EXISTS memories (
      cwd TEXT NOT NULL,
      key TEXT NOT NULL,
      value TEXT NOT NULL,
      version INTEGER NOT NULL DEFAULT 1 CHECK(version > 0),
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL,
      PRIMARY KEY(cwd, key)
    );
    CREATE TABLE IF NOT EXISTS scope_grants (
      id TEXT PRIMARY KEY,
      cwd TEXT NOT NULL,
      profile_id TEXT,
      scope_json TEXT NOT NULL CHECK(json_valid(scope_json)),
      created_at TEXT NOT NULL,
      last_used_at TEXT NOT NULL,
      use_count INTEGER NOT NULL DEFAULT 1
    );
    CREATE INDEX IF NOT EXISTS scope_grants_cwd ON scope_grants(cwd, last_used_at DESC);
    CREATE TABLE IF NOT EXISTS context_maps (
      cwd TEXT PRIMARY KEY,
      scheme INTEGER NOT NULL,
      state TEXT NOT NULL CHECK(state IN ('building','ready','partial')),
      built_at TEXT,
      file_count INTEGER NOT NULL DEFAULT 0,
      symbol_count INTEGER NOT NULL DEFAULT 0,
      pending_prose INTEGER NOT NULL DEFAULT 0,
       updated_at TEXT NOT NULL,
       search_state TEXT,
       search_indexed_at TEXT,
       search_indexed_map_updated_at TEXT,
       search_indexed_file_count INTEGER NOT NULL DEFAULT 0,
       search_indexed_symbol_count INTEGER NOT NULL DEFAULT 0,
       search_last_error TEXT
    );
    CREATE TABLE IF NOT EXISTS task_holds (
      task_id TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
      verb TEXT NOT NULL CHECK(verb IN ('resume','delegate')),
      args_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(args_json)),
      start_at TEXT,
      await_profile TEXT,
      await_model TEXT,
      next_check_at TEXT NOT NULL,
      expires_at TEXT NOT NULL,
      probe_count INTEGER NOT NULL DEFAULT 0,
      note TEXT NOT NULL,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS task_holds_due ON task_holds(next_check_at);
    CREATE TABLE IF NOT EXISTS task_follow_ups (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
      instruction TEXT NOT NULL,
      created_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS task_follow_ups_task ON task_follow_ups(task_id, id);
    CREATE TABLE IF NOT EXISTS task_dependencies (
      task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
      blocker_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
      created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
      PRIMARY KEY(task_id, blocker_id)
    );
    CREATE INDEX IF NOT EXISTS task_dependencies_blocker ON task_dependencies(blocker_id);
    CREATE TABLE IF NOT EXISTS consumer_cursors (
      consumer_id TEXT PRIMARY KEY,
      cursor INTEGER NOT NULL DEFAULT 0 CHECK(cursor >= 0),
      updated_at TEXT NOT NULL,
      scope_cwd TEXT
    );
    CREATE TABLE IF NOT EXISTS deliveries (
      consumer_id TEXT NOT NULL REFERENCES consumer_cursors(consumer_id) ON DELETE CASCADE,
      event_id INTEGER NOT NULL REFERENCES task_events(id) ON DELETE CASCADE,
      channel TEXT NOT NULL,
      status TEXT NOT NULL CHECK(status IN ('pending','sent','seen','expired')),
      attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
      last_error TEXT,
      sent_at TEXT,
      seen_at TEXT,
      PRIMARY KEY(consumer_id, event_id, channel)
    );
    CREATE INDEX IF NOT EXISTS deliveries_unseen
      ON deliveries(consumer_id, channel, status, event_id);
    -- Preferences a person sets, scoped to one directory. The global scope is
    -- the home directory's own row, so it is an ordinary row rather than a
    -- sentinel, and one key per concern keeps the table generic: 'models'
    -- carries per-worker model enablement, 'prompts' the worker prompt text.
    CREATE TABLE IF NOT EXISTS cwd_settings (
      cwd TEXT NOT NULL,
      key TEXT NOT NULL,
      value TEXT NOT NULL CHECK(json_valid(value)),
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL,
      PRIMARY KEY(cwd, key)
    );
     INSERT INTO schema_migrations(version, name) VALUES (38, 'user learned routes');"#;

/// One row per file or symbol, with its derived search text written in the same statement.
const CONTEXT_ENTITIES: &str = r#"      CREATE TABLE context_entities (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        cwd TEXT NOT NULL,
        kind TEXT NOT NULL CHECK(kind IN ('file','fn','class','type','const','struct','enum','ext','view')),
        parent_id INTEGER REFERENCES context_entities(id) ON DELETE CASCADE,
        path TEXT NOT NULL,
        line INTEGER NOT NULL DEFAULT 1,
        end_line INTEGER NOT NULL DEFAULT 1,
        name TEXT NOT NULL,
        -- File rows: whole-file content hash. Symbol rows: hash of the symbol's
        -- own declaration text (line..end_line) — the signal move-detection
        -- matches on, independent of which file currently holds the symbol.
        digest TEXT NOT NULL,
        purpose TEXT,
        confirmed INTEGER CHECK(confirmed IS NULL OR confirmed IN (0,1)),
        params TEXT,
        returns TEXT,
        exported INTEGER CHECK(exported IS NULL OR exported IN (0,1)),
        comments_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(comments_json)),
        lang TEXT,
        status TEXT CHECK(status IS NULL OR status IN ('mapped','unparsed')),
        lines INTEGER,
        size INTEGER,
        mtime_ms INTEGER,
        touch_count INTEGER NOT NULL DEFAULT 0,
        touched_at TEXT,
        mapped_at TEXT,
        header_comment TEXT NOT NULL DEFAULT '',
        refs_json TEXT NOT NULL DEFAULT '{"imports":[],"calls":[]}' CHECK(json_valid(refs_json)),
        importance REAL NOT NULL DEFAULT 0,
        path_text TEXT NOT NULL,
        comments TEXT NOT NULL DEFAULT '',
        signature TEXT NOT NULL DEFAULT '',
        refs TEXT NOT NULL DEFAULT '',
        identifier_tokens TEXT NOT NULL DEFAULT '',
        symbol_kind TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
      );
      CREATE UNIQUE INDEX context_entities_file_identity ON context_entities(cwd, path) WHERE kind = 'file';
      CREATE INDEX context_entities_cwd_path ON context_entities(cwd, path);
      CREATE INDEX context_entities_parent ON context_entities(parent_id);
      CREATE INDEX context_entities_touched ON context_entities(cwd, touched_at DESC) WHERE kind = 'file';
      -- A file's own position update (an application UPDATE, on a confirmed
      -- move) carries its symbols' denormalized path along for free, so every
      -- existing (cwd, path) query pattern keeps working without a join.
      CREATE TRIGGER context_entities_cascade_path
      AFTER UPDATE OF path ON context_entities
      WHEN NEW.kind = 'file' AND NEW.path != OLD.path
      BEGIN
        UPDATE context_entities SET path = NEW.path, path_text = NEW.path WHERE parent_id = NEW.id;
      END;
      CREATE VIRTUAL TABLE context_entities_fts USING fts5(
        name,
        path_text,
        comments,
        purpose,
        signature,
        refs,
        identifier_tokens,
        symbol_kind,
        content='context_entities',
        content_rowid='id',
        tokenize='porter unicode61 remove_diacritics 2',
        prefix='2 3 4 5 6 8 10'
      );
      CREATE TRIGGER context_entities_ai
      AFTER INSERT ON context_entities BEGIN
        INSERT INTO context_entities_fts(
          rowid, name, path_text, comments, purpose, signature, refs, identifier_tokens, symbol_kind
        ) VALUES (
          new.id, new.name, new.path_text, new.comments, COALESCE(new.purpose, ''), new.signature, new.refs,
          new.identifier_tokens, new.symbol_kind
        );
      END;
      CREATE TRIGGER context_entities_ad
      AFTER DELETE ON context_entities BEGIN
        INSERT INTO context_entities_fts(
          context_entities_fts, rowid, name, path_text, comments, purpose, signature, refs,
          identifier_tokens, symbol_kind
        ) VALUES (
          'delete', old.id, old.name, old.path_text, old.comments, COALESCE(old.purpose, ''), old.signature,
          old.refs, old.identifier_tokens, old.symbol_kind
        );
      END;
      CREATE TRIGGER context_entities_au
      AFTER UPDATE ON context_entities BEGIN
        INSERT INTO context_entities_fts(
          context_entities_fts, rowid, name, path_text, comments, purpose, signature, refs,
          identifier_tokens, symbol_kind
        ) VALUES (
          'delete', old.id, old.name, old.path_text, old.comments, COALESCE(old.purpose, ''), old.signature,
          old.refs, old.identifier_tokens, old.symbol_kind
        );
        INSERT INTO context_entities_fts(
          rowid, name, path_text, comments, purpose, signature, refs, identifier_tokens, symbol_kind
        ) VALUES (
          new.id, new.name, new.path_text, new.comments, COALESCE(new.purpose, ''), new.signature, new.refs,
          new.identifier_tokens, new.symbol_kind
        );
      END;"#;

/// Learned routes keep one bounded alias set for each source entity.
const ROUTE_HINTS_TABLE: &str = r#"      CREATE TABLE context_learned_routes (
        id INTEGER PRIMARY KEY,
        cwd TEXT NOT NULL,
        aliases TEXT NOT NULL,
        entity_id INTEGER REFERENCES context_entities(id) ON DELETE SET NULL,
        learned_path TEXT NOT NULL,
        learned_symbol TEXT NOT NULL DEFAULT '',
        source_digest TEXT NOT NULL,
         task_id TEXT,
         attempt INTEGER NOT NULL CHECK(attempt >= 0),
        profile_id TEXT NOT NULL,
        model TEXT NOT NULL,
        created_at TEXT NOT NULL,
        last_confirmed_at TEXT NOT NULL,
        UNIQUE(cwd, learned_path, learned_symbol)
       );
       CREATE INDEX context_learned_routes_cwd_entity ON context_learned_routes(cwd, entity_id);
       CREATE INDEX context_learned_routes_cwd_aliases ON context_learned_routes(cwd, aliases);
       CREATE VIRTUAL TABLE context_learned_routes_fts USING fts5(
         aliases,
         content='context_learned_routes',
         content_rowid='id',
         tokenize='porter unicode61 remove_diacritics 2',
         prefix='2 3 4 5 6 8 10'
       );
       CREATE TRIGGER context_learned_routes_ai
       AFTER INSERT ON context_learned_routes BEGIN
         INSERT INTO context_learned_routes_fts(rowid, aliases) VALUES (new.id, new.aliases);
       END;
       CREATE TRIGGER context_learned_routes_ad
       AFTER DELETE ON context_learned_routes BEGIN
         INSERT INTO context_learned_routes_fts(context_learned_routes_fts, rowid, aliases)
         VALUES ('delete', old.id, old.aliases);
       END;
       CREATE TRIGGER context_learned_routes_au
       AFTER UPDATE ON context_learned_routes BEGIN
         INSERT INTO context_learned_routes_fts(context_learned_routes_fts, rowid, aliases)
         VALUES ('delete', old.id, old.aliases);
         INSERT INTO context_learned_routes_fts(rowid, aliases) VALUES (new.id, new.aliases);
       END;"#;

pub fn migrate_v37_to_v38(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"BEGIN IMMEDIATE;
        DROP TRIGGER IF EXISTS context_learned_routes_ai;
        DROP TRIGGER IF EXISTS context_learned_routes_ad;
        DROP TRIGGER IF EXISTS context_learned_routes_au;
        DROP TABLE IF EXISTS context_learned_routes_fts;
        ALTER TABLE context_learned_routes RENAME TO context_learned_routes_old;
        CREATE TABLE context_learned_routes (
          id INTEGER PRIMARY KEY,
          cwd TEXT NOT NULL,
          hints TEXT NOT NULL,
          entity_id INTEGER REFERENCES context_entities(id) ON DELETE SET NULL,
          learned_path TEXT NOT NULL,
          learned_symbol TEXT NOT NULL DEFAULT '',
          source_digest TEXT NOT NULL,
          task_id TEXT,
          attempt INTEGER NOT NULL CHECK(attempt >= 0),
          profile_id TEXT NOT NULL,
          model TEXT NOT NULL,
          created_at TEXT NOT NULL,
          last_confirmed_at TEXT NOT NULL,
          UNIQUE(cwd, hints, learned_path, learned_symbol)
        );
        INSERT INTO context_learned_routes SELECT * FROM context_learned_routes_old;
        DROP TABLE context_learned_routes_old;
        CREATE INDEX context_learned_routes_cwd_entity ON context_learned_routes(cwd, entity_id);
        CREATE VIRTUAL TABLE context_learned_routes_fts USING fts5(hints, content='context_learned_routes', content_rowid='id', tokenize='porter unicode61 remove_diacritics 2', prefix='2 3 4 5 6 8 10');
        CREATE TRIGGER context_learned_routes_ai AFTER INSERT ON context_learned_routes BEGIN INSERT INTO context_learned_routes_fts(rowid, hints) VALUES (new.id, new.hints); END;
        CREATE TRIGGER context_learned_routes_ad AFTER DELETE ON context_learned_routes BEGIN INSERT INTO context_learned_routes_fts(context_learned_routes_fts, rowid, hints) VALUES ('delete', old.id, old.hints); END;
        CREATE TRIGGER context_learned_routes_au AFTER UPDATE ON context_learned_routes BEGIN INSERT INTO context_learned_routes_fts(context_learned_routes_fts, rowid, hints) VALUES ('delete', old.id, old.hints); INSERT INTO context_learned_routes_fts(rowid, hints) VALUES (new.id, new.hints); END;
        INSERT INTO context_learned_routes_fts(rowid, hints) SELECT id, hints FROM context_learned_routes;
        INSERT INTO schema_migrations(version, name) VALUES (38, 'user learned routes');
        COMMIT;"#,
    )?;
    Ok(())
}

pub fn migrate_v38_to_v39(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"BEGIN IMMEDIATE;
        DROP TRIGGER IF EXISTS context_learned_routes_ai;
        DROP TRIGGER IF EXISTS context_learned_routes_ad;
        DROP TRIGGER IF EXISTS context_learned_routes_au;
        DROP TABLE IF EXISTS context_learned_routes_fts;
        ALTER TABLE context_learned_routes RENAME TO context_learned_routes_old;
        CREATE TABLE context_learned_routes (
          id INTEGER PRIMARY KEY,
          cwd TEXT NOT NULL,
          aliases TEXT NOT NULL,
          entity_id INTEGER REFERENCES context_entities(id) ON DELETE SET NULL,
          learned_path TEXT NOT NULL,
          learned_symbol TEXT NOT NULL DEFAULT '',
          source_digest TEXT NOT NULL,
          task_id TEXT,
          attempt INTEGER NOT NULL CHECK(attempt >= 0),
          profile_id TEXT NOT NULL,
          model TEXT NOT NULL,
          created_at TEXT NOT NULL,
          last_confirmed_at TEXT NOT NULL,
          UNIQUE(cwd, learned_path, learned_symbol)
        );
        INSERT INTO context_learned_routes(id,cwd,aliases,entity_id,learned_path,learned_symbol,source_digest,task_id,attempt,profile_id,model,created_at,last_confirmed_at)
          SELECT MIN(id),cwd,SUBSTR(GROUP_CONCAT(hints, ' '), 1, 160),MAX(entity_id),learned_path,learned_symbol,MAX(source_digest),MAX(task_id),MAX(attempt),MAX(profile_id),MAX(model),MIN(created_at),MAX(last_confirmed_at)
          FROM context_learned_routes_old GROUP BY cwd,learned_path,learned_symbol;
        DROP TABLE context_learned_routes_old;
        CREATE INDEX context_learned_routes_cwd_entity ON context_learned_routes(cwd, entity_id);
        CREATE INDEX context_learned_routes_cwd_aliases ON context_learned_routes(cwd, aliases);
        CREATE VIRTUAL TABLE context_learned_routes_fts USING fts5(aliases, content='context_learned_routes', content_rowid='id', tokenize='porter unicode61 remove_diacritics 2', prefix='2 3 4 5 6 8 10');
        CREATE TRIGGER context_learned_routes_ai AFTER INSERT ON context_learned_routes BEGIN INSERT INTO context_learned_routes_fts(rowid, aliases) VALUES (new.id, new.aliases); END;
        CREATE TRIGGER context_learned_routes_ad AFTER DELETE ON context_learned_routes BEGIN INSERT INTO context_learned_routes_fts(context_learned_routes_fts, rowid, aliases) VALUES ('delete', old.id, old.aliases); END;
        CREATE TRIGGER context_learned_routes_au AFTER UPDATE ON context_learned_routes BEGIN INSERT INTO context_learned_routes_fts(context_learned_routes_fts, rowid, aliases) VALUES ('delete', old.id, old.aliases); INSERT INTO context_learned_routes_fts(rowid, aliases) VALUES (new.id, new.aliases); END;
        INSERT INTO context_learned_routes_fts(rowid, aliases) SELECT id, aliases FROM context_learned_routes;
        INSERT INTO schema_migrations(version, name) VALUES (39, 'learned route aliases');
        COMMIT;"#,
    )?;
    Ok(())
}
