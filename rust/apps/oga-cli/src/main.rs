use std::{
    collections::{BTreeMap, HashSet},
    env,
    path::{Path, PathBuf},
    process,
    sync::Arc,
    time::Duration,
};

use chrono::{Local, SecondsFormat, TimeZone, Utc};
use oga_client::{
    CompletionRequest, EventFrame, EventStreamQuery, LoopbackClient, MapInitRequest, MapQuery,
    QueryRequest, ResumeRequest, StateQuery,
};
use oga_config::{
    DEFAULT_WORKER_PROMPT, canonical_cwd, global_cwd, load_config_layers, load_profiles,
    mask_secret_env, read_config_file, read_worker_prompt, update_config_file,
};
use oga_context::{ContextIndex, LearnRouteProposal};
use oga_domain::{
    ArchivedFilter, BatchFrame, BatchTask, CleanupPlan, CleanupResult, CleanupSettings, EventKind,
    HelloPayload, InFlightTask, MCP_CONTRACT_VERSION, ModelInfo, ModelInfoSource, ModelQuery,
    Profile, Provider, Task, TaskClass, TaskEvent, TaskState, TaskSummary, TaskWorktree, VERSION,
};
use oga_events::{EventSocketOptions, SocketError, event_socket_path, start_event_socket};
use oga_http::HttpState;
use oga_mcp::{McpServer, serve_stdio};
use oga_routing::{
    RoutingPolicy, claude_models, format_rfc3339_ms, now_ms, routing_policy_from_layers,
    unoffered_policy_rules, unoffered_rule_message,
};
use oga_service::reconcile::ReconcileTrigger;
use oga_store::Store;
use oga_worktree::{remove_task_worktree, worktree_has_uncommitted_work, worktrees_root};
use rusqlite::{Row, params};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, UnixStream},
    time::{sleep, timeout},
};

const DEFAULT_PORT: u16 = 7331;
const DEFAULT_WATCH_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_WATCH_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_EVENT_OUTCOME: usize = 200;
const MAX_EVENT_TITLE: usize = 80;
const DEFAULT_CLEANUP_DAYS: u64 = 30;
const MIN_CLEANUP_DAYS: u64 = 1;
const MAX_CLEANUP_DAYS: u64 = 3_650;
const FIRST_CLEANUP_PASS: Duration = Duration::from_secs(5 * 60);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const HOLD_SWEEP_INTERVAL: Duration = Duration::from_secs(30);
/// How often the broker checks whether the host was suspended under it.
const WAKE_WATCH_INTERVAL: Duration = Duration::from_secs(30);
const LOVE_USAGE: &str = "Usage: oga love                                   what this project sends unnamed work to\n       oga love <worker>:<model>                  send all of it there from now on\n       oga love <worker>:<model>:<effort>         also choose reasoning effort\n       oga love <worker>:<model> --when <kinds>   send only those kinds of work there\n       oga love --clear                           go back to choosing per task\n       oga love --clear --when <kinds>            drop the rule for those kinds\n       oga love ... --global                      the same, for every project\n\nKinds are context, mechanical, build, reasoning, general, comma-separated.";
type CliResult<T> = Result<T, CliError>;

#[derive(Debug, Error)]
#[error("{0}")]
struct CliError(String);

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<oga_client::ClientError> for CliError {
    fn from(error: oga_client::ClientError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<oga_config::ConfigError> for CliError {
    fn from(error: oga_config::ConfigError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<oga_context::ContextError> for CliError {
    fn from(error: oga_context::ContextError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<oga_store::StoreError> for CliError {
    fn from(error: oga_store::StoreError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<SocketError> for CliError {
    fn from(error: SocketError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

#[tokio::main]
async fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let result = run(args).await;
    let code = match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            2
        }
    };
    if code != 0 {
        process::exit(code);
    }
}

async fn run(args: Vec<String>) -> CliResult<i32> {
    let Some(command) = args.first().map(String::as_str) else {
        println!("{}", help_text());
        return Ok(0);
    };

    match command {
        "serve" | "--stdio" => {
            run_serve(&args[1..], command == "--stdio").await?;
            Ok(0)
        }
        "watch" => run_watch(&args[1..]).await,
        "tail" => run_tail(&args[1..]).await,
        "query" => run_query(&args[1..]).await,
        "relearn" => run_relearn(&args[1..]).await,
        "love" => run_love(&args[1..]).await,
        "inflight" => run_inflight(),
        "tasks" => run_tasks(&args[1..]).await,
        "inspect" => run_inspect(&args[1..]).await,
        "archive" => run_archive(&args[1..], true).await,
        "restore" => run_archive(&args[1..], false).await,
        "cancel" => run_cancel(&args[1..]).await,
        "resume" => run_resume(&args[1..]).await,
        "complete" => run_complete(&args[1..]).await,
        "cleanup" => run_cleanup(&args[1..]).await,
        "config" => run_config(&args[1..]).await,
        "version" => run_version(),
        "help" | "--help" | "-h" => {
            println!("{}", help_text());
            Ok(0)
        }
        other => {
            eprintln!("{}", unknown_command_message(other));
            Ok(2)
        }
    }
}

fn database_path() -> PathBuf {
    env::var_os("OGA_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| global_cwd().join(".oga").join("oga.db"))
}

fn broker_port() -> u16 {
    env::var("OGA_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

fn broker_client() -> CliResult<LoopbackClient> {
    Ok(LoopbackClient::from_env()?)
}

async fn run_serve(args: &[String], command_is_stdio: bool) -> CliResult<()> {
    let mut port = broker_port();
    let mut stdio = command_is_stdio;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--stdio" => stdio = true,
            "--port" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| CliError::new("--port needs a value"))?;
                port = parse_port(value)?;
            }
            value if value.starts_with("--port=") => {
                port = parse_port(value.trim_start_matches("--port="))?;
            }
            value => return Err(CliError::new(format!("unknown option: {value}"))),
        }
        index += 1;
    }

    let path = database_path();
    let store = Arc::new(Store::open_writable(&path)?);
    let state = HttpState::new(store.clone()).with_build(build_stamp());
    let app = oga_http::router(state.clone()).merge(oga_mcp::router(state.clone()));
    let socket = start_event_socket(
        store.clone(),
        EventSocketOptions::new(
            event_socket_path(&path),
            HelloPayload {
                version: VERSION.to_owned(),
                mcp_contract_version: MCP_CONTRACT_VERSION,
                initial_cursor: None,
                stream_floor: None,
                stale: None,
            },
        ),
    )?;
    // Before anything is served: the rows still reading `running` belong to a
    // broker that is gone, and every surface reads the row.
    match state.dispatcher.reconcile(ReconcileTrigger::BrokerStart) {
        Ok(report) if report.touched() > 0 => {
            eprintln!("recovered {} interrupted runs", report.touched())
        }
        Ok(_) => {}
        Err(error) => eprintln!("restart recovery failed: {error}"),
    }
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let bound_port = listener.local_addr()?.port();
    let sweep_task = state.dispatcher.start_hold_sweep(HOLD_SWEEP_INTERVAL);
    let wake_task = state.dispatcher.start_wake_watch(WAKE_WATCH_INTERVAL);
    let cleanup_task = {
        let store = store.clone();
        tokio::spawn(async move {
            let mut announced = false;
            sleep(FIRST_CLEANUP_PASS).await;
            loop {
                match scheduled_cleanup_settings(&store) {
                    Ok(Some(settings)) if settings.enabled => {
                        if !announced {
                            eprintln!(
                                "automatic cleanup on: finished work older than {} days; {}. First pass after five minutes, then daily.",
                                settings.older_than_days,
                                if settings.archived_only {
                                    "archived work only"
                                } else {
                                    "archived and unarchived work"
                                },
                            );
                            announced = true;
                        }
                        if let Err(error) = scheduled_cleanup_pass(&store, settings).await {
                            eprintln!("cleanup failed: {error}");
                        }
                    }
                    Ok(Some(_)) | Ok(None) => announced = false,
                    Err(error) => eprintln!("cleanup settings unavailable: {error}"),
                }
                sleep(CLEANUP_INTERVAL).await;
            }
        })
    };
    if stdio {
        eprintln!("broker listening on http://127.0.0.1:{bound_port}");
        if let Some(path) = socket.path.as_ref() {
            eprintln!("event socket bound: {}", path.display());
        }
        let server = McpServer::new(state);
        tokio::spawn(async move {
            if let Err(error) = serve_stdio(server).await {
                eprintln!("stdio MCP server stopped: {error}");
            }
        });
    } else {
        println!("broker listening on http://127.0.0.1:{bound_port}");
        if let Some(path) = socket.path.as_ref() {
            println!("event socket bound: {}", path.display());
        }
    }

    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(CliError::from);
    sweep_task.abort();
    wake_task.abort();
    cleanup_task.abort();
    result
}

fn parse_port(value: &str) -> CliResult<u16> {
    value
        .parse::<u16>()
        .map_err(|_| CliError::new(format!("not a port: {value}")))
}

fn build_stamp() -> String {
    env::var("OGA_BUILD_STAMP")
        .ok()
        .or_else(|| option_env!("OGA_BUILD_STAMP").map(str::to_owned))
        .unwrap_or_else(|| "dev".into())
}

fn run_version() -> CliResult<i32> {
    let report = oga_domain::HealthReport::ok(build_stamp(), &Default::default());
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|error| CliError::new(error.to_string()))?
    );
    Ok(0)
}

async fn run_query(args: &[String]) -> CliResult<i32> {
    if matches!(args.first().map(String::as_str), Some("--help" | "-h")) {
        println!("Usage: oga query \"<question>\" | oga query --init [--force]");
        return Ok(0);
    }

    let init = args.iter().any(|arg| arg == "--init");
    let cwd = canonical_cwd(env::current_dir()?);
    let client = broker_client()?;
    if init {
        let force = args.iter().any(|arg| arg == "--force");
        println!(
            "Indexing {}{}...",
            cwd.display(),
            if force {
                " from scratch"
            } else {
                " incrementally"
            }
        );
        let mut request = MapInitRequest::new(cwd.display().to_string());
        request.force = force;
        let response = client
            .init_map(&request)
            .await
            .map_err(|error| query_broker_error(&client, error))?;
        println!(
            "Indexed {} files, {} symbols in {}ms{}.",
            response.file_count,
            response.symbol_count,
            response.elapsed_ms,
            if response.partial { " (partial)" } else { "" }
        );
        return Ok(0);
    }

    let question = args.join(" ").trim().to_owned();
    if question.is_empty() {
        return Err(CliError::new("usage: oga query \"<question>\""));
    }
    let result = if let Some(task_id) = env::var_os("OGA_TASK_ID") {
        let task_id = task_id.to_string_lossy().into_owned();
        let task = match client.get_task(&task_id).await {
            Ok(task) => task,
            Err(error) if error.status().is_some_and(|status| status.as_u16() == 404) => {
                return Err(CliError::new(format!(
                    "unknown task: {task_id} — call tasks to list recent task ids"
                )));
            }
            Err(error) => return Err(query_broker_error(&client, error)),
        };
        if task.archived_at.is_some() {
            return Err(CliError::new(format!(
                "unknown task: {task_id} — call tasks to list recent task ids"
            )));
        }
        let request = MapQuery::new(task_id).question(question);
        client.map(&request).await.map(|response| response.markdown)
    } else {
        client
            .query(&QueryRequest::new(cwd.display().to_string(), question))
            .await
    };
    match result {
        Ok(markdown) => {
            println!("{markdown}");
            Ok(0)
        }
        Err(error) if error.status().is_some_and(|status| status.as_u16() == 404) => {
            println!(
                "Lookup is off for this project. Search the tree or read likely files directly."
            );
            Ok(0)
        }
        Err(error) => Err(query_broker_error(&client, error)),
    }
}

/// Only a transport failure means the broker is down. Every other failure
/// answered over a live connection, so reporting it as "not running" sends the
/// reader after the wrong problem.
fn query_broker_error(client: &LoopbackClient, error: oga_client::ClientError) -> CliError {
    match &error {
        oga_client::ClientError::Transport { .. } => CliError::new(format!(
            "oga broker is not running at {}; start it with 'oga serve'",
            client.base_url()
        )),
        _ => CliError::new(error.to_string()),
    }
}

async fn run_relearn_worker(args: &[String]) -> CliResult<i32> {
    if matches!(args.first().map(String::as_str), Some("--help" | "-h")) {
        println!(
            "Usage: oga relearn '[{{\"hints\":[\"<words>\"],\"path\":\"<file you read>\",\"symbol\":\"<optional symbol>\"}}]'"
        );
        return Ok(0);
    }
    let task_id = env::var("OGA_TASK_ID")
        .map_err(|_| CliError::new("relearn with JSON is available only inside an Oga worker"))?;
    let raw = args.join(" ");
    let routes = serde_json::from_str::<Vec<LearnRouteInput>>(&raw)
        .map(|routes| {
            routes
                .into_iter()
                .map(|route| LearnRouteProposal {
                    hints: route.hints,
                    path: route.path,
                    symbol: route.symbol,
                })
                .collect::<Vec<_>>()
        })
        .map_err(|_| CliError::new("relearn expects one JSON array in a worker"))?;
    let client = broker_client()?;
    let task = client.get_task(&task_id).await?;
    if task.archived_at.is_some() {
        return Err(CliError::new(format!("unknown task: {task_id}")));
    }
    if task.state != TaskState::Running {
        return Err(CliError::new(format!(
            "task is {}; save source routes while it is running",
            task.state.as_str()
        )));
    }
    let attempt = task.attempts.len() + 1;
    let store = Store::open_writable(database_path())?;
    let duplicate = store
        .repositories()
        .events()
        .list(&task.id)?
        .iter()
        .any(|event| {
            event.kind == "learn_routes"
                && event.payload.get("attempt").and_then(Value::as_u64) == Some(attempt as u64)
        });
    if duplicate {
        append_task_event(
            &store,
            &task,
            "learn_routes_duplicate",
            json!({ "attempt": attempt }),
        )?;
        return Err(CliError::new(
            "source routes were already saved for this run",
        ));
    }
    let result = ContextIndex::new(&store).learn_routes(&task, &routes)?;
    if !result.rejected.is_empty() {
        let rejected = result
            .rejected
            .iter()
            .map(|item| format!("route {}: {}", item.index + 1, item.reason))
            .collect::<Vec<_>>();
        append_task_event(
            &store,
            &task,
            "learn_routes_invalid",
            json!({
                "attempt": attempt,
                "rejected": result
                    .rejected
                    .iter()
                    .map(|item| json!({ "index": item.index, "reason": item.reason }))
                    .collect::<Vec<_>>(),
            }),
        )?;
        return Err(CliError::new(rejected.join("; ")));
    }
    append_task_event(
        &store,
        &task,
        "learn_routes",
        json!({ "attempt": attempt, "accepted": result.accepted, "empty": result.accepted == 0 }),
    )?;
    println!(
        "{}",
        if result.accepted == 0 {
            "No reusable source routes to save.".to_owned()
        } else {
            format!(
                "Saved {} source route{}.",
                result.accepted,
                if result.accepted == 1 { "" } else { "s" }
            )
        }
    );
    Ok(0)
}

async fn run_relearn(args: &[String]) -> CliResult<i32> {
    if env::var_os("OGA_TASK_ID").is_some() {
        return run_relearn_worker(args).await;
    }
    if args
        .first()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        println!(
            "Usage: oga relearn | oga relearn \"<hint words|alias>\" <path>[#<symbol>] | oga relearn '<json>'"
        );
        return Ok(0);
    }
    let cwd = canonical_cwd(".");
    let store = Store::open_writable(database_path())?;
    let index = ContextIndex::new(&store);
    if args.is_empty() {
        let result = index.reconcile(&cwd, Default::default())?;
        println!(
            "{} files refreshed, {} moved, {} removed, {} routes re-confirmed, {} routes dropped.",
            result.refreshed,
            result.moved,
            result.removed,
            result.routes_confirmed,
            result.routes_dropped
        );
        print_route_moves(&result.route_moves);
        return Ok(0);
    }
    let routes = if args.len() == 1 {
        serde_json::from_str::<Vec<LearnRouteInput>>(&args[0])?
    } else if args.len() == 2 {
        let (path, symbol) = args[1]
            .split_once('#')
            .map_or((args[1].as_str(), None), |(path, symbol)| {
                (path, Some(symbol.to_owned()))
            });
        vec![LearnRouteInput {
            hints: vec![args[0].clone()],
            path: path.to_owned(),
            symbol,
        }]
    } else {
        return Err(CliError::new(
            "usage: oga relearn \"<hint words|alias>\" <path>[#<symbol>]",
        ));
    };
    let reconciled = index.reconcile(&cwd, Default::default())?;
    print_route_moves(&reconciled.route_moves);
    for route in &routes {
        index.learn_user_route(
            &cwd,
            &LearnRouteProposal {
                hints: route.hints.clone(),
                path: route.path.clone(),
                symbol: route.symbol.clone(),
            },
        )?;
    }
    println!(
        "Saved {} source route{}.",
        routes.len(),
        if routes.len() == 1 { "" } else { "s" }
    );
    Ok(0)
}

fn print_route_moves(moves: &[oga_context::RouteMove]) {
    for route in moves {
        println!(
            "moved route: {} → {}",
            route_target(&route.from_path, route.from_symbol.as_deref()),
            route_target(&route.to_path, route.to_symbol.as_deref()),
        );
    }
}

fn route_target(path: &str, symbol: Option<&str>) -> String {
    symbol.map_or_else(|| path.to_owned(), |symbol| format!("{path}#{symbol}"))
}

#[derive(Debug, Deserialize)]
struct LearnRouteInput {
    hints: Vec<String>,
    path: String,
    symbol: Option<String>,
}

fn append_task_event(store: &Store, task: &Task, kind: &str, payload: Value) -> CliResult<()> {
    let payload = payload
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    store.repositories().events().append(&TaskEvent {
        id: 0,
        task_id: task.id.clone(),
        kind: kind.to_owned(),
        state: task.state,
        payload,
        created_at: format_rfc3339_ms(now_ms()),
        turn_id: None,
    })?;
    Ok(())
}

fn help_text() -> &'static str {
    r#"Oga hands a task to one of your other AI accounts, runs it in the background
with only the files you allow, and tells you the moment it needs you or finishes.

Usage: oga <command> [options]

  serve                Run the broker: the background service your coding
                       agents and the Oga app talk to. The app starts it for
                       you, so you rarely type this.
  watch <task-id>...   Follow a task. Prints one line the moment it asks a
                       question, fails, or finishes, then exits. Run it with no
                       id to see its options.
  tail                 Watch everything the broker records, one line per event.
                       For checking on the service itself; to follow a single
                       task, use watch.
  query "<question>"   Ask where code lives in the current project.
                       Use query --init first when the project has no index.
  love [worker:model[:effort]]  Send work that names no model to one model. Add
                       --when context,mechanical to send only those kinds of
                       work there. Run it bare to see this project's rules,
                       --clear to go back to choosing per task, --global for
                       every project.
  relearn              Refresh the map, or save source routes. Add --force to
                       rebuild it from scratch.
  inflight             List the tasks still running, so you know what stopping
                        the service would interrupt.
  tasks [options]      List today's tasks.
  inspect <task-id>    Show one task record.
  archive <task-id>... Archive tasks; restore reverses this.
  cancel <task-id>...  Cancel tasks.
  resume <task-id>     Resume a task, optionally with -m instruction.
  complete <task-id>   Mark a task complete.
  cleanup              Free the disk that old finished work is holding. Shows
                       what would go and deletes nothing until you say so.
  config [cwd]         Print the effective config for a directory — profiles,
                       models, routes, worker rules — and which file each
                       setting came from. Defaults to the current directory.
  version              Print which build of Oga this is.
  help                 Print this.

First run:

  1. oga serve &  — unless the Oga app is already running it.
  2. Connect your coding agent to http://127.0.0.1:7331/mcp. The Oga app's
     Install MCP button does this for every client it finds.
  3. Ask your agent to delegate a task, then follow it:
     oga watch <task-id> &"#
}

fn unknown_command_message(command: &str) -> String {
    format!(
        "unknown command '{command}'\nCommands: serve, watch, tail, query, relearn, love, inflight, tasks, inspect, archive, restore, cancel, resume, complete, cleanup, config, version, help. Run 'oga help' for what each one does."
    )
}

#[derive(Debug, Default)]
struct TaskCliOptions {
    json: bool,
    state: Option<TaskState>,
    archived: bool,
    limit: Option<u64>,
    instruction: Option<String>,
}

fn parse_task_options(args: &[String]) -> CliResult<(TaskCliOptions, Vec<String>)> {
    let mut options = TaskCliOptions::default();
    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => options.json = true,
            "--archived" => options.archived = true,
            "--state" => {
                index += 1;
                options.state = Some(cli_parse_task_state(
                    args.get(index)
                        .ok_or_else(|| CliError::new("--state needs a value"))?,
                )?);
            }
            value if value.starts_with("--state=") => {
                options.state = Some(cli_parse_task_state(value.trim_start_matches("--state="))?);
            }
            "--limit" => {
                index += 1;
                options.limit = Some(parse_limit(
                    args.get(index)
                        .ok_or_else(|| CliError::new("--limit needs a value"))?,
                )?);
            }
            "-m" => {
                index += 1;
                options.instruction = Some(
                    args.get(index)
                        .ok_or_else(|| CliError::new("-m needs a value"))?
                        .clone(),
                );
            }
            value if value.starts_with("--limit=") => {
                options.limit = Some(parse_limit(value.trim_start_matches("--limit="))?);
            }
            value if value.starts_with('-') => {
                return Err(CliError::new(format!("unknown option: {value}")));
            }
            value => values.push(value.to_owned()),
        }
        index += 1;
    }
    Ok((options, values))
}

