# Improvements audit

Audited 2026-09-26 against `main` at `45da436`. Read-only: no source, dependency, or config changes.

Scope: all 16 crates in `rust/crates/`, both apps in `rust/apps/` (`oga-cli`, `oga-desktop`), and `web/`, plus the contracts between them (HTTP, SSE, the watch socket, and MCP).

Ranking: performance first, then correctness, then complexity. Within each part, findings are ordered by how much a user would feel them, then by effort.

## How this was measured

- Live broker: `oga-server serve` on `127.0.0.1:7331`, sent read-only requests only. Its data: a 2.7 GB `~/.oga/oga.db` holding 994 tasks and 346k events. The largest task has 21,146 events and 177 MB of payload.
- Scratch brokers ran on port 7399 with a temporary HOME and a copy of the DB.
- Benchmarks were throwaway crates and scripts in `/tmp`, run against read-only snapshots of the live DB and real task events.
- The machine was heavily loaded throughout: load average 30–141 on 8 cores, from parallel cargo builds. Absolute wall times are inflated. Ratios and before/after comparisons hold. Where it mattered, times were taken as CPU time.
- Some audit requests had side effects on the live broker:
  - Two GETs to `/api/model-settings` each spawned an `opencode models --verbose --refresh` (see P18).
  - One 28 MB events GET raised the broker's memory footprint (see P29).
- Line numbers are as of `45da436`. Another task was editing `web/src/domain/activity/index.ts`, `web/src/domain/trace/index.ts`, and several Rust files during the audit, so some lines may have shifted.

Effort: **S** = hours, **M** = a day or two, **L** = more. **Measured** = numbers below come from a run. **Inferred** = read from code only.

## Summary

| # | Finding | Area | Effort | Evidence | Status |
|---|---|---|---|---|---|
| P1 | Broker waits 8–15 s on an interactive login shell before opening its port | cli, providers | S | Measured | Done |
| P2 | Live task screen re-parses the whole turn on every update (grows with task length) | web | S / M | Measured | Done (partly: parse cache, no incremental fold) |
| P3 | MCP `models` stalls 16–18 s whenever the usage cache is over 60 s old | mcp, http | S | Measured | Done |
| P4 | Tasks waiting on the same prerequisite start one after another | service | S | Measured | Done |
| P5 | `oga query` spends ~0.3–0.5 s per call on route healing, a `git config` spawn, and a symbol scan | context | S | Measured | Done |
| P6 | `oga watch` makes the broker replay a task's full history, then discards it | cli, events | S | Measured | Done |
| P7 | Streamed thinking/message text shows up only when the next step starts | service | S | Measured | Open: needs owner decision |
| P8 | MCP `tasks` loads every task, and it and MCP `query` block broker threads | mcp | S–M | Measured | Done |
| P9 | CLI commands that take a task id download 2,000 task summaries first | cli | S | Measured | Done |
| P10 | Worktree creation copies ignored build folders: 72 GB on disk, ~19 s per Oga worktree | worktree | M | Measured | Out of scope (build-size task) |
| P11 | Agent events stored at full size and pretty-printed per event, per client | service, events | S / S–M | Measured | Done (CPU part); payload cap is an owner decision |
| P12 | Search is one full-text table for all projects, filtered by a path phrase | context, store | M | Measured | Done |
| P13 | Every `oga query` walks the whole tree on one thread | context, http | M | Measured | Done |
| P14 | First query in a new worktree rebuilds the index in one write transaction | context | M | Measured | Done |
| P15 | Code-index rows are never pruned; subfolder and home-folder queries build their own indexes | context | S | Measured | Done |
| P16 | Task screen re-renders the app shell twice per update, plus once a second | web | S | Measured / Inferred | Done |
| P17 | Task-screen chunk is 535 KB; the bundle ships ~10 MB of Shiki grammars | web | S / M | Measured | Done (partly: lazy diff chunk; Shiki trim is an owner decision) |
| P18 | Model settings sends 627 KB and starts a full provider refresh on every open | http, web | S | Measured | Done |
| P19 | MCP results are pretty-printed; unfiltered `models` returns 712 KB | mcp | S | Measured | Done |
| P20 | Leaving a long task stringifies all its events; long tasks are never cached | web | S | Measured | Done |
| P21 | DB and CPU work runs on async worker threads in several broker paths | events, http | M | Inferred | Done |
| P22 | Task start is dominated by the agent connecting (3–22 s); no per-stage timings | acp, service | M–L | Measured | Done (timings recorded; pre-start not built) |
| P23 | Worker output capture goes quadratic on large output | runner, acp | S | Measured (bench) | Done |
| P24 | Sidebar re-renders on every stream batch and forces layout each time | web | S | Measured / Inferred | Done |
| P25 | Full scans of `tasks` on hot small queries; unused case-insensitive indexes | store | S | Measured | Done |
| P26 | Advisor builds a new HTTP client per call; pricing catalogue deep-cloned per call | advisor, pricing | S | Inferred | Done |
| P27 | Network probes for parked tasks run one after another | service | S | Inferred | Done |
| P28 | Release binary is unstripped; dev installs carry a duplicate binary and a 2.5 GB backup | build | S | Measured | Done (strip only) |
| P29 | Broker memory stays at ~160 MB after heavy event reads | http, events | via P9/P11 | Measured (cause inferred) | Done (re-measured; no longer held) |
| C1 | One refused dependent leaves its siblings `queued` forever; follow-ups skipped | service | S | Measured | Done |
| C2 | A dependent created as its prerequisite finishes waits for the 30 s sweep | service | S | Measured | Done |
| C3 | One slow `opencode --version` refuses every OpenCode 1 task until restart | providers | S | Measured | Done |
| C4 | New tasks can be missing from the sidebar for 15 s or indefinitely | web | S | Measured | Done |
| C5 | Cancel and force-complete drop the end of the run | service | S–M | Measured | Done |
| C6 | Single writer lock and a second writing process risk "database is locked" for workers | store, context, cli | M | Inferred | Done |
| C7 | CLI task lookup is capped at 2,000 tasks; older ids will stop resolving in ~25 days | cli | S | Measured | Done |
| C8 | Title-bar duration stops counting between updates | web | S | Inferred | Done |
| C9 | Wake recovery can cancel tasks that are still connecting | service | S | Inferred | Done |
| C10 | Desktop app discards the broker's stdout and stderr | desktop | S | Inferred | Done |
| C11 | Code-index edge cases: symbol cap skipped, trailing-slash duplicates, `.gitignore` classes | context | S | Measured | Done |
| X1 | Two `append_event_tx` copies and ~8 raw `INSERT INTO task_events` sites | service | S | Inferred | Done |
| X2 | Delivery inbox (684 lines) has no in-repo client, and its GET writes | service, http | owner call | Inferred | Out of scope (owner decision) |
| X3 | Two syntax highlighters (refractor and Shiki) | web | M / L | Measured | Skipped (see fix log) |
| X4 | Dead gap check in the sidebar's batch path | web | S | Measured | Out of scope (cleanup task) |

Suggested order: P1, P4 + C1 + C2 (one fix), P3, P2 (parse cache), P5, P6, P7, P8, P9 + C7, P11 + X1. All of these are S or S–M. Then P10, P12–P15 (the index work), and P17.

## Fix log

Fixed on `main` on 2026-09-26, one commit per fix, each measured before and after with the method named below. The machine load was lower than during the audit, so compare each "before" only with its own "after", not with the audit's numbers.

