//! The founding database shape and every migration that carries an older
//! database up to it.
//!
//! [`create_fresh_schema`] produces the current database shape.

use rusqlite::Connection;

use crate::connection::StoreError;

/// The schema this binary can read.
pub const LATEST_SCHEMA_VERSION: i64 = 46;

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
    let schema = [BASE_SCHEMA, CONTEXT_INDEX, ROUTE_HINTS_TABLE].concat();
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
        'queued','preparing_checkout','removing_checkout','pending','running','needs_input','answered','blocked','completed','failed','cancelled'
      )),
      output TEXT NOT NULL DEFAULT '',
      error TEXT,
      question TEXT,
      parent_task_id TEXT REFERENCES tasks(id),
      orchestrator_id TEXT,
      scope_json TEXT NOT NULL DEFAULT '{"read":["**"],"write":["**"]}' CHECK(json_valid(scope_json)),
      grant_id TEXT,
      allow_questions INTEGER NOT NULL DEFAULT 1 CHECK(allow_questions IN (0,1)),
      can_delegate INTEGER NOT NULL DEFAULT 0 CHECK(can_delegate IN (0,1)),
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
       cost_usd_estimated INTEGER,
       attachments_json TEXT CHECK(attachments_json IS NULL OR json_valid(attachments_json)),
       checkout_state TEXT CHECK(checkout_state IS NULL OR checkout_state IN (
        'queued','preparing_checkout','removing_checkout','pending','running','needs_input','answered','blocked','completed','failed','cancelled'
       ))
    );
    CREATE INDEX IF NOT EXISTS tasks_parent ON tasks(parent_task_id);
    CREATE INDEX IF NOT EXISTS tasks_updated_at ON tasks(updated_at DESC, id DESC);
    CREATE INDEX IF NOT EXISTS tasks_profile_updated ON tasks(profile_id, updated_at DESC);
    CREATE INDEX IF NOT EXISTS tasks_worktree_path ON tasks(worktree_path, archived_at);
    CREATE INDEX IF NOT EXISTS tasks_title_nocase ON tasks(title COLLATE NOCASE);
    CREATE INDEX IF NOT EXISTS tasks_tldr_nocase ON tasks(tldr COLLATE NOCASE);
    CREATE INDEX IF NOT EXISTS tasks_prompt_nocase ON tasks(prompt COLLATE NOCASE);
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
    CREATE TABLE IF NOT EXISTS context_index (
      cwd TEXT PRIMARY KEY,
      scheme INTEGER NOT NULL,
      state TEXT NOT NULL CHECK(state IN ('building','ready','partial')),
      built_at TEXT,
      file_count INTEGER NOT NULL DEFAULT 0,
      symbol_count INTEGER NOT NULL DEFAULT 0,
      updated_at TEXT NOT NULL
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
     INSERT INTO schema_migrations(version, name) VALUES (46, 'checkout lifecycle');"#;

/// One row per indexed file, one per symbol, with the symbol search index
/// derived from the same rows.
const CONTEXT_INDEX: &str = r#"      CREATE TABLE context_files (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        cwd TEXT NOT NULL,
        path TEXT NOT NULL,
        lang TEXT NOT NULL,
        -- Whole-file content hash. A file whose hash still matches is never
        -- re-parsed, which is what makes an unchanged relearn cheap.
        digest TEXT NOT NULL,
        size INTEGER NOT NULL,
        mtime_ms INTEGER NOT NULL,
        lines INTEGER NOT NULL,
        updated_at TEXT NOT NULL
      );
      CREATE UNIQUE INDEX context_files_identity ON context_files(cwd, path);
      CREATE INDEX context_files_digest ON context_files(cwd, digest);
      CREATE TABLE context_symbols (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        file_id INTEGER NOT NULL REFERENCES context_files(id) ON DELETE CASCADE,
        cwd TEXT NOT NULL,
        path TEXT NOT NULL,
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        -- The name folded to one lookup key, so an exact-name question is an
        -- index seek rather than a scan.
        name_key TEXT NOT NULL,
        qualified TEXT NOT NULL,
        parent TEXT NOT NULL DEFAULT '',
        line INTEGER NOT NULL,
        end_line INTEGER NOT NULL,
        signature TEXT NOT NULL DEFAULT '',
        doc TEXT NOT NULL DEFAULT '',
        exported INTEGER NOT NULL DEFAULT 0 CHECK(exported IN (0,1)),
        -- Hash of the declaration text with the name masked out, so a route
        -- can follow the symbol through a rename or a move to another file.
        digest TEXT NOT NULL,
        tokens TEXT NOT NULL DEFAULT ''
      );
      CREATE INDEX context_symbols_file ON context_symbols(file_id);
      CREATE INDEX context_symbols_name ON context_symbols(cwd, name_key);
      CREATE INDEX context_symbols_digest ON context_symbols(cwd, digest);
      CREATE INDEX context_symbols_path ON context_symbols(cwd, path, line);
      CREATE VIRTUAL TABLE context_symbols_fts USING fts5(
        name,
        qualified,
        tokens,
        signature,
        doc,
        path,
        content='context_symbols',
        content_rowid='id',
        tokenize='porter unicode61 remove_diacritics 2'
      );
      CREATE TRIGGER context_symbols_ai
      AFTER INSERT ON context_symbols BEGIN
        INSERT INTO context_symbols_fts(rowid, name, qualified, tokens, signature, doc, path)
        VALUES (new.id, new.name, new.qualified, new.tokens, new.signature, new.doc, new.path);
      END;
      CREATE TRIGGER context_symbols_ad
      AFTER DELETE ON context_symbols BEGIN
        INSERT INTO context_symbols_fts(context_symbols_fts, rowid, name, qualified, tokens, signature, doc, path)
        VALUES ('delete', old.id, old.name, old.qualified, old.tokens, old.signature, old.doc, old.path);
      END;
      CREATE TRIGGER context_symbols_au
      AFTER UPDATE ON context_symbols BEGIN
        INSERT INTO context_symbols_fts(context_symbols_fts, rowid, name, qualified, tokens, signature, doc, path)
        VALUES ('delete', old.id, old.name, old.qualified, old.tokens, old.signature, old.doc, old.path);
        INSERT INTO context_symbols_fts(rowid, name, qualified, tokens, signature, doc, path)
        VALUES (new.id, new.name, new.qualified, new.tokens, new.signature, new.doc, new.path);
      END;"#;