fn parse_limit(value: &str) -> CliResult<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| CliError::new("limit must be a positive number"))
}

fn cli_parse_task_state(value: &str) -> CliResult<TaskState> {
    serde_json::from_value(json!(value))
        .map_err(|_| CliError::new(format!("unknown task state: {value}")))
}

async fn all_task_summaries(client: &LoopbackClient) -> CliResult<Vec<TaskSummary>> {
    Ok(client
        .get_summary(
            &StateQuery::default()
                .archived(ArchivedFilter::Include)
                .compact(true)
                .limit(2_000),
        )
        .await?
        .tasks)
}

fn task_title(task: &TaskSummary) -> &str {
    task.title
        .as_deref()
        .or(task.tldr.as_deref())
        .unwrap_or("Untitled task")
}

fn short_task_row(task: &TaskSummary) -> Value {
    json!({
        "id": &task.id[..task.id.len().min(8)],
        "state": state_name(task.state),
        "title": task_title(task),
        "cwd": Path::new(&task.cwd).file_name().and_then(|name| name.to_str()).unwrap_or(&task.cwd),
    })
}

fn state_name(state: TaskState) -> String {
    serde_json::to_value(state)
        .expect("task state is serializable")
        .as_str()
        .unwrap()
        .to_owned()
}

fn print_json(value: &Value) -> CliResult<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

async fn run_tasks(args: &[String]) -> CliResult<i32> {
    let (options, values) = parse_task_options(args)?;
    if !values.is_empty() {
        return Err(CliError::new("tasks does not take task ids"));
    }
    let client = broker_client()?;
    let midnight = Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight");
    let since = Local
        .from_local_datetime(&midnight)
        .single()
        .unwrap_or_else(Local::now)
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut tasks = all_task_summaries(&client)
        .await?
        .into_iter()
        .filter(|task| options.archived == task.archived_at.is_some())
        .filter(|task| options.state.is_none_or(|state| task.state == state))
        .filter(|task| options.state.is_some() || task.created_at >= since)
        .collect::<Vec<_>>();
    if let Some(limit) = options.limit {
        tasks.truncate(limit as usize);
    }
    let rows = Value::Array(tasks.iter().map(short_task_row).collect());
    if options.json {
        print_json(&rows)?;
    } else {
        for task in tasks {
            println!(
                "{} {} {} {}",
                &task.id[..task.id.len().min(8)],
                state_name(task.state),
                task_title(&task),
                Path::new(&task.cwd)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&task.cwd)
            );
        }
    }
    Ok(0)
}