| # | Commits | Before → after |
|---|---|---|
| P1 | `645c9e4` `5aaeda2` `4c98238` | First `/health` on a scratch broker with the user's shell: median 6.8 s → 0.40 s. The 60 s re-capture is gone; PATH is re-read only when a spawn fails with "not found". |
| P2 | `de8a751` | CPU per live update (real events): opencode 150/1,000/3,000 events 26/185/580 ms → 7/37/60 ms; claude 5.4/38/110 → 3.4/13/20 ms. Cost still grows with turn length (incremental fold not done: the pipeline's lookbacks need a resumable rewrite). |
| P3 | `9245782` | MCP `models` after more than 60 s idle: 6.9–7.5 s → 0.05 s. The very first read for a profile still waits. |
| P4, C1, C2, C9 | `7225580` | Harness, 3 dependents of 1 s: all done at +3.06 s → +1.03 s. A refused dependent no longer strands its sibling. A follow-up on a task with dependents now starts. Race: 5/20 left `pending` → 0/20. Wake recovery leaves connecting runs alone. |
| P5 | `ff8de43` `1dc9d21` `b81bdbe` | Route heal 37 ms per lookup → skipped unless the index changed; `git config` 12.5 ms → 0; name lookups use the name index. |
| P6 | `0163a02` | `oga watch` on a 21k-event task: 0.35 s and 6.2 MB replayed → under 1 ms, 605 B. |
| P8 | `5c91e21` | MCP `tasks` 0.08–0.13 s → 0.001–0.004 s. `/health` max under 12 concurrent calls 0.63–1.06 s → 9–11 ms. |
| P9, C7 | `3df8443` | `oga tasks` 55 → 10 ms; `oga inspect` 53 → 7–9 ms. Ids older than the newest 2,000 resolve. |
| P11 (CPU), X1 | `5dc4e07` `abac42a` `28ccb79` | `event_views` on a 21k-event task 419–451 → 252–302 ms. Summary views 220–238 → 50–59 ms. `/api/events` replay 0.40–0.43 → 0.27–0.30 s. What is stored is unchanged. |
| P12 | `35b3bed` | Term counts 15.0 → 1.2 ms and search 22.0 → 4.7 ms here. Answers no longer include hits from nested indexes. |
| P13 | `805cdf0` `40ecdd4` | Walk: pitasgrid 85 → 32 ms. A lookup within 2 s of the last walk skips it, unless git moved the checkout. |
| P14 | `4a9fd6a` | Longest index write transaction 890–909 → 74–76 ms. First lookup in a fresh clone 1.25 → 0.30 s (4 files parsed instead of 388). |
| P15 | `d9f3abc` | Snapshot: 67 → 20 indexed folders, 1.53M → 185k symbols, 1,487 → 494 MB after vacuum. Subfolder lookups answer from the repository's index; non-repo folders are refused. |
| P16, C8 | `ed77030` | Renders per update: TaskDetail 2 → 1, Sidebar 1 → 0.05. Idle CPU per second 30 → 15 ms. The title-bar time keeps counting. |
| P17 | `396957e` | TaskDetail chunk 535 KB → 64 KB. Diff renderer loads on the first diff. Dist total unchanged at 11.6 MB. Trimming Shiki's languages needs a fragile import shim: owner decision. |
| P18 | `f32df29` `e08cc02` `3c65a79` | Provider refreshes over 6 opens within the TTL: 6 → 1. Empty sidebar no longer reads model settings. Handoff dialog reads enabled models only: 386 KB → 8.8 KB on the scratch catalog. |
| P19 | `3d0bb90` `29f9e41` | `models` default 14.2 → 6.4 KB; unfiltered 750 KB → 8.5 KB (50-row default cap, `limit` for more); `inspect` 1.4 → 0.9 KB. |
| P20 | `1c4193c` | Sizing on leave, 150/1,000/5,000 events: 2.5/24/111 ms → 0.1/0.2/0.9 ms. Long tasks keep their newest 150 events in the cache. |
| P21 | `d47ea36` | `/health` p95 with 16 replaying watchers 15–66 → 4–10 ms. |
| P22 | `6a4da02` | `worker_spawned` records spawn, initialize, session, configure, and opened times. Pre-starting agents is not built. |
| P23 | `b429b9a` | Capture at 50 MiB 1.38–1.60 → 0.03 s. Frame reader at 4 MiB 0.35 → 0.002 s. Codex diff with 1,000 changed files 0.19 → 0.045 s. |
| P24 | `9176ca1` | No-op batch 10.1 ms and 1 render → 0.03 ms and 0 renders. |
| P25 | `d804238` | Activity 2–3 → 0–1 ms, usage 3–7 → 0–1 ms, projects 3–8 → 2–3 ms. Three unused case-insensitive indexes dropped. |
| P26 | `57facdf` | `catalogue()` 3.3 ms → 0.3 µs per call; one shared advisor client. |
| P27 | `34c76f2` | Sweep with 4 network-parked tasks against unreachable hosts 48 → 3 s. |
| P28 | `baeced4` `2e3146a` `37062bc` | Binary 50.1 → 44.1 MB (`strip`; LTO measured and not worth it). Dev install keeps one broker copy (−48.9 MB). DB backup 5.3 s and 2.6 GB → 0.23 s, no extra disk. |
| P29 | — | Re-measured after heavy reads: footprint returns to 41–51 MB after 15 s idle (audit: held at 160 MB). |
| C3 | `99e44ad` `acb9f8b` | A version check that hangs once no longer refuses later OpenCode 1 tasks, and no longer blocks an async thread for 5 s. |
| C4 | `ae18f59` | A second new task within 15 s now refreshes the list. |
| C5 | `88df67f` | Cancel closes the turn; a late run's events and spend are kept; the task stays cancelled. |
| C6 | `09b1d73` | Writer transactions `BEGIN IMMEDIATE`; a second writer waits instead of failing "database is locked". VACUUM (25.6 s on the snapshot) runs only while no task works. |
| C10 | `f64e88c` | Desktop broker output goes to `broker.log` beside the DB, set aside at 5 MiB on start. |
| C11 | `2cf5571` | Symbol cap holds on partial walks and reconciles; trailing-slash folders share one index; `.gitignore` bracket classes match. |
| P7 | — | Open. Flushing text early splits one message into several events, and the task screen, closing answer, handoff brief, and `oga watch` each read one event as one message. Needs a choice of storage shape first. |
| X3 | — | Skipped. `@pierre/diffs` is built on Shiki, so diffs can't move to refractor; moving markdown to Shiki saves only refractor's 95 KB and changes code colours. |

---

## Part A: Performance

### P1. Broker waits on an interactive login shell before opening its port

- **Where:**
  - `rust/apps/oga-cli/src/main.rs:232` awaits `oga_service::warm_login_path` before `TcpListener::bind` at `:242`.
  - `rust/crates/oga-providers/src/worker_path.rs:69-79` runs `$SHELL -lic` with no timeout.
  - `REFRESH_TTL` is 60 s (`worker_path.rs:14`).
  - The desktop gives up waiting at `rust/apps/oga-desktop/src/broker.rs:14-16`: 20 checks, 350 ms apart, about 7 s.
- **User cost:**
  - After every app launch, broker restart, or update, the app, CLI, MCP clients, and workers get "connection refused" for 8–15 s.
  - The desktop shows "Oga did not become ready" on its first cycle.
  - An rc file that hangs stops the broker from ever starting.
  - While tasks dispatch, the 60 s TTL starts another `zsh -lic` about once a minute, costing ~3 s of CPU each time.
- **Evidence (measured):** time to first `/health` on a scratch broker:

  | Shell | Time to first `/health` |
  |---|---|
  | No-op shell | 0.17–0.24 s |
  | Bare zsh | 0.15–0.24 s |
  | The user's real rc files | 8.17 s and 13.99 s |

  Standalone shells: `zsh -lic` took 6.3–15.1 s across three auditors, with ~3 s of CPU. `zsh -lc` took 0.03–0.06 s.
- **Gain:** 7–14 s off every broker start, and no stray shell spawned each minute.
- **Fix:**
  - Bind and serve first.
  - Capture PATH in the background; the lazy `login_path()` already exists.
  - Have only worker spawns wait for the first capture.
  - Add a ~5 s timeout.
  - Raise the TTL, or refresh only when a spawn fails with "not found".

### P2. Live task screen re-parses the whole turn on every update

- **Where:**
  - `web/src/domain/activity/index.ts:265-298`: `ActivityStoryProjection.update` reuses only when nothing changed. On any append it runs `composeWithState` over the whole turn and copies the array.
  - Uncached `JSON.parse` of `rawText` at `domain/activity/index.ts:1216,1449,1643` and `domain/trace/index.ts:1165`.
  - Driven by the `transcriptItems` memo in `screens/task/TaskDetail.tsx` and the rows memo in `TranscriptWork`.
  - The open task's event array is never capped, so each update costs O(n) and a session costs O(n²).
- **User cost:** on a busy running task, scrolling and typing in the composer lag, and CPU climbs. It gets worse the longer the task stays open, or after "load earlier".
- **Evidence (measured):** replaying real events from the live broker (`/tmp/oga-bench/live-append.bench.ts`), CPU ms per update:

  | Events held | opencode task | claude task |
  |---|---|---|
  | 150 | ~27 | ~4.5 |
  | 1,000 | ~220 | ~38 |
  | 3,000 | ~630 | ~119 |

  - At 1,000 events one rebuild does 3,552 `JSON.parse` calls over 29.5 MB. That is 87% of the rebuild time.
  - The repo's own `transcript-performance.bench.ts` reports `incrementalRate: 0` for every fixture.
- **Gain:** a `WeakMap<TaskEventView, parsed>` cache alone cut a rebuild from 118.7 ms to 13.1 ms (opencode) and from 40 ms to 12 ms (claude). Row building went from 87 ms to 44 ms. Folding only newly appended events into the last turn would make the cost flat.
- **Effort:** S for the parse cache, M for the incremental fold.

### P3. MCP `models` stalls 16–18 s when the usage cache is stale

- **Where:** `rust/crates/oga-mcp/src/lib.rs:394` (`usage` defaults to on) and `rust/crates/oga-http/src/settings.rs:1697` (`USAGE_CACHE_TTL` = 60 s) and `:1746`.
- **User cost:** an agent that checks models before delegating hangs ~17 s, often on every call after a minute of quiet. The user waits on their orchestrating session.
- **Evidence (measured):**

  | Call | Time |
  |---|---|
  | First call | 16.28 s |
  | After more than 60 s idle | 18.30 s |
  | Warm | 0.12–0.19 s |
  | `usage:false` | 0.07 s |

- **Gain:** ~17 s per affected call.
- **Fix:** return cached usage immediately and refresh it in the background (`cached_usage()` exists), or default `usage` off for MCP.

### P4. Tasks that wait on the same prerequisite start one after another

- **Where:** `rust/crates/oga-service/src/lifecycle.rs:361-410`. `run_task_and_release_with_active` pops released dependents into `pending` and awaits each run in turn. It is called from `dispatch.rs:640`.
- **User cost:** a fan-out plan, where several tasks depend on one task, runs N times slower than it should. The siblings sit in `queued` meanwhile.
- **Evidence (measured):** a harness with three 1 s dependents of one blocker. After the blocker finished they started at +1.0 s, +2.1 s, and +3.3 s, and all were done at +4.4 s instead of ~+1.1 s.
- **Gain:** siblings run in parallel, so wall time equals the longest one.
- **Fix:** launch each released dependent through `Dispatcher::launch_task`, as `settle_dependents` already does, and drop the inner loop. This also fixes C1 and the follow-up bug in C1.

### P5. `oga query` spends most of its time on avoidable work

Warm `oga query` in this repo takes 0.9–1.9 s end to end. Nearly all of it is in the broker; CLI startup is 0.03 s. The same code against a DB holding only this project answers in ~0.2 s. Three quick wins:

- **(a) Route healing on every query.**
  - Where: `rust/crates/oga-context/src/routes.rs:253-320`, called unconditionally from `index.rs:305`.
  - It writes one transaction per learned route: 311 writes per query here, each firing an FTS trigger and taking the broker's writer lock.
  - Measured: `routes_heal` took 244 ms with `changed=false`. Route counts: oga 322, inter 201, rondpoint 127.
  - Fix: skip when the reconcile changed nothing, and update only rows that differ, in one transaction.
- **(b) A `git config` process spawned on every walk.**
  - Where: `rust/crates/oga-context/src/walk.rs:92-109`.
  - Measured: 58–233 ms under load; `git config` alone took 60–80 ms.
  - Fix: cache the global excludes path once per process.
- **(c) `symbols_by_name` scans every symbol in the project.**
  - Where: `rust/crates/oga-context/src/store.rs:120-181`. The OR'd `instr` conditions defeat the name index.
  - Measured plan: `SEARCH context_symbols USING INDEX context_symbols_path (cwd=?)` plus a temp sort. Medians: 90 ms (oga), 194 ms (pitasgrid, 60k symbols), 2.2 s (home folder).
  - The schema comment says "an index seek rather than a scan"; the plan shows otherwise.
  - Fix: look up exact names with `name_key IN (...)`, and find word matches through FTS.

- **User cost:** every agent calls `oga query` first by house rule, so this cost repeats across every session.
- **Gain:** ~0.3–0.5 s per query here, more on large projects.
- **Effort:** S each.

### P6. `oga watch` replays a task's full history, then throws it away

- **Where:** `rust/apps/oga-cli/src/main.rs:1632` subscribes with `"afterCursor": 0`. `rust/crates/oga-events/src/socket.rs:243-247` starts from 0. The client drops every event up to the hello's `initialCursor` (`main.rs:1790`).
- **User cost:** each `oga watch` on a long task takes seconds to connect. The replay runs on an async worker thread, slowing other broker requests at the same time. Orchestrating sessions run `oga watch` constantly.
- **Evidence (measured):** a benchmark socket against a read-only DB snapshot:

  | Task | `afterCursor=0` | Start at latest |
  |---|---|---|
  | 21k events | 4.365 s, 213 frames, 6.2 MB | 0.002 s |
  | 18k events | 2.75 s | 0.001 s |
  | 379 events | 0.090 s | 0.019 s |

- **Gain:** seconds per watch, down to milliseconds.
- **Fix:** have the client ask to start at the latest cursor, or have the server treat 0 as "from now". The client sees the same events either way.

### P7. Streamed text is held back until the next step starts

- **Where:** `rust/crates/oga-service/src/acp_run.rs:892-936`. The comment says "one event per sentence", but the code flushes only when the update kind changes.
- **User cost:** in the task feed, thinking and message text appear late, sometimes by tens of seconds, then all at once.
- **Evidence (measured):** on the live broker, across 10 tasks, 118 of 123 text-chunk events were written within 5 ms of the event after them. The gap before a chunk, an upper bound on the hold-back, was p50 1.0 s, p90 7.9 s, max 33 s.
- **Gain:** text appears as it is written.
- **Fix:** also flush on a sentence end or newline, or on a 0.5 s timer.

### P8. MCP `tasks` loads every task; MCP `tasks` and `query` block broker threads

- **Where:** `rust/crates/oga-mcp/src/lib.rs:1044` (`list_tasks`; SQL at `:1077`: `SELECT id FROM tasks ORDER BY updated_at` with no WHERE).
  - For each id it loads the full row, including prompt and output, then runs timing, hold, and follow-up-count queries.
  - It filters and keeps 20 afterwards.
  - It runs synchronously inside an async handler.
  - MCP `query` (`lib.rs:230`, `:547`) calls `oga_http::context::answer`, which takes a `std::sync::RwLock` and walks the tree, also inline. The HTTP path wraps the same call in `run_blocking`.
- **User cost:** each `tasks` call takes 0.6 s and grows with history. With several agents polling, the whole broker stalls, including the web app.
- **Evidence (measured):**

  | Load | `tasks` | `/health` |
  |---|---|---|
  | Single call | 0.55–0.64 s | 1 ms |
  | 4 concurrent | – | up to 0.66 s |
  | 12 concurrent | up to 3.76 s | up to 1.77 s |

  MCP `query` took 0.85–1.28 s per call. The blocking itself is inferred from code.
- **Gain:** `tasks` at ~10 ms, and no broker-wide stalls under parallel agents.
- **Fix:** push since/state/archived/limit into SQL, select only summary columns, and wrap both tools in `spawn_blocking`.

### P9. CLI commands that take a task id download 2,000 summaries first

- **Where:**
  - `rust/apps/oga-cli/src/main.rs:891-900`: `all_task_summaries`, archived included, `limit(2_000)`.
  - `:1179`: `resolve_task`, which filters the list client-side.
  - Used by `tasks`, `inspect`, `archive`, `cancel`, `resume`, `handoff`, and `complete`. `archive` and `cancel` repeat it for every id.
- **User cost:** 0.2–0.9 s per CLI call where 2 ms would do. Agents run these many times per session.
- **Evidence (measured):**

  | Call | Time and size |
  |---|---|
  | `oga tasks` | 0.27–0.93 s to print 672 bytes |
  | `oga inspect` | 0.19–0.72 s |
  | `GET /api/tasks/<id>` | 2 ms |
  | Summary request, all tasks | 934 KB for 994 tasks, 0.44–0.70 s |
  | Summary request, `limit=50` | 7 ms |

- **Fix:** call `GET /api/tasks/{id}` directly for a full id. Use a server-side prefix lookup and a filtered, limited query for `tasks`. This also closes C7.

### P10. Worktrees copy ignored build folders

- **Where:**
  - `rust/crates/oga-worktree/src/lib.rs:332`: `is_default_link` excludes `build` but not `.build`.
  - `:354`: dedup keeps the parent folder.
  - `:232`: `copy_tree` copies everything under a kept folder, without applying the exclusion list.
- **User cost:** the disk fills; the project memory records worktrees reaching ~160 GB and killing builds. A new worktree task waits before its worker starts.
- **Evidence (measured):**
  - `~/.oga/worktrees` is 72 GB across 25 worktrees.
  - One Oga worktree holds a 19 GB `rust/target` and a 6.7 GB `swift/.build`.
  - A Laravel project's worktrees each carry a 2 GB `storage/app`.
  - Every new Oga worktree copies the ignored `swift/` folder (6.4 GB, 17.8k files). `cp -Rc` of it alone takes 19 s; `git worktree add` takes 0.60 s.
  - How much real disk the copies use depends on APFS cloning (inferred).
- **Gain:** tens of GB, and ~19 s off each Oga worktree start.
- **Fix:**
  - Apply the exclusion list inside `copy_tree`.
  - Add `.build`, and cap the size of copied folders.
  - Either clone the origin's `target/` for a warm start on purpose, or delete regenerable build folders when a task settles.

### P11. Agent events stored at full size and pretty-printed on every read

- **Where (size):** `append_event_tx` in `rust/crates/oga-service/src/lifecycle.rs:1825`, called from `acp_run.rs:962,979` and `lifecycle.rs:1190`. The 8 KB / 32 KB limit `bound_event_payload` (`oga-events/src/lib.rs:477`) is applied only to hook events (`oga-http/src/hooks.rs:29`).
- **Where (CPU):**
  - `rust/crates/oga-events/src/lib.rs:1217` and `:1078` pretty-print every event's payload into `rawText`.
  - Patched events are pretty-printed again and fully cloned (`lib.rs:671`, `acp.rs:413`).
  - Every connected feed or watcher redoes this per event.
- **User cost:** a 2.7 GB DB; a slow task open, live feed, and `oga watch`; broker CPU that scales with the number of viewers.
- **Evidence (measured):**
  - Size, from sqlite3 on the live DB:
    - 6,179 events are over 32 KB, totalling 444 MB; the largest is 1.2 MB.
    - `agent.tool_call_update` alone is 702 MB.
    - The first screen of a task (`last=150`) is 309–775 KB, and `rawText` is 72–81% of it.
  - CPU, on the 21k-event task:
    - Event views take 2.27 s, of which pretty-printing is 1.50 s.
    - `event_views` over the full history takes 6.17 s, and serializing it 2.0 s (192 MB).
    - Replaying that task over `/api/events` on the live broker took 10.1 s.
- **Gain:** a large cut in DB size and read cost; about two-thirds of per-event CPU on the live-feed and watch paths.
- **Fix:**
  - Apply `bound_event_payload` in the one append helper (see X1). This is a product call: hook events already truncate.
  - Add a summary-only view without `rawText` for the feed and the watch socket.
  - Load `rawText` when a row is expanded.
  - Reuse the patched text instead of printing twice.
  - The full "derive diffs server-side and drop `rawText` from snapshots" change is L; do it later, only if still needed.
- **Effort:** S for the bound, S–M for the view.

### P12. One search table for every project

- **Where:** `rust/crates/oga-context/src/store.rs:555` (`project_match`, a `{cwd} : "<path>"` phrase ANDed with the question). It is used by `term_hits` (`:246`) and `symbols_by_search` (`:184`); the table is `context_symbols_fts` in `oga-store/src/schema.rs`.
- **User cost:** slow `oga query`, plus wrong answers:
  - The phrase for `/Users/malico/desgn/oga` also matches every indexed folder under it. 26 of the top 50 hits came from the stale `oga/rust` and `oga/web` indexes and surface as "(N candidates omitted: no longer on disk)".
  - Question words are not limited to symbol columns, so they match the stored path. "user" matched all 59,760 of one project's symbols.
- **Evidence (measured):** instrumented release build against a DB snapshot:

  | Step | Shared table | Single-project DB |
  |---|---|---|
  | Term counts | ~200–240 ms | 7 ms |
  | Search | ~200–230 ms | 15 ms |

- **Gain:** ~400 ms per query, and correct results.
- **Effort:** M. The layout changes, so bump `INDEX_SCHEME` and rebuild.
- **Fix:** store an opaque per-project token in the path column, and restrict question terms to `{name qualified tokens signature doc path}`.

### P13. Every query walks the whole tree

- **Where:** `rust/crates/oga-http/src/context.rs:199-219`; `rust/crates/oga-context/src/walk.rs:62-197`.
- **User cost:** slow `oga query` in large projects.
- **Evidence (measured):**

  | Tree | Walk time |
  |---|---|
  | oga, 384 files | 35–60 ms |
  | pitasgrid, 5,115 files | 1.2–2.3 s (`git status` on the same tree: 0.3 s) |
  | Home folder, 20k files | 5.3 s, in a 9 s steady-state query |

- **Gain:** 1–5 s per query on big trees.
- **Fix:** skip the walk if one finished a few seconds ago, or keep a per-project file watcher. Otherwise use `git ls-files` or the `ignore` crate's parallel walker.
- **Effort:** M.

### P14. First query in a new worktree rebuilds the index in one transaction

- **Where:** `rust/crates/oga-context/src/index.rs:193-214`; `store.rs:381-454`.
- **User cost:** the first `oga query` in a task worktree takes tens of seconds. While it writes, every other broker write waits, including running workers' events (see C6).
- **Evidence (measured):** fresh DB, heavy load:

  | Project | Files | Parse | Single write transaction |
  |---|---|---|---|
  | oga | 382 | 3.4 s | 6.5 s |
  | pitasgrid | 5,115 | 18.5 s | 29.9 s |

  Eight worktree indexes of ~4.9k files and 58k symbols each were full rebuilds.
- **Gain:** a worktree's first query drops to about the walk time, and broker writes stop stalling.
- **Fix:** write in batches (the schema has an unused `building` state). Seed a worktree's index from its origin's rows and re-parse only files whose hash differs.
- **Effort:** M.

### P15. Code-index rows are never pruned, and stray folders get their own index

- **Where:**
  - Nothing deletes index rows outside `oga-context`. Worktree removal (`oga-service/src/archive.rs`) leaves them behind.
  - `rust/apps/oga-cli/src/main.rs:361` sends the current directory, not the git root.
  - `get_query` → `answer` → `reconcile` → `build` never checks for a repository; only `--init` does (`oga-http/src/context.rs:102`).
- **User cost:**
  - The index is ~1.37 GB of the 2.7 GB DB, which slows backups and vacuums.
  - Dead rows feed the wrong answers in P12.
  - Running `oga query` from `~` built a 20k-file index: 535 s cold, ~9 s steady.
- **Evidence (measured):**
  - 35 of 67 indexed folders no longer exist; they hold 625,236 of 1,527,447 symbols. By file count, 42,642 of 100,101 indexed files are in missing folders.
  - 34 indexes use layouts 7 and 9, which this binary can't read.
  - `oga/rust`, `oga/web`, and `/Users/malico` each have their own index.
- **Gain:** roughly 40–60% fewer index rows, a smaller DB, and cleaner answers.
- **Fix:**
  - Drop a folder's rows when its worktree is removed.
  - Prune missing folders and old layouts at startup or cleanup.
  - Resolve the query folder to its git root and treat the subfolder as `--in`.
  - Refuse non-repo folders, as `--init` does.
- **Effort:** S.

### P16. Task screen re-renders the app shell twice per update, plus once a second

- **Where:**
  - `web/src/screens/task/TaskDetail.tsx:476` passes an inline `onExpansionChange`, which defeats the transcript's memo.
  - The header effect (`:416-419`) calls `onHeader` with a new object on every revision. That re-renders `AppShell`, `Sidebar` (not memoised, and handed a new `navigation={{…}}` at `shell/AppShell.tsx:361`), `TitleBar`, and `TaskDetail` a second time.
  - `setInterval(forceUpdate)` at `TaskDetail.tsx:235` re-renders the whole task screen every second.
- **User cost:** steady CPU and small stutters while a task runs.
- **Evidence:**
  - Measured in jsdom: a transcript re-render costs 6.4–8.0 ms with a new callback, against 1.1–1.8 ms with a stable one.
  - Inferred: the double shell render and the per-second tick.
- **Fix:** `useCallback` the handler, memoise `Sidebar` and `TaskDetailRoute`, and move the tick into one small `<LiveDuration>` component (which also fixes C8).
- **Effort:** S.

### P17. Task-screen chunk is 535 KB; the bundle ships ~10 MB of Shiki grammars

- **Where:** `web/src/components/CodeDiff.tsx:2` imports `@pierre/diffs` statically. `CodeDiff` is used by `Trace.tsx:33` and `ChangedFiles.tsx:12`.
- **User cost:** a slower first task open, and a larger app download on install and every update.
- **Evidence (measured):** a build with source maps to `/tmp`:
  - The `TaskDetail` chunk is 535 KB (153 KB gzipped). About 83% of it is `@pierre/diffs` plus Shiki/oniguruma; the app's own code there is about 57 KB.
  - `dist` is 11.6 MB in 337 files, of which about 10.3 MB is Shiki grammars, themes, and wasm. Tauri bundles all of it.
- **Gain:** a faster first task open, and ~10 MB off the app.
- **Fix:** lazy-load `CodeDiff` (S). Register a fixed set of Shiki languages and themes (M). See X3.

### P18. Model settings sends 627 KB and starts a full provider refresh on every open

- **Where:**
  - Rust: `rust/crates/oga-http/src/settings.rs:1485-1495` answers from `cached_catalog`, then calls `refresh_catalog_in_background`, which runs `discover_catalog(.., true)` (`:982`) and bypasses the 30-minute cache every time.
  - Web: `screens/sidebar/Sidebar.tsx:255` fetches the whole payload only to check `workers.length === 0`. The handoff dialog (`screens/task/Actions.tsx:661`) uses only enabled models. Settings renders all 704 opencode models, unvirtualised.
- **User cost:** a short wait opening the handoff dialog and the Settings models tab. Each open also spawns `opencode models --verbose --refresh` in the background, costing CPU and network.
- **Evidence (measured):**
  - The response is 626,906 B, served in 35–167 ms; the two opencode workers alone are 516 KB.
  - A refresh child process was still running 21 s after one GET.
- **Fix:**
  - Respect the cache TTL in the background refresh.
  - Use `profiles` from the summary for the sidebar's empty check.
  - Add an enabled-only query for the handoff dialog.
- **Effort:** S.

### P19. MCP results are larger than they need to be

- **Where:** `rust/crates/oga-mcp/src/lib.rs:246` uses `to_string_pretty`. `models` rows repeat each profile's `usage` object, and `inspect` includes a `transport` blob by default. Tool descriptions were out of scope.
- **User cost:** agent context spent on whitespace and repetition. An unfiltered `models` call can blow out a context window.
- **Evidence (measured):**
  - Pretty-printing adds 12–17% to `tasks` and `inspect`, and 37% to default `models` (14.2 KB against 8.9 KB compact).
  - `models` with `onlyPreferred:false, onlyEnabled:false` returns 712 KB (~178k tokens, 1,563 rows, no cap). `usage` is 175 KB of the 460 KB compact size, repeated for only 9 distinct profiles.
- **Fix:** compact JSON, usage reported once per profile, a limit on `models`, and no `transport` by default.
- **Effort:** S.

### P20. Leaving a long task stalls the app, and long tasks are never cached

- **Where:** `web/src/state/taskDetail/controller.ts:301` (`cacheBytes`) and the 4 MB budget at `:360`.
- **User cost:** a hitch when navigating away from a long task, and a full reload (0.3–0.8 MB) when coming back to it.
- **Evidence (measured):**
  - Sizing the cache stringifies every held event: 7 ms at 150 events, 150 ms at 5,000.
  - Tasks with about 1,000 events or more (4.6–6.9 MB) never fit the budget.
- **Fix:** count bytes as events arrive, or cap the cache by event count.
- **Effort:** S.

### P21. DB and CPU work on async worker threads

- **Where:**
  - Watch socket loop: `oga-events/src/socket.rs:181` onward. `load_tasks` runs at `:343`, and `load_task_contexts` at `:431` makes 3 queries per task per batch.
  - Event poller: `oga-events/src/lib.rs:285`.
  - HTTP handlers: `get_activity` (`oga-http/src/state.rs:443`, polled every 2 s by the tray), task turns/diff/branch, profiles, settings, the `grants().touch` write in `tasks.rs`, and follow-up writes.
- **User cost:** stalls across the app while a replay runs or a write waits on the writer lock.
- **Evidence (inferred):** read from code, supported by the 4.4 s inline replay in P6 on an 8-thread runtime.
- **Fix:** route these through `run_blocking` / `run_read`, as the state and events handlers already do.
- **Effort:** M.

### P22. Task start is dominated by the agent connecting

- **Where:** `rust/crates/oga-service/src/acp_run.rs:259` (`AcpSession::open`) and `rust/crates/oga-acp/src/session.rs:514-568`.
- **User cost:** seconds between delegating and seeing the first activity.
- **Evidence (measured, 50 live tasks):**

  | Stage | Time |
  |---|---|
  | Broker queued → started | 6–13 ms |
  | Worker spawned, claude | 3.0–9.3 s |
  | Worker spawned, opencode | 1.1–22.7 s |
  | Worker spawned, opencode2 | 0.56–8.5 s |
  | First agent event after spawn | another 1–13 s |

- **Gain:** unknown until the stages are timed.
- **Fix:** record per-stage timings (spawn, initialize, `session/new`, config) in `worker_spawned`, then consider pre-starting one agent per profile.
- **Effort:** M–L.

### P23. Worker output handling goes quadratic on large output

- **Where:**
  - `rust/crates/oga-runner/src/lib.rs:1099-1106`: past 10 MiB, every 8 KiB read shifts the whole buffer.
  - `rust/crates/oga-acp/src/transport.rs:447`: the frame reader rescans the whole buffer on each read.
  - Per-event costs on the command-line path:
    - Each event clones the full `Task` (`lifecycle.rs:873`).
    - Each Codex file change runs a `git` diff (`lifecycle.rs:909`).
    - Up to 5,000 events are copied into a channel nobody reads (`runner lib.rs:1033`).
    - Lines are split byte by byte (`lib.rs:1133`).
- **User cost:** broker CPU spikes on tasks with very large tool output. Today's largest live event (384 KB) costs ~10 ms, so this is rare.
- **Evidence (measured, release-build bench copies):**

  | Path | Size | Time |
  |---|---|---|
  | Capture | 11 MiB | 0.65 s |
  | Capture | 20 MiB | 5.6 s |
  | Capture | 50 MiB | 13.9 s |
  | Frame reader | 1 MiB | 0.18–0.40 s |
  | Frame reader | 4 MiB | 5.7–7.2 s |

- **Fix:** a ring buffer or batched trim, remembering the scan offset (or `memchr`), passing the task id instead of a clone, and creating the channel only when it is read.
- **Effort:** S.

### P24. Sidebar re-renders on every stream batch and forces layout

- **Where:** `web/src/state/sidebar-state.ts:305` (`applyEventBatch` always returns a new state) and `screens/sidebar/Sidebar.tsx:318` (a `useLayoutEffect` with no deps that reads layout twice and builds a new `ResizeObserver` on every render).
- **User cost:** slight jank while tasks run.
- **Evidence:** measured that an empty batch still yields a new state; the projection itself is cheap (0.4 ms for 50 tasks). The layout cost is inferred.
- **Fix:** return the same state when nothing changed, and give the effect deps.
- **Effort:** S.

### P25. Full scans of `tasks` on hot small queries

- **Where:**
  - `/api/activity`: `oga-http/src/state.rs:446`, every 2 s.
  - Spend totals: `oga-store/src/repo/mod.rs:623`.
  - Usage, and projects (`oga-http/src/settings.rs:292`).
  - Case-insensitive search indexes at `oga-store/src/schema.rs:207-209` can't serve `%x%` searches and only cost writes.
- **User cost:** little today; it grows with history.
- **Evidence (measured):** every query plan is a full scan of `tasks`. Activity 12 ms, spend 24 ms, projects 43 ms, usage 38 ms.
- **Fix:** add indexes on `(archived_at, state)` and `(spend_at)`, and drop the three NOCASE indexes.
- **Effort:** S.

### P26. Advisor and pricing overhead on auto-routed dispatch

- **Where:**
  - `rust/crates/oga-advisor/src/lib.rs:136` builds a new `reqwest::Client` per call, with a 15 s timeout.
  - `rust/crates/oga-pricing/src/lib.rs:353` deep-clones the ~5 MB, 8,182-model catalogue per call.
  - Called from `oga-http/src/routing.rs:237-240` and `:294`.
- **User cost:** slower model choice on auto-routed delegates, with each call repeating DNS, TCP, and TLS setup.
- **Evidence (inferred):** not called, to avoid real advisor traffic. 37 recent tasks consulted the advisor. Likely 100–300 ms per auto-routed dispatch.
- **Fix:** keep one static `Client`, and return `Arc<PricingCatalogue>`.
- **Effort:** S.

### P27. Network probes for parked tasks run one after another

- **Where:** `rust/crates/oga-service/src/waiting.rs:111-133`, called for each hold at `holds.rs:318`.
- **User cost:** on a flaky network with several parked tasks, one sweep can take minutes, which delays scheduled starts and rate-limit releases.
- **Evidence (inferred):** 4 hosts × all addresses × a 3 s timeout, per hold.
- **Fix:** probe once per sweep and share the result.
- **Effort:** S.

### P28. Binary and install weight

- **Where:**
  - `rust/Cargo.toml` has no `[profile.release]`.
  - `Makefile:93` installs a second `Resources/oga-server` beside the Tauri sidecar.
  - `Makefile:118` backs up the DB with `cp -p` on every install.
- **User cost:**
  - The release binary is 49 MB (about 21 MB of it tree-sitter tables), and `strip` saves 5.1 MB.
  - On dev machines only, the second copy is 48.9 MB and `~/.oga/oga.db.cutover-backup` is 2.5 GB. Released builds (`scripts/publish.sh`) ship only the sidecar.
- **Evidence (measured):** file sizes. LTO was not measured; that needs a full rebuild.
- **Fix:** `strip = true`, and try `lto = "thin"` with `codegen-units = 1`. Install over the sidecar path, and back up with `cp -c`.
- **Effort:** S.

### P29. Broker memory stays high after heavy reads

- **Evidence:**
  - Measured: RSS was 24.9 MB at the start of the audit. After heavy event reads it swung between 40 and 127 MB, and `footprint` held at 160 MB, 149 MB of it small allocations.
  - Inferred cause: large parsed payloads raising the allocator's high-water mark. No heap profiler was attached.
- **Fix:** comes from P9 (smaller default pages) and P11 (bounded payloads, no `rawText` in feeds). Re-measure after those.

---

## Part B: Correctness

### C1. One refused dependent leaves its siblings `queued` forever; follow-ups skipped

- **Where:**
  - `rust/crates/oga-service/src/lifecycle.rs:389`: `.await?` aborts the release loop.
  - `dispatch.rs:660-671`: the error path cleans up only the first task.
  - `dispatch.rs:656-657`: `outcome.task` is the last task the loop ran, not the one that was launched.
- **User cost:**
  - Tasks never start and show no error. Nothing picks them up again: the hold is deleted, the hold sweep ignores `queued` tasks, and a restart cancels them.
  - Queued follow-ups on a task that has dependents never start.
  - Rate-limit and network parking apply only to the last task in the loop (inferred).
- **Evidence (measured, harness):**
  - Disabling one dependent's model left both dependents `queued` with `err=None` 4 s after the blocker completed, including the one whose model was fine.
  - A follow-up started without a dependent, and did not start with one.
- **Fix:** the same as P4.
- **Effort:** S.

### C2. A dependent created as its prerequisite finishes waits for the sweep

- **Where:** `rust/crates/oga-service/src/dispatch.rs:379` reads the prerequisite's state, `:1112` awaits a `git` subprocess, and `:500` persists in a separate transaction.
- **User cost:** up to 30 s or more before the dependent starts (the sweep interval).
- **Evidence (measured):**
  - A harness left 3 of 20 dependents `pending` after their prerequisite was already `completed`.
  - The test `handoff_keeps_dependency_hold_until_prerequisite_completes` (`oga-service/tests/continuation.rs:1753`) fails 4 of 4 on `main` with "task did not settle". That it hits this exact path is inferred, but it matches the repro.
- **Fix:** re-check prerequisite states inside `persist_plan`'s transaction, and launch right away if they are met.
- **Effort:** S.

### C3. One slow `opencode --version` refuses every OpenCode 1 task until restart

- **Where:** `rust/crates/oga-providers/src/opencode.rs:33` (5 s timeout), `:186` (a failed probe is cached), and `:201-211` (polls with `thread::sleep` on the calling thread). Called synchronously from `oga-service/src/lifecycle.rs:444` and `acp.rs:378`.
- **User cost:** under load, every OpenCode 1 task fails with "no `opencode` it can find is OpenCode 1" until the binary changes or the broker restarts. The probe also blocks a runtime thread.
- **Evidence (measured):** `opencode --version` took 2.7–5.4 s for 1.18.32 and 1.1 s for 2.0.15, and the two are probed one after the other.
- **Fix:** don't cache failures, run the probe off the runtime thread, and allow a longer timeout.
- **Effort:** S.

### C4. New tasks can be missing from the sidebar

- **Where:** `web/src/state/sidebar-state.ts:330`. An update for an unknown task goes to `rereadIfDue` (`:318`), which refreshes at most once every 15 s.
- **User cost:** start two tasks close together (a `dependsOn` chain or a batch delegate) and the second doesn't appear. If it then sits pending and sends nothing else, it stays missing until an unrelated update arrives after the window.
- **Evidence (measured):** the first unknown task returns `"refresh"`, and a second within 15 s returns `"none"`.
- **Fix:** always refresh for an unknown task id. The 100 ms coalescing in `Sidebar.tsx` already limits the rate.
- **Effort:** S.

### C5. Cancel and force-complete drop the end of the run

- **Where:** `rust/crates/oga-service/src/cancel.rs:83-148`. `lifecycle.rs:1039` settles only a task still reading `running`, so the whole settle rolls back.
- **User cost:**
  - The cancelled task keeps a `running` turn, so `running_since` stays set (`oga-store/src/repo/mod.rs:29`) and the task may look like it is still running.
  - The run's duration and spend are not recorded.
  - Command-line runs lose their transcript.
- **Evidence (measured, harness):** events after cancel were `[created, started, transport_chosen, worker_spawned, cancelled]`, and turns were `[(1,'running')]`.
- **Fix:** have cancel close the turn, and let a settle after cancel still write events and usage without changing the state.
- **Effort:** S–M.

### C6. Writer-lock contention can fail worker writes

- **Where:**
  - `rust/crates/oga-store/src/connection.rs:223-247` uses one writer connection behind a mutex, with deferred transactions.
  - A context build writes in one transaction (`oga-context/src/store.rs:381`); see P14.
  - Cleanup runs `VACUUM` on the 2.7 GB file under the maintenance lock (`oga-store/src/maintenance.rs:86`).
  - `oga relearn` and `learn_routes` open the DB writable from the CLI process (`rust/apps/oga-cli/src/main.rs:532,610`).
- **User cost:** worker event writes stall. From another process they can fail with "database is locked" after the 5 s busy timeout, which can end a worker's turn with an error.
- **Evidence:** inferred from code.
- **Fix:** batch index writes (P14), use `BEGIN IMMEDIATE` for read-then-write transactions, and route CLI writes through the broker.
- **Effort:** M.

### C7. CLI task lookup is capped at 2,000 tasks

- **Where:** `rust/apps/oga-cli/src/main.rs:891-900` and `:1179`.
- **User cost:** at the current rate (994 tasks in 25 days), in about 25 days the oldest tasks drop past the cap. `oga inspect`, `resume`, `cancel`, and the rest will then answer "unknown task" even for a full id.
- **Evidence (measured):** task count and creation dates on the live DB.
- **Fix:** the same as P9.
- **Effort:** S.

### C8. Title-bar duration stops counting between updates

- **Where:** `web/src/screens/task/TaskDetail.tsx:416`. `TaskDetailSecondary` is built inside the header effect, and the 1 s tick doesn't re-run that effect (deps at `:419`).
- **User cost:** during a long quiet step, the title-bar time freezes while the sidebar's keeps counting.
- **Evidence:** inferred from code.
- **Fix:** see P16.
- **Effort:** S.

### C9. Wake recovery can cancel tasks that are still connecting

- **Where:** `rust/crates/oga-service/src/lifecycle.rs:190`. `supervises` checks only the live-process map; the "starting" map is used only by archive.
- **User cost:** an ACP task reads `running` during its 3–22 s connect but isn't registered yet, and dependents queued by P4's loop are never registered. A wake recovery would cancel them as "Stopped while this computer was asleep".
- **Evidence:** inferred from code. Low probability.
- **Fix:** count "starting" tasks as supervised.
- **Effort:** S.

### C10. Desktop app discards the broker's output

- **Where:** `rust/apps/oga-desktop/src/broker.rs:268-270` sets all three stdio streams to null.
- **User cost:** when the app starts the broker, messages such as "restart recovery failed" and "cleanup failed" are lost. There is no log to send with a bug report.
- **Evidence:** inferred from code.
- **Fix:** write stdout and stderr to `~/.oga/broker.log`, with rotation.
- **Effort:** S.

### C11. Code-index edge cases

- **Symbol cap skipped.**
  - Where: `rust/crates/oga-context/src/index.rs:201`. `walk.partial || truncate_to_budget(...)` short-circuits, and the reconcile merge at `:303` never applies the cap.
  - Measured: the home folder stored 236,772 symbols on a fresh build (cap 200,000), and the live row has 364,729.
- **Trailing-slash duplicates.**
  - Where: `rust/crates/oga-http/src/context.rs:65-69` uses `task.cwd` unnormalized.
  - Measured: one worktree has two indexes, and 308 tasks have a cwd ending in `/`.
- **`.gitignore` character classes.** The reader escapes `[...]` literally, so `*.py[cod]` never matches (inferred).
- **Effort:** S each.

---

## Part C: Needless complexity

- **X1. Two copies of the event-append helper.** `append_event_tx` exists in both `rust/crates/oga-service/src/lib.rs:128` and `lifecycle.rs:1825`, plus about 8 raw `INSERT INTO task_events` sites. This is why the size limit reached only hook events (P11). Fix: one append function that always bounds the payload. Effort: S.
- **X2. Delivery inbox with no client.** `rust/crates/oga-service/src/deliveries.rs` is 684 lines. `GET /api/consumers/{id}/inbox` writes rows, yet nothing in `web/src`, `oga-mcp`, or `oga-cli` calls it (inferred). This is the owner's call: keep it for an external consumer, or delete it. A GET that writes should become a POST either way.
- **X3. Two syntax highlighters.** refractor serves markdown and Shiki serves diffs (P17). One highlighter would remove a dependency and most of the 10 MB. Effort: M/L.
- **X4. Dead gap check in the sidebar.** In `web/src/state/sidebar-state.ts`, `applyEventBatch` (`:305`) moves the cursor to `batch.cursor` before any pointer is checked, so `applyPointer`'s gap check never fires on the batch path. Fix: delete it or make it real. Effort: S.

---

## Checked and fine

- **Broker basics:** `/health` 1 ms; single-task reads 2 ms; MCP `inspect` 2 ms (~400–500 tokens); `tools/list` 3 ms. Recovery on start takes ~0.15 s on the 2.6 GB DB.
- **SQLite setup:** WAL, `synchronous=NORMAL`, a 5 s busy timeout, checkpoints every 30 s, and a reader pool of up to 8. Event reads use the `(task_id, id)` index.
- **Event machinery:** the SSE handler already moves DB batches off the async threads. The event poller (50 ms while anyone listens) runs a cheap indexed query. The tracked-event window is capped at 4,096. No locks are held across awaits.
- **Idle cost:** broker CPU was 0–5% with 4 tasks running, and a 5 s `sample` showed it mostly parked. `oga watch` uses 0.05 s of CPU in 18 minutes. The hold sweep, wake watch, and checkpoint loops (every 30 s) are cheap.
- **Task lifecycle:** queued→started is 6–13 ms inside the broker. The active-run map uses short locks with no IO under them. Hold release claims are atomic. Resume/sweep and cancel/claim races are guarded by state checks and turn ids. Prompt building is cheap.
- **Config:** read from disk per request, but the files are tiny and the cost is negligible (143 B and 1.3 KB).
- **Code index:** change detection (size, mtime, ctime, content hash) is correct and cheap. Scoring is under 1 ms. Parallel queries on one project share one walk. Tree-sitter queries compile once.
- **Routing:** regexes are static `LazyLock`s, and routing inputs come from in-process caches. The pricing catalogue is served stale-while-revalidate.
- **CLI:** `--help` and `version` 20–40 ms, `inflight` 30–70 ms, `config` ~0.1 s.
- **Web app traffic:** the web view doesn't poll. The shell makes ~54 small requests a minute (`/api/activity` every 2 s at 13 B, health every 5 s). The sidebar summary is 45 KB for 50 tasks in 8–10 ms, with refreshes coalesced.
- **Web app rendering:** the trace list is virtualised (44 rows mounted at 150, 1,000, or 10,000 events). `RunChangeProjection` is incremental (0.01–0.06 ms per update). Markdown is memoised, and highlighting is lazy and runs when idle. Settings, Usage, TaskDetail, and the menu are lazy-loaded. The outcome caches are capped at 512 entries, and listeners are cleaned up.
- **Startup payload:** entry chunk 190 KB (58 KB gzipped), React 192 KB (60 KB gzipped), CSS 97 KB, fonts 115 KB.
- **Desktop:** the desktop process uses 41–49 MB, the web view 204 MB, GPU 21 MB, and the broker 24–39 MB idle. The tray's `/api/activity` poll takes ~5 ms. Event batching is fine.

## Coverage

| Area | Looked at |
|---|---|
| `oga-http` | Router, SSE, state, tasks, hooks, consumers, context, settings, usage, projects; ~20 GET routes timed live |
| `oga-events` | Poller, watch socket, event views, payload bounds; replay benchmarked |
| `oga-store` | Pragmas, pool, schema and indexes, repos, maintenance, migrations; query plans on the live DB |
| `oga-service` | dispatch, lifecycle, acp_run, holds, waiting, reconcile, cancel, resume, complete, handoff, archive, follow_ups, transport, prompt, acp_question, deliveries; harness runs |
| `oga-runner` | Spawn, capture, signals, cancel grace, confinement |
| `oga-acp` | Transport, session handshake, pending-request release |
| `oga-providers` | worker_path, OpenCode probes, stream parsing, adapters |
| `oga-context` | index, store, walk, routes, symbols, lang, query; instrumented cold and warm benchmarks |
| `oga-routing` | catalog, classify, policy, selection, traits |
| `oga-advisor`, `oga-pricing` | Client and network path; catalogue cache |
| `oga-mcp` | Request handling and results for `tasks`, `inspect`, `models`, `query`, `memory`; descriptions excluded |
| `oga-client` | Loopback client, desktop bridge |
| `oga-config`, `oga-domain` | Read path; types |
| `oga-worktree` | Creation, default copies, cleanup; timed on a throwaway clone |
| `oga-cli` | Startup order, command latency, `watch`, task resolution |
| `oga-desktop` | Broker supervision, tray polling, memory |
| `web/` | bridge, state, screens (sidebar, task, settings, usage), domain (activity, changes, markdown, review, trace), components, shell; build output; benchmarks with jsdom |

## Tests run

- **Rust:** `cargo test -p <crate>` for every crate in `rust/crates/` and both apps.
- **Web:** `bun test`, `bun run typecheck`, and `vite build` to `/tmp`.

Known failures on `main`, as briefed and not investigated: `models_honor_project_model_enablement` (oga-mcp `wire`), two archive-branch tests (oga-mcp `next`), and `consumer_routes` (oga-http `routes`).

Other failures, not on the known list:

| Test | Result | Notes |
|---|---|---|
| `oga-config` `love_rules_reject_unknown_fields_kinds_and_efforts` | Fails on `main` | The expected message lists kinds of work without `ux`; the code now includes it. |
| `oga-routing` `policy::tests::{merges_project_over_user_with_whole_allow_replacement, names_unsatisfiable_entries_and_stays_quiet_on_fallback_catalogs, matches_normalized_ids_with_anchored_globs, rejects_invalid_fields_with_their_paths}` | Fails on `main` (`policy.rs` unmodified) | TOML fixtures are parsed with `serde_yaml`. The run stopped at this binary, so the crate's other test targets did not run. |
| `oga-service` `handoff_keeps_dependency_hold_until_prerequisite_completes` | Fails 4/4 | "task did not settle". Consistent with C2. |
| `oga-context` `answers_plain_language_questions_about_this_repository` | Fails | Got `web/src/bridge/types.ts:55#TaskScope`. The test indexes the live working tree, which another task was editing; not confirmed on a clean tree. |
| Web `TraceRows > renders an image written on disk through the desktop bridge` | Fails on a clean export of `HEAD` | "Received value must be a string: undefined". |
| Web `AppShell`, two tests plus one unhandled error | Fail under the full run | Pass 5/5 when the file runs alone. Likely flaky under load; the other task's uncommitted `AppShell.tsx` edit may also be a factor. |

All other targets passed:

| Target | Passed |
|---|---|
| `oga-store` | 21 integration |
| `oga-events` | 110 unit, plus feed, socket, subagents |
| `oga-http` | 35 unit, 26/27 routes |
| `oga-service` | 74 unit, 76 acp, 23/24 continuation |
| `oga-runner` | 18 |
| `oga-acp` | 22 |
| `oga-providers` | 46 |
| `oga-advisor` | 15 |
| `oga-pricing` | 8 |
| `oga-client` | 25 |
| `oga-worktree` | 22 |
| `oga-cli` | 27 |
| `oga-desktop` | 25 |
| Web | 603 of 606 across 41 files; typecheck passes |