/// Learned routes keep one bounded alias set for each place a worker found.
const ROUTE_HINTS_TABLE: &str = r#"      CREATE TABLE context_learned_routes (
        id INTEGER PRIMARY KEY,
        cwd TEXT NOT NULL,
        aliases TEXT NOT NULL,
        hint_keys TEXT NOT NULL DEFAULT '',
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
        INSERT INTO context_learned_routes(id,cwd,hints,learned_path,learned_symbol,source_digest,task_id,attempt,profile_id,model,created_at,last_confirmed_at)
          SELECT id,cwd,hints,learned_path,learned_symbol,source_digest,task_id,attempt,profile_id,model,created_at,last_confirmed_at
          FROM context_learned_routes_old;
        DROP TABLE context_learned_routes_old;
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
        INSERT INTO context_learned_routes(id,cwd,aliases,learned_path,learned_symbol,source_digest,task_id,attempt,profile_id,model,created_at,last_confirmed_at)
          SELECT MIN(id),cwd,SUBSTR(GROUP_CONCAT(hints, ' '), 1, 160),learned_path,learned_symbol,MAX(source_digest),MAX(task_id),MAX(attempt),MAX(profile_id),MAX(model),MIN(created_at),MAX(last_confirmed_at)
          FROM context_learned_routes_old GROUP BY cwd,learned_path,learned_symbol;
        DROP TABLE context_learned_routes_old;
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

pub fn migrate_v39_to_v40(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"BEGIN IMMEDIATE;
        CREATE INDEX IF NOT EXISTS tasks_title_nocase ON tasks(title COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS tasks_tldr_nocase ON tasks(tldr COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS tasks_prompt_nocase ON tasks(prompt COLLATE NOCASE);
        INSERT INTO schema_migrations(version, name) VALUES (40, 'task search indexes');
        COMMIT;"#,
    )?;
    Ok(())
}

pub fn migrate_v40_to_v41(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"BEGIN IMMEDIATE;
        ALTER TABLE tasks ADD COLUMN attachments_json TEXT;
        INSERT INTO schema_migrations(version, name) VALUES (41, 'task attachments');
        COMMIT;"#,
    )?;
    Ok(())
}