async fn resolve_task(client: &LoopbackClient, id: &str) -> CliResult<TaskSummary> {
    let tasks = all_task_summaries(client).await?;
    let matches = tasks
        .iter()
        .filter(|task| task.id == id || task.id.starts_with(id))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [task] => Ok((*task).clone()),
        [] => Err(CliError::new(format!("unknown task: {id}"))),
        _ => Err(CliError::new(format!(
            "ambiguous task id '{id}': {}",
            matches
                .iter()
                .map(|task| &task.id)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

async fn run_inspect(args: &[String]) -> CliResult<i32> {
    let (_options, values) = parse_task_options(args)?;
    let id = values
        .first()
        .ok_or_else(|| CliError::new("usage: oga inspect <task-id> [--json]"))?;
    if values.len() != 1 {
        return Err(CliError::new("inspect takes one task id"));
    }
    let client = broker_client()?;
    let task = resolve_task(&client, id).await?;
    let value = serde_json::to_value(client.get_task(&task.id).await?)?;
    print_json(&value)?;
    Ok(0)
}

async fn run_archive(args: &[String], archived: bool) -> CliResult<i32> {
    let (options, ids) = parse_task_options(args)?;
    if ids.is_empty() {
        return Err(CliError::new("at least one task id is required"));
    }
    let client = broker_client()?;
    let mut output = Vec::new();
    for id in ids {
        let task = resolve_task(&client, &id).await?;
        let response = client.archive_task(&task.id, archived).await?;
        output.push(json!({"id": response.id, "state": response.state, "title": task_title(&task), "action": if archived { "archived" } else { "restored" }}));
    }
    if options.json {
        print_json(&Value::Array(output))?;
    } else {
        for item in output {
            println!(
                "{} {} {}",
                if archived { "Archived" } else { "Restored" },
                item["id"].as_str().unwrap_or_default(),
                item["title"].as_str().unwrap_or_default()
            );
        }
    }
    Ok(0)
}

async fn run_cancel(args: &[String]) -> CliResult<i32> {
    let (options, ids) = parse_task_options(args)?;
    if ids.is_empty() {
        return Err(CliError::new("at least one task id is required"));
    }
    let client = broker_client()?;
    let mut output = Vec::new();
    for id in ids {
        let task = resolve_task(&client, &id).await?;
        let response = client.cancel_task(&task.id, None).await?;
        output.push(json!({"id": response.id, "state": response.state, "title": task_title(&task), "action": "cancelled"}));
    }
    if options.json {
        print_json(&Value::Array(output))?;
    } else {
        for item in output {
            println!(
                "Cancelled {} {}",
                item["id"].as_str().unwrap_or_default(),
                item["title"].as_str().unwrap_or_default()
            );
        }
    }
    Ok(0)
}

async fn run_resume(args: &[String]) -> CliResult<i32> {
    let (options, values) = parse_task_options(args)?;
    let id = values
        .first()
        .ok_or_else(|| CliError::new("usage: oga resume <task-id> [-m instruction]"))?;
    if values.len() != 1 {
        return Err(CliError::new(
            "resume takes one task id and one instruction",
        ));
    }
    let client = broker_client()?;
    let task = resolve_task(&client, id).await?;
    let request = ResumeRequest {
        instruction: options.instruction,
        ..ResumeRequest::default()
    };
    let response = client.resume_task(&task.id, &request).await?;
    let value = json!({"id": response.id, "state": response.state, "title": task_title(&task), "action": "resumed"});
    if options.json {
        print_json(&value)?;
    } else {
        println!("Resumed {} {}", response.id, task_title(&task));
    }
    Ok(0)
}

async fn run_complete(args: &[String]) -> CliResult<i32> {
    let (options, values) = parse_task_options(args)?;
    let id = values
        .first()
        .ok_or_else(|| CliError::new("usage: oga complete <task-id>"))?;
    if values.len() != 1 {
        return Err(CliError::new("complete takes one task id"));
    }
    let client = broker_client()?;
    let task = resolve_task(&client, id).await?;
    let response = client
        .complete_task(&task.id, &CompletionRequest::default())
        .await?;
    let value = json!({"id": response.id, "state": response.state, "title": task_title(&task), "action": "completed"});
    if options.json {
        print_json(&value)?;
    } else {
        println!("Completed {} {}", response.id, task_title(&task));
    }
    Ok(0)
}

#[derive(Debug, Clone)]
struct WatchArgs {
    task_ids: Vec<String>,
    timeout: Duration,
    all: bool,
}

fn watch_usage() -> String {
    "usage: oga watch <taskId...> [--timeout 30m] [--all]\n  Blocks until a task asks a question, fails, is cancelled, or completes.\n  Prints JSON lines -- type \"event\" as they stream, type \"settled\" per task. Exit 0 with news, 1 on timeout, 2 on bad input.\n  By default only lifecycle events stream. --all also streams tool, command, file, and error events.".into()
}

fn parse_watch_args(args: &[String]) -> CliResult<WatchArgs> {
    let mut task_ids = Vec::new();
    let mut timeout_value = DEFAULT_WATCH_TIMEOUT;
    let mut all = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--timeout" || arg == "-t" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| CliError::new("--timeout needs a value"))?;
            timeout_value = parse_duration(value)?;
        } else if let Some(value) = arg.strip_prefix("--timeout=") {
            timeout_value = parse_duration(value)?;
        } else if arg == "--all" {
            all = true;
        } else if arg.starts_with('-') {
            return Err(CliError::new(format!("unknown option: {arg}")));
        } else {
            task_ids.push(arg.clone());
        }
        index += 1;
    }
    if task_ids.is_empty() {
        return Err(CliError::new("at least one task id is required"));
    }
    Ok(WatchArgs {
        task_ids,
        timeout: timeout_value,
        all,
    })
}

fn parse_duration(value: &str) -> CliResult<Duration> {
    let value = value.trim();
    let split = value
        .char_indices()
        .find(|(_, character)| !character.is_ascii_digit())
        .map_or((value, ""), |(index, _)| value.split_at(index));
    let number = split
        .0
        .parse::<u64>()
        .map_err(|_| CliError::new(format!("not a duration: {value}")))?;
    let multiplier = match split.1 {
        "" | "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => return Err(CliError::new(format!("not a duration: {value}"))),
    };
    let duration = Duration::from_millis(
        number
            .checked_mul(multiplier)
            .ok_or_else(|| CliError::new(format!("not a duration: {value}")))?,
    );
    if duration.is_zero() || duration > MAX_WATCH_TIMEOUT {
        return Err(CliError::new(format!("not a duration: {value}")));
    }
    Ok(duration)
}

async fn run_watch(args: &[String]) -> CliResult<i32> {
    let parsed = match parse_watch_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{}", error);
            eprintln!("{}", watch_usage());
            return Ok(2);
        }
    };
    let deadline = tokio::time::Instant::now() + parsed.timeout;
    match run_socket_watch(&parsed, deadline).await? {
        SocketWatchResult::Complete(code) => Ok(code),
        SocketWatchResult::Fallback {
            cursor,
            pending,
            settled,
            reason,
        } => {
            if reason == "event socket closed" {
                eprintln!("event socket lost: {reason}; falling back to database");
            }
            run_database_watch(&parsed, deadline, cursor, pending, settled).await
        }
    }
}

enum SocketWatchResult {
    Complete(i32),
    Fallback {
        cursor: i64,
        pending: HashSet<String>,
        settled: usize,
        reason: String,
    },
}

async fn run_socket_watch(
    args: &WatchArgs,
    deadline: tokio::time::Instant,
) -> CliResult<SocketWatchResult> {
    let path = env::var_os("OGA_SOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|| event_socket_path(database_path()));
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return Ok(SocketWatchResult::Fallback {
            cursor: 0,
            pending: args.task_ids.iter().cloned().collect(),
            settled: 0,
            reason: "the deadline expired before connecting".into(),
        });
    }
    let stream = match timeout(remaining, UnixStream::connect(&path)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            return Ok(SocketWatchResult::Fallback {
                cursor: 0,
                pending: args.task_ids.iter().cloned().collect(),
                settled: 0,
                reason: format!("{}: {error}", path.display()),
            });
        }
        Err(_) => {
            return Ok(SocketWatchResult::Fallback {
                cursor: 0,
                pending: args.task_ids.iter().cloned().collect(),
                settled: 0,
                reason: format!("connecting to {} outlasted the deadline", path.display()),
            });
        }
    };
    let (reader, mut writer) = stream.into_split();
    let subscribe = json!({
        "v": 1,
        "watch": args.task_ids,
        "afterCursor": 0,
    });
    writer
        .write_all(format!("{}\n", subscribe).as_bytes())
        .await?;
    let mut reader = BufReader::new(reader);
    let first = match read_line_until(&mut reader, deadline).await? {
        Some(line) => line,
        None => {
            return Ok(SocketWatchResult::Fallback {
                cursor: 0,
                pending: args.task_ids.iter().cloned().collect(),
                settled: 0,
                reason: "event socket closed before hello".into(),
            });
        }
    };
    let first_value: Value = serde_json::from_str(&first)
        .map_err(|error| CliError::new(format!("invalid event socket frame: {error}")))?;
    if let Some(error) = first_value.get("error").and_then(Value::as_str) {
        return Err(CliError::new(error));
    }
    let hello: oga_domain::HelloFrame = serde_json::from_value(first_value)
        .map_err(|error| CliError::new(format!("invalid event socket hello: {error}")))?;
    let mut cursor = hello.hello.initial_cursor.unwrap_or(0);
    let mut pending = args.task_ids.iter().cloned().collect::<HashSet<_>>();
    let mut settled = 0;

    loop {
        let Some(line) = read_line_until(&mut reader, deadline).await? else {
            return Ok(SocketWatchResult::Fallback {
                cursor,
                pending,
                settled,
                reason: "event socket closed".into(),
            });
        };
        let value: Value = serde_json::from_str(&line)
            .map_err(|error| CliError::new(format!("invalid event socket frame: {error}")))?;
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(CliError::new(error));
        }
        let frame: BatchFrame = serde_json::from_value(value)
            .map_err(|error| CliError::new(format!("invalid event socket batch: {error}")))?;
        process_watch_events(&frame, &mut cursor, args.all);
        if frame.has_more {
            continue;
        }
        settled += settle_watch_tasks(&frame.tasks, &mut pending);
        if pending.is_empty() {
            return Ok(SocketWatchResult::Complete(0));
        }
    }
}

async fn read_line_until<R>(
    reader: &mut BufReader<R>,
    deadline: tokio::time::Instant,
) -> CliResult<Option<String>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return Ok(None);
    }
    let mut line = String::new();
    match timeout(remaining, reader.read_line(&mut line)).await {
        Ok(Ok(0)) => Ok(None),
        Ok(Ok(_)) => Ok(Some(line.trim().to_owned())),
        Ok(Err(error)) => Err(error.into()),
        Err(_) => Ok(None),
    }
}

