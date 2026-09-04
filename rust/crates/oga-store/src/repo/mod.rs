//! Typed persistence operations. Repository methods deliberately accept the
//! domain records rather than exposing SQL rows to the service layer.

use std::collections::BTreeMap;

use oga_domain::{
    ConsumerCursor, MemoryEntry, Profile, ProfileFailure, ProfileSuccess, ScopeGrant, SpendTotals,
    Task, TaskEvent, TaskHold, TaskKind, TaskScope, TaskState, TaskTurn, TaskTurnStatus,
};
use rusqlite::{OptionalExtension, Row, params};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

use crate::{Store, StoreError};

pub struct Repositories<'a> {
    store: &'a Store,
}

impl Store {
    pub fn repositories(&self) -> Repositories<'_> {
        Repositories { store: self }
    }
}

impl<'a> Repositories<'a> {
    pub fn profiles(&self) -> Profiles<'a> {
        Profiles { store: self.store }
    }
    pub fn tasks(&self) -> Tasks<'a> {
        Tasks { store: self.store }
    }
    pub fn events(&self) -> Events<'a> {
        Events { store: self.store }
    }
    pub fn turns(&self) -> Turns<'a> {
        Turns { store: self.store }
    }
    pub fn memories(&self) -> Memories<'a> {
        Memories { store: self.store }
    }
    pub fn grants(&self) -> Grants<'a> {
        Grants { store: self.store }
    }
    pub fn failures(&self) -> Failures<'a> {
        Failures { store: self.store }
    }
    pub fn holds(&self) -> Holds<'a> {
        Holds { store: self.store }
    }
    pub fn deliveries(&self) -> Deliveries<'a> {
        Deliveries { store: self.store }
    }
    pub fn settings(&self) -> Settings<'a> {
        Settings { store: self.store }
    }
    pub fn spend(&self) -> Spend<'a> {
        Spend { store: self.store }
    }
}

fn encode<T: Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|e| StoreError::Refusal(format!("invalid JSON: {e}")))
}

fn decode<T: DeserializeOwned>(value: &str) -> Result<T, StoreError> {
    serde_json::from_str(value)
        .map_err(|e| StoreError::Refusal(format!("invalid stored JSON: {e}")))
}

fn bool_value(value: bool) -> i64 {
    i64::from(value)
}
pub struct Profiles<'a> {
    store: &'a Store,
}