/// Replace the line-scanned entity table with the parsed file and symbol
/// tables. Saved routes survive as text; the next relearn re-resolves each one
/// against the rebuilt index and refreshes its digest.
pub fn migrate_v41_to_v42(conn: &Connection) -> Result<(), StoreError> {
    // Routes are carried out to a constraint-free table first. They reference
    // the entity rows, and a parent table cannot be dropped while a child
    // still points at it.
    let batch = format!(
        r#"BEGIN IMMEDIATE;
        DROP TRIGGER IF EXISTS context_learned_routes_ai;
        DROP TRIGGER IF EXISTS context_learned_routes_ad;
        DROP TRIGGER IF EXISTS context_learned_routes_au;
        DROP TABLE IF EXISTS context_learned_routes_fts;
        CREATE TABLE context_learned_routes_carry AS
          SELECT cwd,aliases,learned_path,learned_symbol,source_digest,task_id,attempt,profile_id,model,created_at,last_confirmed_at
          FROM context_learned_routes;
        DROP TABLE context_learned_routes;
        DROP TRIGGER IF EXISTS context_entities_ai;
        DROP TRIGGER IF EXISTS context_entities_ad;
        DROP TRIGGER IF EXISTS context_entities_au;
        DROP TRIGGER IF EXISTS context_entities_cascade_path;
        DROP TABLE IF EXISTS context_entities_fts;
        DROP TABLE IF EXISTS context_entities;
        DROP TABLE IF EXISTS context_maps;
        CREATE TABLE context_maps (
          cwd TEXT PRIMARY KEY,
          scheme INTEGER NOT NULL,
          state TEXT NOT NULL CHECK(state IN ('building','ready','partial')),
          built_at TEXT,
          file_count INTEGER NOT NULL DEFAULT 0,
          symbol_count INTEGER NOT NULL DEFAULT 0,
          updated_at TEXT NOT NULL
        );
        {CONTEXT_INDEX}
        {ROUTE_HINTS_TABLE}
        INSERT INTO context_learned_routes(cwd,aliases,learned_path,learned_symbol,source_digest,task_id,attempt,profile_id,model,created_at,last_confirmed_at)
          SELECT cwd,aliases,learned_path,learned_symbol,source_digest,task_id,attempt,profile_id,model,created_at,last_confirmed_at
          FROM context_learned_routes_carry;
        DROP TABLE context_learned_routes_carry;
        INSERT INTO schema_migrations(version, name) VALUES (42, 'tree-sitter code index');
        COMMIT;"#
    );
    conn.execute_batch(&batch)?;
    Ok(())
}

/// Take the context map out of the database.
///
/// The map's own tables went at v42. What is left is the per-project
/// bookkeeping row the code index writes, which outlived the map and is
/// renamed here for what it actually records.
pub fn migrate_v42_to_v43(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"BEGIN IMMEDIATE;
        ALTER TABLE context_maps RENAME TO context_index;
        INSERT INTO schema_migrations(version, name) VALUES (43, 'code index without context maps');
        COMMIT;"#,
    )?;
    Ok(())
}

/// Record whether a task may hand work onward. Tasks already on file kept
/// their work to themselves, so they carry the same answer.
pub fn migrate_v43_to_v44(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"BEGIN IMMEDIATE;
        ALTER TABLE tasks ADD COLUMN can_delegate INTEGER NOT NULL DEFAULT 0 CHECK(can_delegate IN (0,1));
        INSERT INTO schema_migrations(version, name) VALUES (44, 'tasks that may delegate');
        COMMIT;"#,
    )?;
    Ok(())
}

/// Keep the phrases a route was taught, folded to one key each, next to the
/// alias set it is searched by. Routes already on file carry their aliases
/// across as their one phrase, so a single-word hint still answers word for
/// word and every route keeps answering by overlap.
///
/// A database coming up from v41 rebuilt this table from the current shape and
/// already has the column, so it is added only where it is missing.
pub fn migrate_v44_to_v45(conn: &Connection) -> Result<(), StoreError> {
    let add = if has_column(conn, "context_learned_routes", "hint_keys")? {
        ""
    } else {
        "ALTER TABLE context_learned_routes ADD COLUMN hint_keys TEXT NOT NULL DEFAULT '';"
    };
    conn.execute_batch(&format!(
        r#"BEGIN IMMEDIATE;
        {add}
        UPDATE context_learned_routes SET hint_keys=aliases WHERE hint_keys='';
        INSERT INTO schema_migrations(version, name) VALUES (45, 'taught route phrases');
        COMMIT;"#
    ))?;
    Ok(())
}