async fn run_database_watch(
    args: &WatchArgs,
    deadline: tokio::time::Instant,
    mut cursor: i64,
    mut pending: HashSet<String>,
    mut settled: usize,
) -> CliResult<i32> {
    let store = match Store::open_observe(database_path()) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("error: {error}");
            return Ok(2);
        }
    };
    if cursor == 0 && pending.len() == args.task_ids.len() {
        let missing = args
            .task_ids
            .iter()
            .filter(|id| {
                store
                    .repositories()
                    .tasks()
                    .get(id)
                    .ok()
                    .flatten()
                    .is_none()
            })
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            eprintln!(
                "error: unknown task: {} (searched {})",
                missing.join(", "),
                database_path().display()
            );
            return Ok(2);
        }
        cursor = latest_task_cursor(&store, &args.task_ids)?;
    }

    while !pending.is_empty() {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let events = list_watch_events(&store, cursor, &pending)?;
        for event in &events {
            cursor = cursor.max(event.id);
            if stream_event_allowed(event.kind, event.minor, args.all) {
                println!(
                    "{}",
                    serde_json::to_string(&json!({
                        "type": "event",
                        "kind": event.kind.as_str(),
                        "task": event.task_id,
                        "text": compact(&event.summary, MAX_EVENT_OUTCOME),
                    }))
                    .expect("watch event is serializable")
                );
            }
        }
        if events.is_empty() {
            let tasks = load_watch_tasks(&store, &pending)?;
            settled += settle_watch_tasks(&tasks, &mut pending);
            if pending.is_empty() {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    }
    let _ = store.close();
    Ok(if settled > 0 { 0 } else { 1 })
}

#[derive(Debug)]
struct WatchEvent {
    id: i64,
    task_id: String,
    kind: EventKind,
    minor: bool,
    summary: String,
}

fn process_watch_events(frame: &BatchFrame, cursor: &mut i64, all: bool) {
    for event in &frame.events {
        if event.id <= *cursor {
            continue;
        }
        *cursor = event.id;
        if stream_event_allowed(event.kind, event.minor.unwrap_or(false), all) {
            println!(
                "{}",
                serde_json::to_string(&json!({
                    "type": "event",
                    "kind": event.kind.as_str(),
                    "task": event.task_id,
                    "text": compact(&event.summary, MAX_EVENT_OUTCOME),
                }))
                .expect("watch event is serializable")
            );
        }
    }
    *cursor = (*cursor).max(frame.cursor);
}

fn stream_event_allowed(kind: EventKind, minor: bool, all: bool) -> bool {
    !minor
        && if all {
            matches!(
                kind,
                EventKind::Lifecycle
                    | EventKind::Tool
                    | EventKind::Command
                    | EventKind::File
                    | EventKind::Error
                    | EventKind::Retry
            )
        } else {
            kind == EventKind::Lifecycle
        }
}

fn settle_watch_tasks(tasks: &[BatchTask], pending: &mut HashSet<String>) -> usize {
    let mut settled = 0;
    for task in tasks {
        if !pending.contains(&task.id) || !task.state.settled() {
            continue;
        }
        pending.remove(&task.id);
        settled += 1;
        println!("{}", watch_line(task));
    }
    settled
}

fn watch_line(task: &BatchTask) -> String {
    let mut line = serde_json::Map::new();
    line.insert("type".into(), json!("settled"));
    line.insert("task".into(), json!(task.id));
    line.insert("state".into(), json!(task.state));
    let detail = if task.state == TaskState::NeedsInput {
        task.question.as_deref()
    } else {
        task.error.as_deref()
    };
    if let Some(detail) = detail
        .map(|value| compact(value, MAX_EVENT_OUTCOME))
        .filter(|v| !v.is_empty())
    {
        line.insert(
            if task.state == TaskState::NeedsInput {
                "question"
            } else {
                "error"
            }
            .into(),
            json!(detail),
        );
    }
    if let Some(title) = task
        .title
        .as_deref()
        .map(|value| compact(value, MAX_EVENT_TITLE))
        .filter(|v| !v.is_empty())
    {
        line.insert("title".into(), json!(title));
    }
    if task.state == TaskState::Completed
        && let Some(tldr) = task
            .tldr
            .as_deref()
            .map(|value| compact(value, MAX_EVENT_OUTCOME))
            .filter(|v| !v.is_empty())
    {
        line.insert("tldr".into(), json!(tldr));
    }
    if let Some(code) = task.code {
        line.insert("code".into(), json!(code));
    }
    if task.truncated.unwrap_or(false) {
        line.insert("truncated".into(), json!(true));
    }
    if task.more.unwrap_or(false) {
        line.insert("more".into(), json!(true));
    }
    if task.archived_at.is_some() {
        line.insert("archived".into(), json!(true));
    }
    Value::Object(line).to_string()
}

fn compact(value: &str, max: usize) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

fn latest_task_cursor(store: &Store, task_ids: &[String]) -> CliResult<i64> {
    store
        .with_connection(|connection| {
            let placeholders = std::iter::repeat_n("?", task_ids.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT COALESCE(MAX(id),0) FROM task_events WHERE task_id IN ({placeholders})"
            );
            let values = task_ids.iter().map(String::as_str).collect::<Vec<_>>();
            Ok(connection.query_row(&sql, rusqlite::params_from_iter(values), |row| row.get(0))?)
        })
        .map_err(Into::into)
}

fn list_watch_events(
    store: &Store,
    cursor: i64,
    task_ids: &HashSet<String>,
) -> CliResult<Vec<WatchEvent>> {
    if task_ids.is_empty() {
        return Ok(Vec::new());
    }
    store
        .with_connection(|connection| {
            let placeholders = std::iter::repeat_n("?", task_ids.len()).collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT id,task_id,event_type,state,payload FROM task_events WHERE id > ? AND task_id IN ({placeholders}) ORDER BY id LIMIT 100"
            );
            let mut values: Vec<&dyn rusqlite::ToSql> = vec![&cursor];
            values.extend(task_ids.iter().map(|id| id as &dyn rusqlite::ToSql));
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(values), watch_event_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(Into::into)
}

fn load_watch_tasks(store: &Store, task_ids: &HashSet<String>) -> CliResult<Vec<BatchTask>> {
    if task_ids.is_empty() {
        return Ok(Vec::new());
    }
    store
        .with_connection(|connection| {
            let placeholders = std::iter::repeat_n("?", task_ids.len()).collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT id,state,error,question,title,archived_at,tldr,completion_json FROM tasks WHERE id IN ({placeholders})"
            );
            let values = task_ids.iter().map(String::as_str).collect::<Vec<_>>();
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
                let completion = row
                    .get::<_, Option<String>>(7)?
                    .and_then(|value| serde_json::from_str::<oga_domain::TaskCompletion>(&value).ok());
                Ok(BatchTask {
                    id: row.get(0)?,
                    state: parse_task_state(&row.get::<_, String>(1)?)?,
                    question: row.get(3)?,
                    error: row.get(2)?,
                    title: row.get(4)?,
                    archived_at: row.get(5)?,
                    tldr: row.get(6)?,
                    code: completion.map(|completion| completion.code),
                    truncated: None,
                    more: None,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(Into::into)
}

fn watch_event_from_row(row: &Row<'_>) -> rusqlite::Result<WatchEvent> {
    let state = parse_task_state(&row.get::<_, String>(3)?)?;
    let payload = serde_json::from_str::<BTreeMap<String, Value>>(&row.get::<_, String>(4)?)
        .unwrap_or_default();
    let kind = payload
        .get("kind")
        .and_then(Value::as_str)
        .and_then(parse_event_kind)
        .unwrap_or(EventKind::Raw);
    let summary = payload
        .get("detail")
        .and_then(Value::as_str)
        .or_else(|| payload.get("title").and_then(Value::as_str))
        .or_else(|| payload.get("text").and_then(Value::as_str))
        .unwrap_or_else(|| {
            row.get_ref(2)
                .ok()
                .and_then(|v| v.as_str().ok())
                .unwrap_or("event")
        })
        .to_owned();
    Ok(WatchEvent {
        id: row.get(0)?,
        task_id: row.get(1)?,
        kind,
        minor: payload
            .get("minor")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        summary: if state.settled() && summary == "event" {
            state.as_str().to_owned()
        } else {
            summary
        },
    })
}

fn parse_task_state(value: &str) -> rusqlite::Result<TaskState> {
    serde_json::from_str(&format!("\"{value}\"")).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn parse_event_kind(value: &str) -> Option<EventKind> {
    Some(match value {
        "lifecycle" => EventKind::Lifecycle,
        "message" => EventKind::Message,
        "reasoning" => EventKind::Reasoning,
        "tool" => EventKind::Tool,
        "command" => EventKind::Command,
        "file" => EventKind::File,
        "error" => EventKind::Error,
        "usage" => EventKind::Usage,
        "raw" => EventKind::Raw,
        "retry" => EventKind::Retry,
        _ => return None,
    })
}

fn tail_usage() -> &'static str {
    "usage: oga tail [--task <id>]... [--kinds a,b] [--agents] [--detail] [--cursor <n>]\n  Prints one JSON line per event as the broker records it, and keeps printing.\n  Starts from the beginning of the log unless --cursor names a later point; --detail adds each event's body.\n  Exit 0 when the broker closes the stream, 1 if it refused or could not be reached, 2 on bad input."
}

#[derive(Debug, Default)]
struct TailArgs {
    tasks: Vec<String>,
    kinds: Vec<EventKind>,
    agents: bool,
    detail: bool,
    cursor: i64,
}

fn parse_tail_args(args: &[String]) -> CliResult<TailArgs> {
    let mut parsed = TailArgs::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--agents" => parsed.agents = true,
            "--detail" => parsed.detail = true,
            "--task" => {
                index += 1;
                parsed.tasks.push(
                    args.get(index)
                        .ok_or_else(|| CliError::new("--task needs a value"))?
                        .clone(),
                );
            }
            "--kinds" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| CliError::new("--kinds needs a value"))?;
                for kind in value
                    .split(',')
                    .map(str::trim)
                    .filter(|kind| !kind.is_empty())
                {
                    parsed.kinds.push(parse_event_kind(kind).ok_or_else(|| {
                        CliError::new(format!(
                            "not an event kind: {kind} (expected one of lifecycle, message, reasoning, tool, command, file, error, usage, raw, retry)"
                        ))
                    })?);
                }
            }
            "--cursor" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| CliError::new("--cursor needs a value"))?;
                parsed.cursor = value
                    .parse::<i64>()
                    .ok()
                    .filter(|cursor| *cursor >= 0)
                    .ok_or_else(|| CliError::new(format!("not a cursor: {value}")))?;
            }
            value => return Err(CliError::new(format!("unknown option: {value}"))),
        }
        index += 1;
    }
    Ok(parsed)
}

async fn run_tail(args: &[String]) -> CliResult<i32> {
    let parsed = match parse_tail_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{}", error);
            eprintln!("{}", tail_usage());
            return Ok(2);
        }
    };
    let client = match broker_client() {
        Ok(client) => client,
        Err(error) => {
            eprintln!("{error}");
            return Ok(1);
        }
    };
    let mut query = EventStreamQuery::new(parsed.cursor).agents(parsed.agents);
    for task in parsed.tasks {
        query = query.task(task);
    }
    for kind in parsed.kinds {
        query = query.kind(kind);
    }
    let mut stream = match client.event_stream(query).await {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!(
                "could not reach the broker at {}. Start it with: oga serve",
                client.base_url()
            );
            eprintln!("{error}");
            return Ok(1);
        }
    };
    loop {
        let frame = match stream.next_once().await {
            Ok(Some(frame)) => frame,
            Ok(None) => return Ok(0),
            Err(error) => {
                eprintln!("the broker refused the stream: {error}");
                return Ok(1);
            }
        };
        match frame {
            EventFrame::Ready(frame) => print_tail_frame(
                "ready",
                json!({
                    "version": frame.version,
                    "cursor": frame.cursor,
                    "streamFloor": frame.stream_floor,
                    "tasks": frame.tasks,
                    "kinds": frame.kinds,
                    "agents": frame.agents,
                    "stale": frame.stale,
                }),
            ),
            EventFrame::Task(pointer) => {
                let mut value = serde_json::to_value(&pointer)?;
                if parsed.detail {
                    let details = client
                        .get_task_events(
                            &pointer.task_id,
                            &oga_client::TaskEventsQuery::default()
                                .after(pointer.id - 1)
                                .limit(1),
                        )
                        .await
                        .ok()
                        .and_then(|page| {
                            page.events.into_iter().find(|event| event.id == pointer.id)
                        });
                    if let Some(object) = value.as_object_mut()
                        && let Some(details) = details
                    {
                        object.insert("detail".into(), serde_json::to_value(details)?);
                    }
                }
                print_tail_frame("task", value);
            }
            EventFrame::Cursor(frame) => {
                print_tail_frame("cursor", json!({ "cursor": frame.cursor }))
            }
            EventFrame::Keepalive(frame) => {
                print_tail_frame("keepalive", json!({ "cursor": frame.cursor }))
            }
            EventFrame::Unknown { event, data } => print_tail_frame(&event, data),
        }
    }
}

fn print_tail_frame(name: &str, value: Value) {
    let mut frame = serde_json::Map::new();
    frame.insert("frame".into(), json!(name));
    if let Some(object) = value.as_object() {
        frame.extend(object.clone());
    } else {
        frame.insert("data".into(), value);
    }
    println!("{}", Value::Object(frame));
}

fn run_inflight() -> CliResult<i32> {
    let store = match Store::open_observe(database_path()) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("error: {error}");
            return Ok(2);
        }
    };
    let tasks = list_inflight(&store)?;
    println!("{}", inflight_report(&tasks));
    let code = if tasks.is_empty() { 0 } else { 1 };
    let _ = store.close();
    Ok(code)
}

fn list_inflight(store: &Store) -> CliResult<Vec<InFlightTask>> {
    store
        .with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,state,title,tldr,worker_json FROM tasks WHERE kind='delegated' AND state IN ('queued','running') ORDER BY updated_at DESC",
            )?;
            let rows = statement.query_map([], |row| {
                let state = parse_task_state(&row.get::<_, String>(1)?)?;
                let title = row.get::<_, Option<String>>(2)?.or(row.get(3)?);
                let pid = row
                    .get::<_, Option<String>>(4)?
                    .and_then(|worker| serde_json::from_str::<Value>(&worker).ok())
                    .and_then(|worker| worker.get("pid").and_then(Value::as_u64))
                    .and_then(|pid| u32::try_from(pid).ok());
                Ok(InFlightTask {
                    id: row.get(0)?,
                    state,
                    title,
                    pid,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(Into::into)
}

fn inflight_report(tasks: &[InFlightTask]) -> String {
    if tasks.is_empty() {
        return "No tasks in flight.".into();
    }
    let mut lines = vec![format!(
        "{} task{} in flight. Restarting the broker stops {}:",
        tasks.len(),
        if tasks.len() == 1 { "" } else { "s" },
        if tasks.len() == 1 { "it" } else { "them" }
    )];
    lines.extend(tasks.iter().map(|task| {
        format!(
            "  {}  {}{}{}",
            task.id,
            task.state.as_str(),
            task.pid
                .map_or_else(|| "  no worker".into(), |pid| format!("  pid {pid}")),
            task.title
                .as_deref()
                .map_or_else(String::new, |title| format!("  {title}")),
        )
    }));
    lines.extend([
        "".into(),
        "Workers that outlive the broker are stopped and recorded `cancelled`; their".into(),
        "provider sessions are kept, so `resume` continues them where they left off.".into(),
    ]);
    lines.join("\n")
}

fn cleanup_usage() -> &'static str {
    "usage: oga cleanup [--older-than <days>] [--delete]\n\n  Reports what would be permanently deleted and deletes nothing. Add --delete\n  to actually remove it.\n\n  --older-than <days>  How long finished work keeps its activity. 30 or 30d.\n                       Defaults to 30 days for a preview; --delete requires it.\n  --delete             Delete, permanently. There is nothing to restore from.\n\n  Only tasks that have finished and that you archived are ever eligible.\n  Work that is running or waiting on you and project memories are never deleted."
}

#[derive(Debug, Clone, Copy)]
struct CleanupArgs {
    older_than_days: u64,
    chosen_days: bool,
    execute: bool,
}

fn parse_cleanup_args(args: &[String]) -> CliResult<CleanupArgs> {
    let mut parsed = CleanupArgs {
        older_than_days: DEFAULT_CLEANUP_DAYS,
        chosen_days: false,
        execute: false,
    };
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--delete" {
            parsed.execute = true;
        } else if let Some(value) = arg.strip_prefix("--older-than=") {
            parsed.older_than_days = parse_cleanup_days(value)?;
            parsed.chosen_days = true;
        } else if arg == "--older-than" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| CliError::new("--older-than needs a value"))?;
            parsed.older_than_days = parse_cleanup_days(value)?;
            parsed.chosen_days = true;
        } else {
            return Err(CliError::new(format!("unknown option: {arg}")));
        }
        index += 1;
    }
    if parsed.execute && !parsed.chosen_days {
        return Err(CliError::new(
            "--delete needs --older-than <days>, so the retention is one you chose",
        ));
    }
    Ok(parsed)
}

fn parse_cleanup_days(value: &str) -> CliResult<u64> {
    let raw = value.trim();
    let normalized = raw.strip_suffix('d').unwrap_or(raw);
    let days = normalized
        .parse::<u64>()
        .ok()
        .filter(|days| (MIN_CLEANUP_DAYS..=MAX_CLEANUP_DAYS).contains(days))
        .ok_or_else(|| {
            CliError::new(format!(
                "--older-than must be a whole number of days from {MIN_CLEANUP_DAYS} to {MAX_CLEANUP_DAYS}: {raw}"
            ))
        })?;
    Ok(days)
}

fn scheduled_cleanup_days() -> CliResult<Option<u64>> {
    let Some(value) = env::var_os("OGA_CLEANUP_DAYS") else {
        return Ok(None);
    };
    let value = value.to_string_lossy();
    parse_cleanup_days(&value)
        .map(Some)
        .map_err(|_| {
            CliError::new(format!(
                "OGA_CLEANUP_DAYS must be a whole number of days from {MIN_CLEANUP_DAYS} to {MAX_CLEANUP_DAYS}, got: {value}"
            ))
        })
}