impl Profiles<'_> {
    pub fn insert(&self, profile: &Profile, now: &str) -> Result<(), StoreError> {
        let env = encode(&profile.env)?;
        let capabilities = encode(&profile.capabilities)?;
        let command = profile.command.as_ref().map(encode).transpose()?;
        self.store.transaction(|tx| {
            tx.execute("INSERT INTO profiles (id,label,provider,default_model,enabled,env_json,capabilities_json,command_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
                params![profile.id, profile.label, profile.provider.as_str(), profile.default_model, bool_value(profile.enabled), env, capabilities, command, now, now])?;
            Ok(())
        })
    }

    pub fn update_if_unchanged(
        &self,
        id: &str,
        expected: &Profile,
        profile: &Profile,
        now: &str,
    ) -> Result<bool, StoreError> {
        let env = encode(&profile.env)?;
        let capabilities = encode(&profile.capabilities)?;
        let command = profile.command.as_ref().map(encode).transpose()?;
        let expected_env = encode(&expected.env)?;
        let expected_capabilities = encode(&expected.capabilities)?;
        let expected_command = expected.command.as_ref().map(encode).transpose()?;
        self.store.transaction(|tx| {
            Ok(tx.execute(
                "UPDATE profiles SET label=?,provider=?,default_model=?,enabled=?,env_json=?,capabilities_json=?,command_json=?,updated_at=? WHERE id=? AND deleted_at IS NULL AND label=? AND provider=? AND default_model=? AND enabled=? AND env_json=? AND capabilities_json=? AND command_json IS ?",
                params![
                    profile.label,
                    profile.provider.as_str(),
                    profile.default_model,
                    bool_value(profile.enabled),
                    env,
                    capabilities,
                    command,
                    now,
                    id,
                    expected.label,
                    expected.provider.as_str(),
                    expected.default_model,
                    bool_value(expected.enabled),
                    expected_env,
                    expected_capabilities,
                    expected_command,
                ],
            )? != 0)
        })
    }

    pub fn remove(&self, id: &str, now: &str) -> Result<bool, StoreError> {
        self.store.transaction(|tx| {
            Ok(tx.execute(
                "UPDATE profiles SET deleted_at=?,updated_at=? WHERE id=? AND deleted_at IS NULL",
                params![now, now, id],
            )? != 0)
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<Profile>, StoreError> {
        self.store.with_connection(|c| c.query_row("SELECT id,label,provider,default_model,enabled,env_json,capabilities_json,command_json FROM profiles WHERE id=? AND deleted_at IS NULL", [id], profile_from_row).optional().map_err(Into::into))
    }

    pub fn exists(&self, id: &str) -> Result<bool, StoreError> {
        self.store.with_connection(|c| {
            Ok(
                c.query_row("SELECT 1 FROM profiles WHERE id=?", [id], |_| Ok(()))
                    .optional()?
                    .is_some(),
            )
        })
    }

    pub fn list(&self) -> Result<Vec<Profile>, StoreError> {
        self.store.with_connection(|c| {
            let mut stmt = c.prepare("SELECT id,label,provider,default_model,enabled,env_json,capabilities_json,command_json FROM profiles WHERE deleted_at IS NULL ORDER BY created_at,id")?;
            Ok(stmt.query_map([], profile_from_row)?.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn set_enabled(&self, id: &str, enabled: bool, now: &str) -> Result<bool, StoreError> {
        self.store.transaction(|tx| {
            Ok(tx.execute(
                "UPDATE profiles SET enabled=?,updated_at=? WHERE id=? AND deleted_at IS NULL",
                params![bool_value(enabled), now, id],
            )? != 0)
        })
    }
}

fn profile_from_row(row: &Row<'_>) -> rusqlite::Result<Profile> {
    let provider = row.get::<_, String>(2)?;
    let provider = serde_json::from_value(json!(provider)).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let env =
        decode::<BTreeMap<String, String>>(&row.get::<_, String>(5)?).map_err(store_row_error)?;
    let capabilities = decode::<Vec<String>>(&row.get::<_, String>(6)?).map_err(store_row_error)?;
    let command = row
        .get::<_, Option<String>>(7)?
        .map(|v| decode(&v))
        .transpose()
        .map_err(store_row_error)?;
    Ok(Profile {
        id: row.get(0)?,
        label: row.get(1)?,
        provider,
        default_model: row.get(3)?,
        enabled: row.get::<_, i64>(4)? != 0,
        env,
        capabilities,
        command,
    })
}

fn store_row_error(error: StoreError) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(error.to_string())
}

pub struct Memories<'a> {
    store: &'a Store,
}
impl Memories<'_> {
    pub fn upsert(
        &self,
        entry: &MemoryEntry,
        expected_version: Option<u64>,
    ) -> Result<MemoryEntry, StoreError> {
        self.store.transaction(|tx| {
            let current: Option<u64> = tx.query_row("SELECT version FROM memories WHERE cwd=? AND key=?", params![entry.cwd, entry.key], |r| r.get(0)).optional()?;
            if expected_version.is_some() && current != expected_version { return Err(StoreError::Refusal("memory revision conflict".into())); }
            let version = current.map_or(entry.version, |v| v + 1);
            tx.execute("INSERT INTO memories(cwd,key,value,version,created_at,updated_at) VALUES(?,?,?,?,?,?) ON CONFLICT(cwd,key) DO UPDATE SET value=excluded.value,version=excluded.version,updated_at=excluded.updated_at", params![entry.cwd, entry.key, entry.value, version, entry.created_at, entry.updated_at])?;
            Ok(MemoryEntry { version, ..entry.clone() })
        })
    }
    pub fn list(&self, cwd: &str) -> Result<Vec<MemoryEntry>, StoreError> {
        self.store.with_connection(|c| { let mut s=c.prepare("SELECT cwd,key,value,version,created_at,updated_at FROM memories WHERE cwd=? ORDER BY key")?; Ok(s.query_map([cwd], |r| Ok(MemoryEntry { cwd:r.get(0)?,key:r.get(1)?,value:r.get(2)?,version:r.get(3)?,created_at:r.get(4)?,updated_at:r.get(5)? }))?.collect::<Result<Vec<_>,_>>()?) })
    }
}

pub struct Grants<'a> {
    store: &'a Store,
}
impl Grants<'_> {
    pub fn upsert(&self, grant: &ScopeGrant) -> Result<(), StoreError> {
        let scope = encode(&grant.scope)?;
        self.store.transaction(|tx| { tx.execute("INSERT INTO scope_grants(id,cwd,profile_id,scope_json,created_at,last_used_at,use_count) VALUES(?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET scope_json=excluded.scope_json,last_used_at=excluded.last_used_at,use_count=excluded.use_count", params![grant.id,grant.cwd,grant.profile_id,scope,grant.created_at,grant.last_used_at,grant.use_count])?; Ok(()) })
    }
    pub fn for_cwd(&self, cwd: &str) -> Result<Vec<ScopeGrant>, StoreError> {
        self.store.with_connection(|c| { let mut s=c.prepare("SELECT id,cwd,profile_id,scope_json,created_at,last_used_at,use_count FROM scope_grants WHERE cwd=? ORDER BY last_used_at DESC")?; Ok(s.query_map([cwd], |r| { let scope=decode::<TaskScope>(&r.get::<_,String>(3)?).map_err(store_row_error)?; Ok(ScopeGrant{id:r.get(0)?,cwd:r.get(1)?,profile_id:r.get(2)?,scope,created_at:r.get(4)?,last_used_at:r.get(5)?,use_count:r.get(6)?}) })?.collect::<Result<Vec<_>,_>>()?) })
    }

    pub fn touch(&self, id: &str, now: &str) -> Result<bool, StoreError> {
        self.store.transaction(|tx| {
            Ok(tx.execute(
                "UPDATE scope_grants SET last_used_at=?,use_count=use_count+1 WHERE id=?",
                params![now, id],
            )? != 0)
        })
    }
}

pub struct Tasks<'a> {
    store: &'a Store,
}
impl Tasks<'_> {
    pub fn insert(&self, task: &Task) -> Result<(), StoreError> {
        let kind = task.kind.unwrap_or(TaskKind::Delegated);
        let scope = encode(&task.scope)?;
        let completion = task.completion.as_ref().map(encode).transpose()?;
        let attempts = encode(&task.attempts)?;
        self.store.transaction(|tx| { tx.execute("INSERT INTO tasks(id,kind,profile_id,model,prompt,cwd,branch,state,output,error,question,parent_task_id,orchestrator_id,scope_json,grant_id,allow_questions,timeout_ms,session_id,shipped_prompt,completion_json,attempts_json,cost_usd,cost_usd_estimated,turns,archived_at,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", params![task.id,kind_string(kind),task.profile_id,task.model,task.prompt,task.cwd,task.branch,task.state.as_str(),task.output,task.error,task.question,task.parent_task_id,task.orchestrator_id,scope,task.grant_id,bool_value(task.allow_questions),task.timeout_ms,task.session_id,task.shipped_prompt,completion,attempts,task.cost_usd,bool_value(task.cost_usd_estimated),task.turns,task.archived_at,task.created_at,task.updated_at])?; Ok(()) })
    }
    pub fn get(&self, id: &str) -> Result<Option<Task>, StoreError> {
        self.store.with_connection(|c| c.query_row("SELECT id,kind,profile_id,model,prompt,cwd,branch,state,output,error,question,parent_task_id,orchestrator_id,scope_json,grant_id,allow_questions,timeout_ms,session_id,shipped_prompt,completion_json,attempts_json,cost_usd,cost_usd_estimated,turns,archived_at,created_at,updated_at FROM tasks WHERE id=?", [id], task_from_row).optional().map_err(Into::into))
    }
    pub fn set_state(&self, id: &str, state: TaskState, now: &str) -> Result<bool, StoreError> {
        self.store.transaction(|tx| {
            Ok(tx.execute(
                "UPDATE tasks SET state=?,updated_at=? WHERE id=?",
                params![state.as_str(), now, id],
            )? != 0)
        })
    }
}
fn kind_string(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::Delegated => "delegated",
        TaskKind::Orchestrator => "orchestrator",
    }
}
fn task_from_row(r: &Row<'_>) -> rusqlite::Result<Task> {
    let kind = match r.get::<_, String>(1)?.as_str() {
        "orchestrator" => Some(TaskKind::Orchestrator),
        _ => Some(TaskKind::Delegated),
    };
    let state =
        decode::<TaskState>(&format!("\"{}\"", r.get::<_, String>(7)?)).map_err(store_row_error)?;
    let scope = decode(&r.get::<_, String>(13)?).map_err(store_row_error)?;
    let completion = r
        .get::<_, Option<String>>(19)?
        .map(|v| decode(&v))
        .transpose()
        .map_err(store_row_error)?;
    let attempts = decode::<Vec<_>>(
        &r.get::<_, Option<String>>(20)?
            .unwrap_or_else(|| "[]".into()),
    )
    .map_err(store_row_error)?;
    Ok(Task {
        id: r.get(0)?,
        kind,
        profile_id: r.get(2)?,
        model: r.get(3)?,
        prompt: r.get(4)?,
        shipped_prompt: r.get(18)?,
        cwd: r.get(5)?,
        branch: r.get(6)?,
        worktree: None,
        worktree_label: None,
        state,
        created_at: r.get(25)?,
        updated_at: r.get(26)?,
        output: r.get(8)?,
        error: r.get(9)?,
        question: r.get(10)?,
        parent_task_id: r.get(11)?,
        orchestrator_id: r.get(12)?,
        scope,
        grant_id: r.get(14)?,
        allow_questions: r.get::<_, i64>(15)? != 0,
        timeout_ms: r.get(16)?,
        effort: None,
        effort_actual: None,
        tldr: None,
        title: None,
        session_id: r.get(17)?,
        completion,
        attempts,
        cost_usd: r.get(21)?,
        cost_usd_estimated: r.get::<_, Option<i64>>(22)?.unwrap_or(0) != 0,
        turns: r.get(23)?,
        archived_at: r.get(24)?,
        queued_follow_ups: None,
        queued_follow_up_items: None,
        hold: None,
    })
}