pub fn migrate_v45_to_v46(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"PRAGMA legacy_alter_table=ON;
        PRAGMA foreign_keys=OFF;
        BEGIN IMMEDIATE;
        ALTER TABLE tasks RENAME TO tasks_v45;
        CREATE TABLE tasks (
          id TEXT PRIMARY KEY, kind TEXT NOT NULL DEFAULT 'delegated' CHECK(kind IN ('delegated','orchestrator')),
          profile_id TEXT NOT NULL REFERENCES profiles(id), model TEXT NOT NULL, prompt TEXT NOT NULL,
          cwd TEXT NOT NULL, branch TEXT,
          state TEXT NOT NULL CHECK(state IN ('queued','preparing_checkout','removing_checkout','pending','running','needs_input','answered','blocked','completed','failed','cancelled')),
          output TEXT NOT NULL DEFAULT '', error TEXT, question TEXT, parent_task_id TEXT REFERENCES tasks(id), orchestrator_id TEXT,
          scope_json TEXT NOT NULL DEFAULT '{"read":["**"],"write":["**"]}' CHECK(json_valid(scope_json)), grant_id TEXT,
          allow_questions INTEGER NOT NULL DEFAULT 1 CHECK(allow_questions IN (0,1)), can_delegate INTEGER NOT NULL DEFAULT 0 CHECK(can_delegate IN (0,1)),
          timeout_ms INTEGER, session_id TEXT, shipped_prompt TEXT, completion_json TEXT CHECK(completion_json IS NULL OR json_valid(completion_json)),
          attempts_json TEXT CHECK(attempts_json IS NULL OR json_valid(attempts_json)), cost_usd REAL, turns INTEGER, tokens_in INTEGER, tokens_out INTEGER,
          spend_at TEXT, archived_at TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, effort TEXT, tldr TEXT, title TEXT,
          worker_json TEXT, selection_json TEXT, effort_actual TEXT, origin_cwd TEXT, worktree_path TEXT, worktree_branch TEXT,
          worktree_links_json TEXT, caller_id TEXT, cost_usd_estimated INTEGER, attachments_json TEXT CHECK(attachments_json IS NULL OR json_valid(attachments_json)),
          checkout_state TEXT CHECK(checkout_state IS NULL OR checkout_state IN ('queued','preparing_checkout','removing_checkout','pending','running','needs_input','answered','blocked','completed','failed','cancelled'))
        );
        INSERT INTO tasks(id,kind,profile_id,model,prompt,cwd,branch,state,output,error,question,parent_task_id,orchestrator_id,scope_json,grant_id,allow_questions,can_delegate,timeout_ms,session_id,shipped_prompt,completion_json,attempts_json,cost_usd,turns,tokens_in,tokens_out,spend_at,archived_at,created_at,updated_at,effort,tldr,title,worker_json,selection_json,effort_actual,origin_cwd,worktree_path,worktree_branch,worktree_links_json,caller_id,cost_usd_estimated,attachments_json)
          SELECT id,kind,profile_id,model,prompt,cwd,branch,state,output,error,question,parent_task_id,orchestrator_id,scope_json,grant_id,allow_questions,can_delegate,timeout_ms,session_id,shipped_prompt,completion_json,attempts_json,cost_usd,turns,tokens_in,tokens_out,spend_at,archived_at,created_at,updated_at,effort,tldr,title,worker_json,selection_json,effort_actual,origin_cwd,worktree_path,worktree_branch,worktree_links_json,caller_id,cost_usd_estimated,attachments_json FROM tasks_v45;
        DROP TABLE tasks_v45;
        CREATE INDEX tasks_parent ON tasks(parent_task_id);
        CREATE INDEX tasks_updated_at ON tasks(updated_at DESC, id DESC);
        CREATE INDEX tasks_profile_updated ON tasks(profile_id, updated_at DESC);
        CREATE INDEX tasks_worktree_path ON tasks(worktree_path, archived_at);
        CREATE INDEX tasks_title_nocase ON tasks(title COLLATE NOCASE);
        CREATE INDEX tasks_tldr_nocase ON tasks(tldr COLLATE NOCASE);
        CREATE INDEX tasks_prompt_nocase ON tasks(prompt COLLATE NOCASE);
        INSERT INTO schema_migrations(version, name) VALUES (46, 'checkout lifecycle');
        COMMIT;
        PRAGMA foreign_keys=ON;
        PRAGMA legacy_alter_table=OFF;"#,
    )?;
    Ok(())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, StoreError> {
    Ok(conn
        .prepare(&format!(
            "SELECT 1 FROM pragma_table_info('{table}') WHERE name=?"
        ))?
        .exists([column])?)
}