fn scheduled_cleanup_settings(store: &Store) -> CliResult<Option<CleanupSettings>> {
    let cwd = global_cwd().display().to_string();
    let stored = store.repositories().settings().get(&cwd, "cleanup")?;
    if let Some(raw) = stored {
        let settings = serde_json::from_str::<CleanupSettings>(&raw)
            .map_err(|error| CliError::new(format!("invalid cleanup settings: {error}")))?;
        if !(MIN_CLEANUP_DAYS..=MAX_CLEANUP_DAYS).contains(&settings.older_than_days) {
            return Err(CliError::new("cleanup age must be between 1 and 3650 days"));
        }
        return Ok(Some(settings));
    }
    Ok(scheduled_cleanup_days()?.map(|days| CleanupSettings {
        enabled: true,
        older_than_days: days,
        archived_only: true,
    }))
}

async fn scheduled_cleanup_pass(store: &Store, settings: CleanupSettings) -> CliResult<()> {
    let cutoff =
        format_rfc3339_ms(now_ms().saturating_sub((settings.older_than_days * 86_400_000) as i64));
    let finished_at = format_rfc3339_ms(now_ms());
    let result = store.cleanup(&cutoff, settings.archived_only, &finished_at)?;
    if result.record.plan.events > 0 {
        eprintln!(
            "cleanup: removed {} activity records from {} tasks; file {} to {}",
            result.record.plan.events,
            result.record.plan.tasks,
            format_bytes(result.file_bytes_before),
            format_bytes(result.file_bytes_after),
        );
    }
    Ok(())
}

async fn run_cleanup(args: &[String]) -> CliResult<i32> {
    let parsed = match parse_cleanup_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{}\n\n{}", error, cleanup_usage());
            return Ok(2);
        }
    };
    let cutoff = format_rfc3339_ms(now_ms() - (parsed.older_than_days * 86_400_000) as i64);
    let path = database_path();
    if !parsed.execute {
        let store = match Store::open_maintenance(&path) {
            Ok(store) => store,
            Err(error) => {
                eprintln!("error: {error}");
                return Ok(2);
            }
        };
        let plan = cleanup_plan(&store, &cutoff)?;
        let worktrees = cleanup_worktrees(&store, &cutoff)?;
        let uncommitted = count_uncommitted_worktrees(&worktrees).await?;
        let removable = (worktrees.len() as u64).saturating_sub(uncommitted);
        let kept = count_kept_worktrees(&store, &worktrees)?;
        println!(
            "{}",
            cleanup_preview(
                &plan,
                parsed.older_than_days,
                parsed.chosen_days,
                removable as u64,
                kept,
                uncommitted,
            )
        );
        let _ = store.close();
        return Ok(0);
    }

    let store = Store::open_maintenance(&path)?;
    let worktrees = cleanup_worktrees(&store, &cutoff)?;
    let (removed_worktrees, uncommitted) = remove_cleanup_worktrees(&store, &worktrees).await?;
    let kept = count_kept_worktrees(&store, &worktrees)?;
    let result = delete_cleanup_activity(&store, &cutoff)?;
    println!(
        "{}",
        cleanup_deleted(
            &result,
            parsed.older_than_days,
            removed_worktrees,
            kept,
            uncommitted,
        )
    );
    let _ = store.close();
    Ok(0)
}

const ELIGIBLE_TASKS: &str = "SELECT parent.id FROM tasks parent WHERE parent.state IN ('completed','failed','cancelled') AND parent.archived_at IS NOT NULL AND parent.updated_at < ? AND NOT EXISTS (SELECT 1 FROM tasks child WHERE child.parent_task_id = parent.id AND NOT (child.state IN ('completed','failed','cancelled') AND child.archived_at IS NOT NULL AND child.updated_at < ?))";

struct CleanupWorktree {
    task_id: String,
    worktree: TaskWorktree,
}

fn cleanup_worktrees(store: &Store, cutoff: &str) -> CliResult<Vec<CleanupWorktree>> {
    store
        .with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT id,origin_cwd,worktree_path,worktree_branch,worktree_links_json FROM tasks WHERE id IN ({ELIGIBLE_TASKS}) AND worktree_path IS NOT NULL"
            ))?;
            let rows = statement
                .query_map(params![cutoff, cutoff], |row| {
                    let links = row
                        .get::<_, Option<String>>(4)?
                        .map(|raw| {
                            serde_json::from_str::<Vec<String>>(&raw).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    4,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })
                        })
                        .transpose()?;
                    Ok(CleanupWorktree {
                        task_id: row.get(0)?,
                        worktree: TaskWorktree {
                            origin_cwd: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                            path: row.get::<_, String>(2)?,
                            branch: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                            links,
                        },
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows
                .into_iter()
                .filter(|entry| Path::new(&entry.worktree.path).exists())
                .collect())
        })
        .map_err(Into::into)
}

fn active_checkout(store: &Store, path: &str) -> CliResult<bool> {
    store
        .with_connection(|connection| {
            let count = connection.query_row(
                "SELECT COUNT(*) FROM tasks WHERE worktree_path=? AND state NOT IN ('completed','failed','cancelled')",
                [path],
                |row| row.get::<_, u64>(0),
            )?;
            Ok(count > 0)
        })
        .map_err(Into::into)
}

async fn count_uncommitted_worktrees(worktrees: &[CleanupWorktree]) -> CliResult<u64> {
    let mut count = 0;
    for entry in worktrees {
        if Path::new(&entry.worktree.path).exists()
            && worktree_has_uncommitted_work(&entry.worktree)
                .await
                .map_err(|error| CliError::new(error.to_string()))?
        {
            count += 1;
        }
    }
    Ok(count)
}

async fn remove_cleanup_worktrees(
    store: &Store,
    worktrees: &[CleanupWorktree],
) -> CliResult<(u64, u64)> {
    let mut removed = 0;
    let mut uncommitted = 0;
    for entry in worktrees {
        if !Path::new(&entry.worktree.path).exists()
            || active_checkout(store, &entry.worktree.path)?
        {
            continue;
        }
        if worktree_has_uncommitted_work(&entry.worktree)
            .await
            .map_err(|error| CliError::new(error.to_string()))?
        {
            uncommitted += 1;
            continue;
        }
        remove_task_worktree(&entry.worktree)
            .await
            .map_err(|error| CliError::new(error.to_string()))?;
        removed += 1;
    }
    Ok((removed, uncommitted))
}

fn count_kept_worktrees(store: &Store, eligible: &[CleanupWorktree]) -> CliResult<u64> {
    let eligible = eligible
        .iter()
        .map(|entry| entry.task_id.as_str())
        .collect::<HashSet<_>>();
    store
        .with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT id,worktree_path FROM tasks WHERE worktree_path IS NOT NULL")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut count = 0;
            for row in rows {
                let (task_id, path) = row?;
                if !eligible.contains(task_id.as_str()) && Path::new(&path).exists() {
                    count += 1;
                }
            }
            Ok(count)
        })
        .map_err(Into::into)
}

fn cleanup_plan(store: &Store, cutoff: &str) -> CliResult<CleanupPlan> {
    store.cleanup_plan(cutoff, true).map_err(Into::into)
}

fn delete_cleanup_activity(store: &Store, cutoff: &str) -> CliResult<CleanupResult> {
    let finished_at = format_rfc3339_ms(now_ms());
    store
        .cleanup(cutoff, true, &finished_at)
        .map_err(Into::into)
}

const CLEANUP_KEPT: &str = "Each of those tasks keeps its title, prompt, result, cost and lineage, and\nstill lists and opens as before. Only the step-by-step activity of the runs\ngoes. Project memories are never touched.";

fn cleanup_preview(
    plan: &CleanupPlan,
    days: u64,
    chosen: bool,
    worktrees: u64,
    kept: u64,
    uncommitted: u64,
) -> String {
    let age = format!("{days} day{}", if days == 1 { "" } else { "s" });
    if plan.events == 0 {
        let mut report = format!(
            "Nothing has been deleted, and nothing would be.\n\n{}",
            cleanup_nothing(plan, &age)
        );
        report.push_str(&worktree_lines(
            worktrees,
            kept,
            uncommitted,
            "Also removes",
        ));
        return report;
    }
    let mut report = format!(
        "Nothing has been deleted. This is what would go at {age}{}.\n\nFinished and archived, untouched for {age} (before {}):\n\n  {}   {}\n  {}, about {}\n\n{}",
        if chosen { "" } else { ", the default" },
        &plan.cutoff[..10.min(plan.cutoff.len())],
        count_label(plan.tasks, "task"),
        cleanup_states(plan),
        count_label(plan.events, "activity record"),
        format_bytes(plan.bytes),
        CLEANUP_KEPT,
    );
    if plan.held_back > 0 {
        report.push_str(&format!(
            "\n\nHolding back {} that fanned work out to runs that have not\nfinished, so each batch keeps the task it started from.",
            count_label(plan.held_back, "task")
        ));
    }
    report.push_str(&worktree_lines(
        worktrees,
        kept,
        uncommitted,
        "Also removes",
    ));
    report.push_str(&format!(
        "\n\nTo delete it permanently, with nothing to restore from:\n  oga cleanup --older-than {days}d --delete"
    ));
    report
}

fn cleanup_deleted(
    result: &CleanupResult,
    days: u64,
    worktrees: u64,
    kept: u64,
    uncommitted: u64,
) -> String {
    let plan = &result.record.plan;
    if plan.events == 0 && worktrees == 0 {
        let age = format!("{days} day{}", if days == 1 { "" } else { "s" });
        let mut report = format!("Nothing was deleted. {}", cleanup_nothing(plan, &age));
        report.push_str(&worktree_lines(0, kept, uncommitted, "Removed"));
        return report;
    }
    let mut report = if plan.events == 0 {
        format!(
            "No activity was deleted. {}",
            cleanup_nothing(
                plan,
                &format!("{days} day{}", if days == 1 { "" } else { "s" })
            )
        )
    } else {
        format!(
            "Deleted the activity of {}: {}.\n{}. Database file {} to {}.\n\n{}",
            count_label(plan.tasks, "task"),
            cleanup_states(plan),
            count_label(plan.events, "activity record") + " removed",
            format_bytes(result.file_bytes_before),
            format_bytes(result.file_bytes_after),
            CLEANUP_KEPT,
        )
    };
    report.push_str(&worktree_lines(worktrees, kept, uncommitted, "Removed"));
    report
}

fn cleanup_nothing(plan: &CleanupPlan, age: &str) -> String {
    if plan.tasks == 0 {
        format!("No task has finished, been archived, and gone untouched for {age}.")
    } else {
        format!("The finished, archived work older than {age} has no activity left to delete.")
    }
}

fn cleanup_states(plan: &CleanupPlan) -> String {
    plan.by_state
        .iter()
        .map(|state| format!("{} {}", state.tasks, state.state.as_str()))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn count_label(value: u64, noun: &str) -> String {
    format!("{value} {noun}{}", if value == 1 { "" } else { "s" })
}

fn format_bytes(value: u64) -> String {
    if value < 1_000 {
        return format!("{value} B");
    }
    let units = ["kB", "MB", "GB"];
    let mut scaled = value as f64 / 1_000.0;
    let mut unit = 0;
    while scaled >= 1_000.0 && unit < units.len() - 1 {
        scaled /= 1_000.0;
        unit += 1;
    }
    format!("{scaled:.1} {}", units[unit])
}

fn worktree_lines(worktrees: u64, kept: u64, uncommitted: u64, verb: &str) -> String {
    let root = worktrees_root().display().to_string();
    let mut lines = Vec::new();
    if worktrees > 0 {
        lines.extend([
            String::new(),
            format!(
                "{verb} {} in {root}.",
                count_label(worktrees, "task worktree")
            ),
            "The branch each one was made on stays in its repository.".into(),
        ]);
    }
    if uncommitted > 0 {
        lines.extend([
            String::new(),
            format!(
                "Keeping {} that still {} uncommitted work.",
                count_label(uncommitted, "task worktree"),
                if uncommitted == 1 { "has" } else { "have" }
            ),
            "Commit or discard those changes to let cleanup take them.".into(),
        ]);
    }
    if kept > 0 {
        lines.extend([
            String::new(),
            format!(
                "{} stay{} in {root}.",
                count_label(kept, "other task worktree"),
                if kept == 1 { "s" } else { "" }
            ),
            "Archive those tasks to have cleanup take their worktrees too.".into(),
        ]);
    }
    if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n")
    }
}