pub struct Events<'a> {
    store: &'a Store,
}
impl Events<'_> {
    pub fn append(&self, event: &TaskEvent) -> Result<i64, StoreError> {
        let payload = encode(&event.payload)?;
        self.store.transaction(|tx|{tx.execute("INSERT INTO task_events(task_id,event_type,state,payload,created_at,turn_id) VALUES(?,?,?,?,?,?)",params![event.task_id,event.kind,event.state.as_str(),payload,event.created_at,event.turn_id])?;Ok(tx.last_insert_rowid())})
    }
    pub fn list(&self, task_id: &str) -> Result<Vec<TaskEvent>, StoreError> {
        self.store.with_connection(|c|{let mut s=c.prepare("SELECT id,task_id,event_type,state,payload,created_at,turn_id FROM task_events WHERE task_id=? ORDER BY id")?;Ok(s.query_map([task_id],|r|{let state=decode(&format!("\"{}\"",r.get::<_,String>(3)?)).map_err(store_row_error)?;let payload=decode(&r.get::<_,String>(4)?).map_err(store_row_error)?;Ok(TaskEvent{id:r.get(0)?,task_id:r.get(1)?,kind:r.get(2)?,state,payload,created_at:r.get(5)?,turn_id:r.get(6)?})})?.collect::<Result<Vec<_>,_>>()?)})
    }
}

pub struct Failures<'a> {
    store: &'a Store,
}
impl Failures<'_> {
    pub fn record(&self, f: &ProfileFailure) -> Result<(), StoreError> {
        self.store.transaction(|tx|{tx.execute("INSERT INTO profile_failures(profile_id,code,message,failed_at,consecutive_failures,retry_at,model) VALUES(?,?,?,?,?,?,?) ON CONFLICT(profile_id,model) DO UPDATE SET code=excluded.code,message=excluded.message,failed_at=excluded.failed_at,consecutive_failures=excluded.consecutive_failures,retry_at=excluded.retry_at",params![f.profile_id,serde_json::to_value(f.code).unwrap().as_str().unwrap(),f.message,f.failed_at,f.consecutive_failures,f.retry_at,f.model.clone().unwrap_or_default()])?;Ok(())})
    }
    pub fn clear(&self, s: &ProfileSuccess) -> Result<bool, StoreError> {
        self.store.transaction(|tx| {
            Ok(tx.execute(
                "DELETE FROM profile_failures WHERE profile_id=?",
                [&s.profile_id],
            )? != 0)
        })
    }
}

pub struct Holds<'a> {
    store: &'a Store,
}
impl Holds<'_> {
    pub fn put(&self, h: &TaskHold) -> Result<(), StoreError> {
        let args = encode(&h.args)?;
        self.store.transaction(|tx|{tx.execute("INSERT OR REPLACE INTO task_holds(task_id,verb,args_json,start_at,await_profile,await_model,next_check_at,expires_at,probe_count,note,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",params![h.task_id,serde_json::to_value(h.verb).unwrap().as_str().unwrap(),args,h.start_at,h.await_profile,h.await_model,h.next_check_at,h.expires_at,h.probe_count,h.note,h.created_at,h.updated_at])?;Ok(())})
    }
    pub fn remove(&self, id: &str) -> Result<bool, StoreError> {
        self.store
            .transaction(|tx| Ok(tx.execute("DELETE FROM task_holds WHERE task_id=?", [id])? != 0))
    }
}