async fn run_config(args: &[String]) -> CliResult<i32> {
    let cwd = canonical_cwd(
        args.first()
            .map_or_else(|| env::current_dir().unwrap_or_default(), PathBuf::from),
    );
    let base_profiles = Store::open_observe(database_path())
        .ok()
        .and_then(|store| {
            let profiles = store.repositories().profiles().list().ok();
            let _ = store.close();
            profiles
        })
        .unwrap_or_default();
    let layers = load_config_layers(Some(&cwd))?;
    let profiles = load_profiles(base_profiles, Some(&cwd))?;
    let project_layer = layers.project.as_ref();
    let user_layer = layers.user.as_ref();
    let policy =
        routing_policy_from_layers(&layers).map_err(|error| CliError::new(error.to_string()))?;
    let profile_values = profiles
        .profiles
        .iter()
        .map(|profile| {
            let source = profiles.sources.get(&profile.id);
            json!({
                "id": profile.id,
                "label": profile.label,
                "provider": profile.provider,
                "enabled": profile.enabled,
                "env": mask_secret_env(&profile.env),
                "capabilities": profile.capabilities,
                "model": profile.default_model,
                "source": source.map(|source| source.source.clone()).unwrap_or_else(|| "defaults".into()),
                "sources": source.map(|source| source.sources.clone()).unwrap_or_else(|| vec!["defaults".into()]),
            })
        })
        .collect::<Vec<_>>();
    let mut output = serde_json::Map::new();
    output.insert("cwd".into(), json!(cwd));
    let mut layer_values = serde_json::Map::new();
    if let Some(layer) = project_layer {
        layer_values.insert("project".into(), json!(layer.path));
    }
    if let Some(layer) = user_layer {
        layer_values.insert("user".into(), json!(layer.path));
    }
    output.insert("layers".into(), Value::Object(layer_values));
    output.insert("profiles".into(), json!(profile_values));
    if !profiles.excluded.is_empty() {
        output.insert(
            "excluded".into(),
            json!(
                profiles
                    .excluded
                    .iter()
                    .map(|profile| json!({ "id": profile.id, "disabledBy": profile.by }))
                    .collect::<Vec<_>>()
            ),
        );
    }
    output.insert(
        "routes".into(),
        config_routes_json(policy.as_ref(), &layers),
    );
    output.insert("models".into(), merged_model_overrides(&layers)?);
    output.insert(
        "worker".into(),
        worker_config_json(&layers, &stored_worker_prompts(&cwd))?,
    );
    let love = love_rules_from_layers(&layers, &cwd)?;
    if !love.is_empty() {
        output.insert("love".into(), json!(love));
    }
    let fallback_models = configured_models(&profiles.profiles);
    let offered_models = match broker_client() {
        Ok(client) => {
            let query = ModelQuery {
                cwd: Some(cwd.display().to_string()),
                ..ModelQuery::default()
            };
            match client.models(&query).await {
                Ok(models) if !models.is_empty() => models,
                _ => fallback_models,
            }
        }
        Err(_) => fallback_models,
    };
    if let Some(policy) = policy.as_ref() {
        let warnings = unoffered_policy_rules(policy, &offered_models)
            .into_iter()
            .map(|(task_class, rule)| unoffered_rule_message(task_class, &rule))
            .collect::<Vec<_>>();
        if !warnings.is_empty() {
            output.insert("warnings".into(), json!(warnings));
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&Value::Object(output))
            .map_err(|error| CliError::new(error.to_string()))?
    );
    Ok(0)
}

fn yaml_to_json(value: &serde_yaml::Value) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

#[derive(Debug, Clone, Default)]
struct ModelOverrideSet {
    shared: BTreeMap<String, Value>,
    by_profile: BTreeMap<String, BTreeMap<String, Value>>,
}

fn merged_model_overrides(layers: &oga_config::ConfigLayers) -> CliResult<Value> {
    let user = model_overrides_for_layer(layers.user.as_ref())?;
    let project = if layers.project.as_ref().is_some_and(|layer| {
        Some(layer.path.clone()) != layers.user.as_ref().map(|layer| layer.path.clone())
    }) {
        model_overrides_for_layer(layers.project.as_ref())?
    } else {
        ModelOverrideSet::default()
    };
    let shared = merge_override_entries(&user.shared, &project.shared);
    let mut profile_ids = user
        .by_profile
        .keys()
        .chain(project.by_profile.keys())
        .cloned()
        .collect::<Vec<_>>();
    profile_ids.sort();
    profile_ids.dedup();
    let by_profile = profile_ids
        .into_iter()
        .map(|profile_id| {
            let user_entries = user
                .by_profile
                .get(&profile_id)
                .cloned()
                .unwrap_or_default();
            let project_entries = project
                .by_profile
                .get(&profile_id)
                .cloned()
                .unwrap_or_default();
            (
                profile_id,
                merge_override_entries(&user_entries, &project_entries),
            )
        })
        .collect::<BTreeMap<_, _>>();
    Ok(json!({ "shared": shared, "byProfile": by_profile }))
}

fn model_overrides_for_layer(
    layer: Option<&oga_config::ConfigLayer>,
) -> CliResult<ModelOverrideSet> {
    let Some(layer) = layer else {
        return Ok(ModelOverrideSet::default());
    };
    let Some(models) = layer.root.get("models") else {
        return Ok(ModelOverrideSet::default());
    };
    let table = models.as_mapping().ok_or_else(|| {
        CliError::new(format!(
            "invalid model config {} at models: must be a table",
            layer.path.display()
        ))
    })?;
    let mut result = ModelOverrideSet::default();
    for (raw_key, value) in table {
        let Some(key) = raw_key.as_str() else {
            continue;
        };
        let entry = value.as_mapping().ok_or_else(|| {
            CliError::new(format!(
                "invalid model config {} at models.{key}: must be a table",
                layer.path.display()
            ))
        })?;
        if !entry.is_empty() && entry.values().all(serde_yaml::Value::is_mapping) {
            let models = entry
                .iter()
                .filter_map(|(raw_model, value)| {
                    let model = raw_model.as_str()?.to_owned();
                    Some(
                        parse_model_override(
                            value,
                            &layer.path,
                            &format!("models.{key}.{}", raw_model.as_str().unwrap_or_default()),
                        )
                        .map(|parsed| (model, parsed)),
                    )
                })
                .collect::<CliResult<BTreeMap<_, _>>>()?;
            result.by_profile.insert(key.to_owned(), models);
        } else {
            result.shared.insert(
                key.to_owned(),
                parse_model_override(value, &layer.path, &format!("models.{key}"))?,
            );
        }
    }
    Ok(result)
}

fn parse_model_override(value: &serde_yaml::Value, path: &Path, field: &str) -> CliResult<Value> {
    let table = value.as_mapping().ok_or_else(|| {
        CliError::new(format!(
            "invalid model config {} at {field}: must be a table",
            path.display()
        ))
    })?;
    let mut output = Map::new();
    for (raw_key, value) in table {
        let Some(key) = raw_key.as_str() else {
            return Err(CliError::new(format!(
                "invalid model config {} at {field}: keys must be strings",
                path.display()
            )));
        };
        if !matches!(
            key,
            "enabled" | "preferred" | "loved" | "effort" | "capabilities"
        ) {
            return Err(CliError::new(format!(
                "invalid model config {} at {field}.{key}: unknown field",
                path.display()
            )));
        }
        match key {
            "enabled" | "preferred" | "loved" if !value.is_bool() => {
                return Err(CliError::new(format!(
                    "invalid model config {} at {field}.{key}: must be true or false",
                    path.display()
                )));
            }
            "effort"
                if value.as_str().is_none_or(|effort| {
                    !["minimal", "low", "medium", "high", "xhigh", "max"].contains(&effort)
                }) =>
            {
                return Err(CliError::new(format!(
                    "invalid model config {} at {field}.effort: must be one of minimal, low, medium, high, xhigh, max",
                    path.display()
                )));
            }
            "capabilities"
                if value
                    .as_sequence()
                    .is_none_or(|values| values.iter().any(|value| value.as_str().is_none())) =>
            {
                return Err(CliError::new(format!(
                    "invalid model config {} at {field}.capabilities: must be an array of strings",
                    path.display()
                )));
            }
            _ => {}
        }
        output.insert(key.to_owned(), yaml_to_json(value));
    }
    Ok(Value::Object(output))
}

fn merge_override_entries(
    user: &BTreeMap<String, Value>,
    project: &BTreeMap<String, Value>,
) -> BTreeMap<String, Value> {
    user.keys()
        .chain(project.keys())
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .map(|key| {
            let mut value = Map::new();
            if let Some(entry) = user.get(&key).and_then(Value::as_object) {
                value.extend(entry.clone());
            }
            if let Some(entry) = project.get(&key).and_then(Value::as_object) {
                value.extend(entry.clone());
            }
            (key, Value::Object(value))
        })
        .collect()
}

fn config_routes_json(policy: Option<&RoutingPolicy>, layers: &oga_config::ConfigLayers) -> Value {
    let Some(policy) = policy else {
        return json!({});
    };
    let mut sources = BTreeMap::new();
    for layer in [layers.project.as_ref(), layers.user.as_ref()]
        .into_iter()
        .flatten()
    {
        if let Some(routes) = layer
            .root
            .get("routes")
            .and_then(serde_yaml::Value::as_mapping)
        {
            for raw_key in routes.keys() {
                let Some(key) = raw_key.as_str() else {
                    continue;
                };
                sources
                    .entry(key.to_ascii_lowercase())
                    .or_insert_with(|| layer.path.display().to_string());
            }
        }
    }
    let routes = policy
        .routes
        .iter()
        .map(|(task_class, route)| {
            let mut value = Map::new();
            if let Some(preference) = route.preference {
                value.insert("preference".into(), json!(preference));
            }
            if let Some(min_quality) = route.min_quality {
                value.insert("minQuality".into(), json!(min_quality));
            }
            value.insert(
                "allow".into(),
                json!(
                    route
                        .allow
                        .iter()
                        .map(|allowed| json!({
                            "provider": allowed.provider,
                            "model": allowed.model
                        }))
                        .collect::<Vec<_>>()
                ),
            );
            value.insert(
                "source".into(),
                json!(
                    sources
                        .get(task_class.as_str())
                        .cloned()
                        .unwrap_or_else(|| policy.path.clone())
                ),
            );
            (task_class.as_str().to_owned(), Value::Object(value))
        })
        .collect::<Map<_, _>>();
    Value::Object(routes)
}

/// The worker rules a directory ends up with, in the order a dispatch resolves
/// them: its own `.oga.yaml`, what Settings saved for it, the all-projects file,
/// then what Settings saved there.
fn worker_config_json(
    layers: &oga_config::ConfigLayers,
    saved: &SavedWorkerPrompts,
) -> CliResult<Value> {
    let own = layers.project.as_ref();
    let mut worker = Map::new();
    if let Some((prompt, layer)) = read_worker_prompt(own)?.zip(own) {
        worker.insert("workerPrompt".into(), json!(prompt));
        worker.insert("source".into(), json!(layer.path));
        return Ok(Value::Object(worker));
    }
    let prompt = saved
        .here
        .clone()
        .or(read_worker_prompt(layers.user.as_ref())?)
        .or_else(|| saved.everywhere.clone())
        .unwrap_or_else(|| DEFAULT_WORKER_PROMPT.to_owned());
    worker.insert("workerPrompt".into(), json!(prompt));
    Ok(Value::Object(worker))
}

#[derive(Debug, Default)]
struct SavedWorkerPrompts {
    here: Option<String>,
    everywhere: Option<String>,
}