pub struct Deliveries<'a> {
    store: &'a Store,
}
impl Deliveries<'_> {
    pub fn cursor(&self, c: &ConsumerCursor) -> Result<(), StoreError> {
        self.store.transaction(|tx|{tx.execute("INSERT INTO consumer_cursors(consumer_id,cursor,updated_at) VALUES(?,?,?) ON CONFLICT(consumer_id) DO UPDATE SET cursor=excluded.cursor,updated_at=excluded.updated_at",params![c.consumer_id,c.cursor,c.updated_at])?;Ok(())})
    }
    pub fn advance(&self, id: &str, cursor: i64, now: &str) -> Result<bool, StoreError> {
        self.store.transaction(|tx|Ok(tx.execute("UPDATE consumer_cursors SET cursor=?,updated_at=? WHERE consumer_id=? AND cursor<=?",params![cursor,now,id,cursor])?!=0))
    }
}

pub struct Turns<'a> {
    store: &'a Store,
}
impl Turns<'_> {
    pub fn start(&self, task_id: &str, ordinal: u64, now: &str) -> Result<i64, StoreError> {
        self.store.transaction(|tx| {
            tx.execute(
                "INSERT INTO task_turns(task_id,ordinal,status,started_at) VALUES(?,?,?,?)",
                params![task_id, ordinal, "running", now],
            )?;
            Ok(tx.last_insert_rowid())
        })
    }
    pub fn finish(
        &self,
        id: i64,
        status: TaskTurnStatus,
        ended_at: &str,
    ) -> Result<bool, StoreError> {
        let value = encode(&status)?.trim_matches('"').to_string();
        self.store.transaction(|tx| {
            Ok(tx.execute(
                "UPDATE task_turns SET status=?,ended_at=? WHERE id=?",
                params![value, ended_at, id],
            )? != 0)
        })
    }
    pub fn list(&self, task_id: &str) -> Result<Vec<TaskTurn>, StoreError> {
        self.store.with_connection(|c|{let mut s=c.prepare("SELECT id,task_id,ordinal,status,started_at,ended_at FROM task_turns WHERE task_id=? ORDER BY ordinal")?;Ok(s.query_map([task_id],|r|{let status=decode(&format!("\"{}\"",r.get::<_,String>(3)?)).map_err(store_row_error)?;Ok(TaskTurn{id:r.get(0)?,task_id:r.get(1)?,ordinal:r.get(2)?,status,started_at:r.get(4)?,ended_at:r.get(5)?})})?.collect::<Result<Vec<_>,_>>()?)})
    }
}