fn stored_worker_prompts(cwd: &Path) -> SavedWorkerPrompts {
    let Ok(store) = Store::open_observe(database_path()) else {
        return SavedWorkerPrompts::default();
    };
    let global = canonical_cwd(global_cwd());
    let read = |path: &Path| {
        store
            .repositories()
            .settings()
            .get(&path.display().to_string(), "prompts")
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .filter(|value| value.get("written").and_then(Value::as_bool) == Some(true))
            .and_then(|value| {
                value
                    .get("value")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
    };
    let saved = SavedWorkerPrompts {
        here: (canonical_cwd(cwd) != global).then(|| read(cwd)).flatten(),
        everywhere: read(&global),
    };
    let _ = store.close();
    saved
}

/// The love rules a directory ends up with: its own file's list, else the
/// all-projects one, parsed by the same reader routing uses.
fn love_rules_from_layers(
    layers: &oga_config::ConfigLayers,
    cwd: &Path,
) -> CliResult<oga_config::LoveRules> {
    let project = (canonical_cwd(cwd) != canonical_cwd(global_cwd()))
        .then(|| layers.project.clone())
        .flatten();
    read_love_rules(&oga_config::ConfigLayers {
        user: layers.user.clone(),
        project,
    })
}

/// The love rules written in the one file `oga love` is about to edit.
fn love_rules_for_scope(
    layers: &oga_config::ConfigLayers,
    cwd: &Path,
) -> CliResult<oga_config::LoveRules> {
    let layers = if canonical_cwd(cwd) == canonical_cwd(global_cwd()) {
        oga_config::ConfigLayers {
            user: layers.user.clone(),
            project: None,
        }
    } else {
        oga_config::ConfigLayers {
            user: None,
            project: layers.project.clone(),
        }
    };
    read_love_rules(&layers)
}

fn read_love_rules(layers: &oga_config::ConfigLayers) -> CliResult<oga_config::LoveRules> {
    oga_config::read_model_overrides(layers)
        .map(|(_, love)| love)
        .map_err(|error| CliError::new(error.to_string()))
}

fn configured_models(profiles: &[Profile]) -> Vec<ModelInfo> {
    profiles
        .iter()
        .filter(|profile| profile.enabled)
        .flat_map(|profile| {
            if profile.provider == Provider::Claude {
                claude_models(profile)
            } else {
                vec![ModelInfo {
                    id: profile.default_model.clone(),
                    label: profile.default_model.clone(),
                    provider: profile.provider,
                    profile_id: profile.id.clone(),
                    source: ModelInfoSource::Configured,
                    cost: None,
                    context_window: None,
                    reasoning: None,
                    efforts: None,
                    default_effort: None,
                    tool_call: None,
                }]
            }
        })
        .collect()
}

fn profiles_from_store() -> Vec<Profile> {
    let Ok(store) = Store::open_observe(database_path()) else {
        return Vec::new();
    };
    let profiles = store.repositories().profiles().list().unwrap_or_default();
    let _ = store.close();
    profiles
}

async fn run_love(args: &[String]) -> CliResult<i32> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{LOVE_USAGE}");
        return Ok(0);
    }
    let LoveArgs {
        global,
        clear,
        target,
        when,
    } = parse_love_args(args)?;
    let cwd = if global {
        global_cwd()
    } else {
        canonical_cwd(env::current_dir()?)
    };
    let snapshot = read_config_file(Some(&cwd))?;
    let layers = load_config_layers(Some(&cwd))?;
    let current = love_rules_for_scope(&layers, &cwd)?;
    let scope_label = if global {
        "in every project"
    } else {
        "in this project"
    };
    if let Some(target) = target {
        let (profile_id, rest) = target.split_once(':').ok_or_else(|| {
            CliError::new(format!(
                "name the worker and model with ':', like 'oga love claude:opus'\n{LOVE_USAGE}"
            ))
        })?;
        let (model, effort) = rest
            .rsplit_once(':')
            .filter(|(_, effort)| {
                ["minimal", "low", "medium", "high", "xhigh", "max"].contains(effort)
            })
            .map_or((rest, None), |(model, effort)| (model, Some(effort)));
        if profile_id.is_empty() || model.is_empty() {
            return Err(CliError::new(format!(
                "name the worker and model with ':', like 'oga love claude:opus'\n{LOVE_USAGE}"
            )));
        }
        let profiles = load_profiles(profiles_from_store(), Some(&cwd))?;
        let profile = profiles
            .profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| {
                let connected = profiles
                    .profiles
                    .iter()
                    .map(|profile| profile.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                CliError::new(format!(
                    "no worker called {profile_id}. Connected: {}",
                    if connected.is_empty() {
                        "none"
                    } else {
                        &connected
                    }
                ))
            })?;
        if !profile.enabled {
            return Err(CliError::new(format!(
                "{profile_id} is turned off, so nothing can be sent there. Turn it on first."
            )));
        }
        let (was_off, unlisted) = love_target_status(&cwd, profile_id, model, profile).await?;
        let replaced = replaced_rules(&current, &when);
        let next = update_love_text(
            &snapshot.text,
            &LoveEdit::Set {
                profile_id: profile_id.to_owned(),
                model: model.to_owned(),
                when: when.clone(),
                effort: effort.map(str::to_owned),
                turn_on: was_off,
            },
        );
        update_config_file(Some(&cwd), &snapshot.revision, &next)?;
        println!(
            "{profile_id}/{model} now takes {} {scope_label}.",
            work_label(&when).to_lowercase()
        );
        if let Some(effort) = effort {
            println!("It thinks at {effort} effort on that work.");
        }
        for rule in replaced
            .iter()
            .filter(|rule| !rule.names_model(profile_id, model))
        {
            println!("It replaces {}.", rule.label());
        }
        if was_off {
            println!("It was turned off here, so it is back on.");
        }
        if unlisted {
            println!(
                "{profile_id} did not list this model. Check the spelling if work sent there fails to start."
            );
        }
        println!("Written to {}.", snapshot.path.display());
        return Ok(0);
    }
    if clear {
        let dropped = cleared_rules(&current, &when);
        if dropped.is_empty() {
            match when.is_empty() {
                true => println!("Nothing is loved {scope_label}."),
                false => println!(
                    "Nothing takes {} {scope_label}.",
                    work_label(&when).to_lowercase()
                ),
            }
            return Ok(0);
        }
        let next = update_love_text(&snapshot.text, &LoveEdit::Clear { when: when.clone() });
        update_config_file(Some(&cwd), &snapshot.revision, &next)?;
        for rule in dropped {
            println!(
                "{} no longer takes {} {scope_label}.",
                rule.label(),
                work_label(if when.is_empty() { &rule.when } else { &when }).to_lowercase()
            );
        }
        println!("Oga picks a model for that work again.");
        return Ok(0);
    }
    let rules = love_rules_from_layers(&layers, &cwd)?;
    if rules.is_empty() {
        println!("Nothing is loved here, so Oga picks a model for each task.");
        println!("{LOVE_USAGE}");
        return Ok(0);
    }
    for line in love_table(&rules) {
        println!("{line}");
    }
    Ok(0)
}

#[derive(Debug, Default)]
struct LoveArgs {
    global: bool,
    clear: bool,
    target: Option<String>,
    when: Vec<TaskClass>,
}

fn parse_love_args(args: &[String]) -> CliResult<LoveArgs> {
    let mut parsed = LoveArgs::default();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--global" => parsed.global = true,
            "--clear" => parsed.clear = true,
            kinds if kinds == "--when" || kinds.starts_with("--when=") => {
                let value = match kinds.split_once('=') {
                    Some((_, value)) => Some(value.to_owned()),
                    None => rest.next().cloned(),
                };
                let value = value.ok_or_else(|| {
                    CliError::new(format!("--when needs kinds of work\n{LOVE_USAGE}"))
                })?;
                parsed.when = parse_work_kinds(&value)?;
            }
            flag if flag.starts_with('-') => {
                return Err(CliError::new(format!(
                    "unknown option '{flag}'\n{LOVE_USAGE}"
                )));
            }
            target if parsed.target.is_none() => parsed.target = Some(target.to_owned()),
            extra => {
                return Err(CliError::new(format!(
                    "'{extra}' is one model too many; love one at a time\n{LOVE_USAGE}"
                )));
            }
        }
    }
    if parsed.clear && parsed.target.is_some() {
        return Err(CliError::new(format!(
            "--clear takes no model\n{LOVE_USAGE}"
        )));
    }
    Ok(parsed)
}

fn parse_work_kinds(value: &str) -> CliResult<Vec<TaskClass>> {
    let mut kinds = Vec::new();
    for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let class = TaskClass::parse(part).ok_or_else(|| {
            CliError::new(format!(
                "there is no kind of work called '{part}'\n{LOVE_USAGE}"
            ))
        })?;
        if !kinds.contains(&class) {
            kinds.push(class);
        }
    }
    if kinds.is_empty() {
        return Err(CliError::new(format!(
            "--when needs kinds of work\n{LOVE_USAGE}"
        )));
    }
    Ok(kinds)
}

/// The rules a new rule takes work away from: the ones holding those kinds, or
/// the catch-all when no kind was named.
fn replaced_rules<'a>(
    current: &'a oga_config::LoveRules,
    when: &[TaskClass],
) -> Vec<&'a oga_config::LoveRule> {
    current
        .iter()
        .filter(|rule| match when.is_empty() {
            true => rule.when.is_empty(),
            false => rule.when.iter().any(|class| when.contains(class)),
        })
        .collect()
}

/// The rules a clear removes: the ones holding those kinds, or every rule when
/// no kind was named.
fn cleared_rules<'a>(
    current: &'a oga_config::LoveRules,
    when: &[TaskClass],
) -> Vec<&'a oga_config::LoveRule> {
    match when.is_empty() {
        true => current.iter().collect(),
        false => replaced_rules(current, when),
    }
}

/// What a kind of work is called where someone reads it, never the router's own
/// name for it.
fn work_label(when: &[TaskClass]) -> String {
    if when.is_empty() {
        return "Every other kind of work".into();
    }
    let labels = when
        .iter()
        .map(|class| match class {
            TaskClass::Context => "reading and lookups",
            TaskClass::Mechanical => "small edits",
            TaskClass::Build => "building and fixing",
            TaskClass::Reasoning => "hard thinking",
            TaskClass::General => "open-ended work",
        })
        .collect::<Vec<_>>();
    let mut label = labels.join(", ");
    label[..1].make_ascii_uppercase();
    label
}

fn love_table(rules: &oga_config::LoveRules) -> Vec<String> {
    let rows = rules
        .iter()
        .map(|rule| {
            (
                work_label(&rule.when),
                rule.label(),
                rule.effort.clone().unwrap_or_else(|| "as needed".into()),
                if rule.scope == "project" {
                    "this project"
                } else {
                    "every project"
                }
                .to_owned(),
            )
        })
        .collect::<Vec<_>>();
    let width = |column: fn(&(String, String, String, String)) -> &String, header: &str| {
        rows.iter()
            .map(|row| column(row).chars().count())
            .chain([header.chars().count()])
            .max()
            .unwrap_or_default()
    };
    let work = width(|row| &row.0, "Work");
    let model = width(|row| &row.1, "Goes to");
    let effort = width(|row| &row.2, "Thinking");
    let mut lines = vec![format!(
        "{:work$}  {:model$}  {:effort$}  Set for",
        "Work", "Goes to", "Thinking"
    )];
    lines.extend(rows.iter().map(|row| {
        format!(
            "{:work$}  {:model$}  {:effort$}  {}",
            row.0, row.1, row.2, row.3
        )
    }));
    lines
}

async fn love_target_status(
    cwd: &Path,
    profile_id: &str,
    model: &str,
    profile: &Profile,
) -> CliResult<(bool, bool)> {
    let fallback_unlisted = model != profile.default_model;
    let Ok(client) = broker_client() else {
        return Ok((false, fallback_unlisted));
    };
    let query = ModelQuery {
        cwd: Some(cwd.display().to_string()),
        profile: Some(profile_id.to_owned()),
        include_disabled: Some(true),
        ..ModelQuery::default()
    };
    let offered = client.models(&query).await.unwrap_or_default();
    let unlisted = love_target_catalog_status(profile_id, model, &offered, fallback_unlisted)?;
    let cwd_string = cwd.display().to_string();
    let was_off = client
        .model_settings(Some(&cwd_string), false)
        .await
        .ok()
        .and_then(|settings| {
            settings
                .workers
                .into_iter()
                .find(|worker| worker.id == profile_id)
        })
        .is_some_and(|worker| {
            !worker.enabled
                || worker
                    .models
                    .into_iter()
                    .find(|entry| entry.id == model)
                    .is_some_and(|entry| !entry.enabled)
        });
    Ok((was_off, unlisted))
}

fn love_target_catalog_status(
    profile_id: &str,
    model: &str,
    offered: &[ModelInfo],
    fallback_unlisted: bool,
) -> CliResult<bool> {
    let listed = offered.iter().any(|offered| offered.id == model);
    let enumerated = offered.len() > 1
        || offered
            .iter()
            .any(|offered| offered.source == ModelInfoSource::Discovered);
    if enumerated && !listed {
        return Err(CliError::new(format!(
            "{profile_id} does not offer a model called {model}."
        )));
    }
    Ok(if offered.is_empty() {
        fallback_unlisted
    } else {
        !listed
    })
}

/// What one `oga love` run does to a config file's rules.
enum LoveEdit {
    Set {
        profile_id: String,
        model: String,
        when: Vec<TaskClass>,
        effort: Option<String>,
        turn_on: bool,
    },
    Clear {
        when: Vec<TaskClass>,
    },
}

fn update_love_text(source: &str, edit: &LoveEdit) -> String {
    let mut root = serde_yaml::from_str::<serde_yaml::Value>(source)
        .ok()
        .filter(serde_yaml::Value::is_mapping)
        .unwrap_or_else(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    let mut rules = take_love_rules(&mut root);
    match edit {
        LoveEdit::Set {
            profile_id,
            model,
            when,
            effort,
            turn_on,
        } => {
            release_kinds(&mut rules, when);
            let mut rule = serde_yaml::Mapping::new();
            rule.insert("model".into(), format!("{profile_id}:{model}").into());
            if !when.is_empty() {
                rule.insert(
                    "when".into(),
                    when.iter()
                        .map(|class| class.as_str())
                        .collect::<Vec<_>>()
                        .into(),
                );
            }
            if let Some(effort) = effort {
                rule.insert("effort".into(), effort.as_str().into());
            }
            rules.push(serde_yaml::Value::Mapping(rule));
            if *turn_on {
                switch_model_on(&mut root, profile_id, model);
            }
        }
        LoveEdit::Clear { when } if when.is_empty() => rules.clear(),
        LoveEdit::Clear { when } => release_kinds(&mut rules, when),
    }
    let mapping = root.as_mapping_mut().expect("mapping root");
    if rules.is_empty() {
        mapping.remove("love");
    } else {
        mapping.insert("love".into(), serde_yaml::Value::Sequence(rules));
    }
    serde_yaml::to_string(&root).unwrap_or_else(|_| source.to_owned())
}

/// The file's rules as a list to edit, with a `models` entry still carrying the
/// old `loved` flag folded in as the catch-all rule so one write migrates it.
fn take_love_rules(root: &mut serde_yaml::Value) -> Vec<serde_yaml::Value> {
    let mut rules = root
        .get("love")
        .and_then(serde_yaml::Value::as_sequence)
        .cloned()
        .unwrap_or_default();
    let Some(models) = root
        .get_mut("models")
        .and_then(serde_yaml::Value::as_mapping_mut)
    else {
        return rules;
    };
    let mut migrated = Vec::new();
    for (raw_key, value) in models.iter_mut() {
        let Some(key) = raw_key.as_str().map(str::to_owned) else {
            continue;
        };
        if let Some(entry) = value.as_mapping_mut()
            && entry.get("loved").and_then(serde_yaml::Value::as_bool) == Some(true)
        {
            entry.remove("loved");
            migrated.push((key.clone(), entry.remove("effort")));
            continue;
        }
        let Some(profile_models) = value.as_mapping_mut() else {
            continue;
        };
        for (raw_model, value) in profile_models.iter_mut() {
            let Some(model) = raw_model.as_str().map(str::to_owned) else {
                continue;
            };
            if let Some(entry) = value.as_mapping_mut()
                && entry.get("loved").and_then(serde_yaml::Value::as_bool) == Some(true)
            {
                entry.remove("loved");
                migrated.push((format!("{key}:{model}"), entry.remove("effort")));
            }
        }
    }
    prune_empty_models(models);
    if models.is_empty() {
        root.as_mapping_mut()
            .expect("mapping root")
            .remove("models");
    }
    for (model, effort) in migrated {
        let mut rule = serde_yaml::Mapping::new();
        rule.insert("model".into(), model.into());
        if let Some(effort) = effort {
            rule.insert("effort".into(), effort);
        }
        rules.push(serde_yaml::Value::Mapping(rule));
    }
    rules
}

/// Drop the entries a migration emptied, so moving a flag out of `models`
/// leaves no `{}` behind.
fn prune_empty_models(models: &mut serde_yaml::Mapping) {
    for value in models.values_mut() {
        if let Some(entry) = value.as_mapping_mut() {
            entry.retain(|_, value| value.as_mapping().is_none_or(|table| !table.is_empty()));
        }
    }
    models.retain(|_, value| value.as_mapping().is_none_or(|table| !table.is_empty()));
}

/// Take these kinds of work away from whichever rules hold them, dropping a
/// rule left with nothing to take. No kinds means the catch-all rule gives its
/// place up to the rule being written.
fn release_kinds(rules: &mut Vec<serde_yaml::Value>, when: &[TaskClass]) {
    if when.is_empty() {
        rules.retain(|rule| rule.get("when").is_some());
        return;
    }
    for rule in rules.iter_mut() {
        let Some(kinds) = rule.get("when").and_then(serde_yaml::Value::as_sequence) else {
            continue;
        };
        let kept = kinds
            .iter()
            .filter(|kind| {
                kind.as_str()
                    .and_then(TaskClass::parse)
                    .is_none_or(|class| !when.contains(&class))
            })
            .cloned()
            .collect::<Vec<_>>();
        if let Some(map) = rule.as_mapping_mut() {
            map.insert("when".into(), serde_yaml::Value::Sequence(kept));
        }
    }
    rules.retain(|rule| {
        rule.get("when")
            .and_then(serde_yaml::Value::as_sequence)
            .is_none_or(|kinds| !kinds.is_empty())
    });
}

fn switch_model_on(root: &mut serde_yaml::Value, profile_id: &str, model: &str) {
    let mut current = root;
    for key in ["models", profile_id, model] {
        let map = current.as_mapping_mut().expect("config map");
        current = map
            .entry(key.into())
            .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    }
    if let Some(map) = current.as_mapping_mut() {
        map.insert("enabled".into(), true.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_watch_duration_and_flags() {
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1_800));
        let args = vec!["task-a".into(), "--all".into(), "--timeout=2s".into()];
        let parsed = parse_watch_args(&args).unwrap();
        assert_eq!(parsed.task_ids, ["task-a"]);
        assert!(parsed.all);
        assert_eq!(parsed.timeout, Duration::from_secs(2));
        assert!(parse_duration("0s").is_err());
    }

    #[test]
    fn parses_tail_filters() {
        let args = vec![
            "--task".into(),
            "task-a".into(),
            "--kinds".into(),
            "lifecycle,error".into(),
            "--detail".into(),
            "--cursor".into(),
            "7".into(),
        ];
        let parsed = parse_tail_args(&args).unwrap();
        assert_eq!(parsed.tasks, ["task-a"]);
        assert_eq!(parsed.kinds, [EventKind::Lifecycle, EventKind::Error]);
        assert!(parsed.detail);
        assert_eq!(parsed.cursor, 7);
    }

    #[test]
    fn cleanup_delete_requires_explicit_retention() {
        assert!(parse_cleanup_args(&["--delete".into()]).is_err());
        let parsed = parse_cleanup_args(&["--older-than=30d".into(), "--delete".into()]).unwrap();
        assert!(parsed.execute);
        assert_eq!(parsed.older_than_days, 30);
    }

    fn set(profile_id: &str, model: &str, when: &[TaskClass], effort: Option<&str>) -> LoveEdit {
        LoveEdit::Set {
            profile_id: profile_id.into(),
            model: model.into(),
            when: when.to_vec(),
            effort: effort.map(str::to_owned),
            turn_on: false,
        }
    }

    #[test]
    fn love_text_writes_one_rule_per_kind_of_work() {
        let source = "other:\n  value: 1\n";
        let next = update_love_text(
            source,
            &set(
                "opencode",
                "luna",
                &[TaskClass::Context, TaskClass::Mechanical],
                Some("low"),
            ),
        );
        let next = update_love_text(&next, &set("claude", "opus", &[], None));
        let rules = read_rules(&next);

        assert_eq!(rules.0.len(), 2);
        assert_eq!(rules.for_class(TaskClass::Context).unwrap().model, "luna");
        assert_eq!(
            rules
                .for_class(TaskClass::Context)
                .unwrap()
                .effort
                .as_deref(),
            Some("low")
        );
        assert_eq!(rules.for_class(TaskClass::Build).unwrap().model, "opus");
        assert!(next.contains("value: 1"));
    }

    #[test]
    fn love_text_takes_a_kind_over_from_the_rule_that_held_it() {
        let source = update_love_text(
            "",
            &set(
                "opencode",
                "luna",
                &[TaskClass::Context, TaskClass::Build],
                None,
            ),
        );
        let next = update_love_text(&source, &set("claude", "opus", &[TaskClass::Build], None));
        let rules = read_rules(&next);

        assert_eq!(rules.for_class(TaskClass::Context).unwrap().model, "luna");
        assert_eq!(rules.for_class(TaskClass::Build).unwrap().model, "opus");

        // Clearing one kind leaves the rest of that rule standing.
        let cleared = update_love_text(
            &next,
            &LoveEdit::Clear {
                when: vec![TaskClass::Context],
            },
        );
        let rules = read_rules(&cleared);
        assert!(rules.for_class(TaskClass::Context).is_none());
        assert_eq!(rules.for_class(TaskClass::Build).unwrap().model, "opus");

        // Clearing with no kind named takes every rule with it.
        let empty = update_love_text(&cleared, &LoveEdit::Clear { when: Vec::new() });
        assert!(read_rules(&empty).is_empty());
        assert!(!empty.contains("love"));
    }

    #[test]
    fn love_text_migrates_the_loved_flag_and_reenables_the_model() {
        let source = "models:\n  claude:\n    opus:\n      loved: true\n      effort: high\n  opencode:\n    luna:\n      enabled: false\n";
        let next = update_love_text(
            source,
            &LoveEdit::Set {
                profile_id: "opencode".into(),
                model: "luna".into(),
                when: vec![TaskClass::Context],
                effort: None,
                turn_on: true,
            },
        );
        let rules = read_rules(&next);

        assert!(!next.contains("loved: true"));
        assert!(!next.contains("enabled: false"));
        assert!(next.contains("enabled: true"));
        assert_eq!(rules.for_class(TaskClass::Context).unwrap().model, "luna");
        let migrated = rules.for_class(TaskClass::Build).unwrap();
        assert_eq!(migrated.label(), "claude/opus");
        assert_eq!(migrated.effort.as_deref(), Some("high"));
    }

    #[test]
    fn love_arguments_read_kinds_and_refuse_what_is_not_one() {
        let parsed = parse_love_args(&[
            "opencode:luna".into(),
            "--when".into(),
            "context, mechanical".into(),
            "--global".into(),
        ])
        .unwrap();
        assert_eq!(parsed.target.as_deref(), Some("opencode:luna"));
        assert_eq!(parsed.when, [TaskClass::Context, TaskClass::Mechanical]);
        assert!(parsed.global);

        assert_eq!(
            parse_love_args(&["--when=refactoring".into()])
                .unwrap_err()
                .0
                .lines()
                .next()
                .unwrap(),
            "there is no kind of work called 'refactoring'"
        );
        assert!(parse_love_args(&["--clear".into(), "claude:opus".into()]).is_err());
    }

    fn read_rules(source: &str) -> oga_config::LoveRules {
        let layers = oga_config::ConfigLayers {
            user: None,
            project: Some(oga_config::ConfigLayer {
                path: "/work/.oga.yaml".into(),
                root: serde_yaml::from_str(source).unwrap_or_default(),
            }),
        };
        read_love_rules(&layers).unwrap()
    }

    #[test]
    fn config_models_merge_project_fields_by_scope() {
        let layers = oga_config::ConfigLayers {
            user: Some(oga_config::ConfigLayer {
                path: "/home/user/.oga.yaml".into(),
                root: serde_yaml::from_str("models:\n  alpha:\n    enabled: false\n  claude:\n    opus:\n      loved: true\n").unwrap(),
            }),
            project: Some(oga_config::ConfigLayer {
                path: "/work/.oga.yaml".into(),
                root: serde_yaml::from_str("models:\n  alpha:\n    preferred: true\n  claude:\n    opus:\n      enabled: true\n").unwrap(),
            }),
        };
        let models = merged_model_overrides(&layers).unwrap();
        assert_eq!(models["shared"]["alpha"]["enabled"], false);
        assert_eq!(models["shared"]["alpha"]["preferred"], true);
        assert_eq!(models["byProfile"]["claude"]["opus"]["loved"], true);
        assert_eq!(models["byProfile"]["claude"]["opus"]["enabled"], true);
    }

    #[test]
    fn project_love_does_not_treat_global_love_as_local() {
        let global = global_cwd();
        let layers = oga_config::ConfigLayers {
            user: Some(oga_config::ConfigLayer {
                path: global.join(".oga.yaml"),
                root: serde_yaml::from_str("love:\n  - model: alpha\n").unwrap(),
            }),
            project: None,
        };

        assert!(
            love_rules_for_scope(&layers, Path::new("/work/project"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            love_rules_for_scope(&layers, &global).unwrap().0[0].scope,
            "global"
        );
    }

    fn saved(here: &str) -> SavedWorkerPrompts {
        SavedWorkerPrompts {
            here: Some(here.into()),
            everywhere: None,
        }
    }

    #[test]
    fn worker_config_prefers_the_project_file_over_saved_instructions() {
        let layers = oga_config::ConfigLayers {
            user: Some(oga_config::ConfigLayer {
                path: "/home/user/.oga.yaml".into(),
                root: serde_yaml::from_str("worker:\n  prompt: user rules\n").unwrap(),
            }),
            project: Some(oga_config::ConfigLayer {
                path: "/work/.oga.yaml".into(),
                root: serde_yaml::from_str("worker:\n  prompt: project rules\n").unwrap(),
            }),
        };
        let worker = worker_config_json(&layers, &saved("saved rules")).unwrap();

        assert_eq!(worker["workerPrompt"], "project rules");
        assert_eq!(worker["source"], "/work/.oga.yaml");
    }

    #[test]
    fn worker_config_falls_back_to_saved_instructions_without_a_project_table() {
        let layers = oga_config::ConfigLayers {
            user: Some(oga_config::ConfigLayer {
                path: "/home/user/.oga.yaml".into(),
                root: serde_yaml::from_str("worker:\n  prompt: user rules\n").unwrap(),
            }),
            project: Some(oga_config::ConfigLayer {
                path: "/work/.oga.yaml".into(),
                root: serde_yaml::from_str("profiles:\n  alpha:\n    model: sonnet\n").unwrap(),
            }),
        };
        let worker = worker_config_json(&layers, &saved("saved rules")).unwrap();

        assert_eq!(worker["workerPrompt"], "saved rules");
        assert!(worker.get("source").is_none());
    }

    #[test]
    fn worker_config_falls_back_to_the_all_projects_file() {
        let layers = oga_config::ConfigLayers {
            user: Some(oga_config::ConfigLayer {
                path: "/home/user/.oga.yaml".into(),
                root: serde_yaml::from_str("worker:\n  prompt: user rules\n").unwrap(),
            }),
            project: None,
        };
        let worker = worker_config_json(&layers, &SavedWorkerPrompts::default()).unwrap();

        assert_eq!(worker["workerPrompt"], "user rules");
    }

    #[test]
    fn love_rejects_targets_missing_from_an_enumerated_catalog() {
        let offered = vec![ModelInfo {
            id: "listed".into(),
            label: "listed".into(),
            provider: Provider::Codex,
            profile_id: "worker".into(),
            source: ModelInfoSource::Discovered,
            cost: None,
            context_window: None,
            reasoning: None,
            efforts: None,
            default_effort: None,
            tool_call: None,
        }];

        assert!(love_target_catalog_status("worker", "missing", &offered, true).is_err());
    }

    #[test]
    fn unknown_watch_kinds_are_not_lifecycle_events() {
        assert_eq!(parse_event_kind("future_kind"), None);
        assert!(!stream_event_allowed(EventKind::Raw, false, true));
    }

    #[test]
    fn watch_line_carries_settlement_details() {
        let task = BatchTask {
            id: "task-a".into(),
            state: TaskState::Completed,
            question: None,
            error: None,
            title: Some("Ship it".into()),
            archived_at: None,
            tldr: Some("finished".into()),
            code: None,
            truncated: None,
            more: None,
        };
        let line: Value = serde_json::from_str(&watch_line(&task)).unwrap();
        assert_eq!(line["type"], "settled");
        assert_eq!(line["tldr"], "finished");
        assert_eq!(line["title"], "Ship it");
    }

    #[test]
    fn watch_keeps_running_tasks_pending() {
        let task = BatchTask {
            id: "task-a".into(),
            state: TaskState::Running,
            question: None,
            error: None,
            title: None,
            archived_at: None,
            tldr: None,
            code: None,
            truncated: None,
            more: None,
        };
        let mut pending = HashSet::from([String::from("task-a")]);

        assert_eq!(settle_watch_tasks(&[task], &mut pending), 0);
        assert!(pending.contains("task-a"));
    }
}