pub struct Settings<'a> {
    store: &'a Store,
}
impl Settings<'_> {
    pub fn get(&self, cwd: &str, key: &str) -> Result<Option<String>, StoreError> {
        self.store.with_connection(|c| {
            c.query_row(
                "SELECT value FROM cwd_settings WHERE cwd=? AND key=?",
                params![cwd, key],
                |r| r.get(0),
            )
            .optional()
            .map_err(Into::into)
        })
    }
    pub fn put(&self, cwd: &str, key: &str, value: &str, now: &str) -> Result<(), StoreError> {
        serde_json::from_str::<serde_json::Value>(value)
            .map_err(|e| StoreError::Refusal(format!("invalid setting JSON: {e}")))?;
        self.store.transaction(|tx|{tx.execute("INSERT INTO cwd_settings(cwd,key,value,created_at,updated_at) VALUES(?,?,?,?,?) ON CONFLICT(cwd,key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",params![cwd,key,value,now,now])?;Ok(())})
    }
    pub fn remove(&self, cwd: &str, key: &str) -> Result<bool, StoreError> {
        self.store.transaction(|tx| {
            Ok(tx.execute(
                "DELETE FROM cwd_settings WHERE cwd=? AND key=?",
                params![cwd, key],
            )? != 0)
        })
    }
}

pub struct Spend<'a> {
    store: &'a Store,
}
impl Spend<'_> {
    pub fn totals(
        &self,
        since: &str,
        _now: &str,
        window_ms: u64,
    ) -> Result<SpendTotals, StoreError> {
        self.store.with_connection(|c| {
            let (cost, tokens, unpriced): (f64, u64, u64) = c.query_row(
                "SELECT COALESCE(SUM(cost_usd), 0),
                        COALESCE(SUM(COALESCE(tokens_in, 0) + COALESCE(tokens_out, 0)), 0),
                        COALESCE(SUM(CASE WHEN cost_usd IS NULL
                                               AND state IN ('completed', 'failed', 'blocked', 'cancelled')
                                          THEN 1 ELSE 0 END), 0)
                   FROM tasks
                  WHERE spend_at >= ?",
                [since],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            Ok(SpendTotals {
                cost_usd: cost,
                tokens,
                since: since.to_string(),
                window_ms,
                unpriced_tasks: unpriced,
            })
        })
    }
    pub fn record(
        &self,
        task_id: &str,
        cost: f64,
        tokens_in: u64,
        tokens_out: u64,
        at: &str,
    ) -> Result<bool, StoreError> {
        self.store.transaction(|tx| {
            Ok(tx.execute(
                "UPDATE tasks SET cost_usd=?,tokens_in=?,tokens_out=?,spend_at=? WHERE id=?",
                params![cost, tokens_in, tokens_out, at, task_id],
            )? != 0)
        })
    }
}

impl<'a> Repositories<'a> {
    pub fn follow_up(
        &self,
        task_id: &str,
        instruction: &str,
        created_at: &str,
    ) -> Result<i64, StoreError> {
        self.store.transaction(|tx| {
            tx.execute(
                "INSERT INTO task_follow_ups(task_id,instruction,created_at) VALUES(?,?,?)",
                params![task_id, instruction, created_at],
            )?;
            Ok(tx.last_insert_rowid())
        })
    }
    pub fn follow_ups(&self, task_id: &str) -> Result<Vec<String>, StoreError> {
        self.store.with_connection(|c| {
            let mut s =
                c.prepare("SELECT instruction FROM task_follow_ups WHERE task_id=? ORDER BY id")?;
            Ok(s.query_map([task_id], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })
    }
    pub fn dependency(&self, task_id: &str, blocker_id: &str) -> Result<(), StoreError> {
        self.store.transaction(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO task_dependencies(task_id,blocker_id) VALUES(?,?)",
                params![task_id, blocker_id],
            )?;
            Ok(())
        })
    }
    pub fn remove_dependency(&self, task_id: &str, blocker_id: &str) -> Result<bool, StoreError> {
        self.store.transaction(|tx| {
            Ok(tx.execute(
                "DELETE FROM task_dependencies WHERE task_id=? AND blocker_id=?",
                params![task_id, blocker_id],
            )? != 0)
        })
    }
}
