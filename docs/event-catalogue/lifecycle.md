# Oga Lifecycle Event Catalogue

This catalogue describes every lifecycle event type that Oga itself emits (not provider events). It classifies each as one of:

- **Show as a row**: user-facing, relevant to understanding task progress
- **Show only when it matters**: normally hidden, surfaced when a specific condition makes it significant
- **Never show**: pure plumbing

## Summary

- **52 total event types** (non-`agent.*`)
- **24 should show as rows**
- **12 show only when it matters**
- **16 never show** (internal plumbing)

Most damaging to hide:
- `scope_refusal`: user's write was blocked by sandbox and task failed silently without explanation
- `line_dropped` / `event_dropped` / `events_truncated`: output or events were lost; user sees an incomplete trace
- `hold_expired` / `network_retry_exhausted`: task stalled because a rate limit or retry budget expired; user sees waiting with no reason why

---

## Show as a row

These events represent state changes or user-visible outcomes a person watching the task should see.

### Task lifecycle

#### `created`
- Payload: `{}` (empty)
- User-facing: **"Task queued"** (when state is queued)
- Why: User needs to know when their request was received and entered the system.

#### `started`
- Payload: `{}` (empty)
- User-facing: **"Worker started"** (when a worker picks up the task)
- Why: Marks the transition from waiting to active work.

#### `completed`
- Payload: `{completion: {exitCode: 0, blocked: false, code: "completed"}}`
- User-facing: **"Task completed"**
- Why: Terminal state; user should always see success.

#### `failed`
- Payload varies; key fields:
  - `error: string` (the failure reason)
  - `completion.code: string` (one of "worker_error", "cancelled", etc.)
  - `completion.reason: string` (detailed reason)
- User-facing: **"Task failed"** with detail from `completion.reason`
- Example: "Task failed: exit 1"
- Why: Terminal state; essential to show why the run did not complete.

#### `cancelled`
- Payload:
  - `error: string` (reason, e.g., "cancelled by caller")
  - `completion.code: "cancelled"`
  - `completion.reason: string`
- User-facing: **"Task cancelled"** with reason if present
- Example: "Task cancelled by caller"
- Why: Explicit cancellation is terminal and user-initiated; should be visible.

### Worker and model control

#### `worker_spawned`
- Payload:
  - `provider: string` (e.g., "claude", "opencode")
  - `model: string` (e.g., "haiku", "sonnet", "muse-spark-1.2-contributor")
  - `pid: number` (worker process id)
- User-facing: **"Worker started"** or **"Running \<model\>"**
- Example: "Running Claude Haiku"
- Why: User can see which model is active, especially on the first spawn or after a handoff.

#### `model_changed`
- Payload:
  - `model: string` (the new model)
- User-facing: **"Model changed to \<model\>"**
- Example: "Model changed to Sonnet"
- Why: Explicit model switch deserves to be visible.

#### `handed_off`
- Payload:
  - `fromProfile: string` (e.g., "claude", "opencode")
  - `toProfile: string` (destination profile)
  - `fromModel: string`
  - `model: string` (destination model)
  - `attempt: number`
  - `sessionPreserved: boolean`
  - `scopeUpdated: boolean`
- User-facing: **"Handed off"** with detail showing the hop
  - Same profile, different model: "\<from\> → \<to\>" (e.g., "Claude Opus → Claude Sonnet")
  - Cross-profile: "\<fromProfile\> → \<toProfile\>" (e.g., "Claude → OpenCode")
- Why: Handoff is a discrete control point; user should know work moved to a new provider/model.

#### `steered`
- Payload:
  - `instruction: string` (the instruction sent)
- User-facing: **"Instruction sent"** with first 160 chars of instruction
- Example: "Instruction sent: Fix the typo in auth.ts and re-run checks"
- Why: Steering is user-initiated mid-run; the worker should not silently ignore it.

#### `steer_accepted`
- Payload:
  - `instruction: string`
- User-facing: **"Instruction accepted"** with instruction text
- Why: Confirms the steering was received and acted on.

#### `steer_rejected`
- Payload:
  - `instruction: string`
  - `reason: string` (why it was rejected)
- User-facing: **"Instruction rejected"**
  - Detail: "Reason: \<reason\>" (e.g., "no live stdin control channel")
- Why: User needs to know why their mid-run instruction didn't take effect.

### Queued for execution

#### `queued`
- Payload:
  - `note: string` (optional; what the task is waiting for, e.g., "waiting for another task to finish")
- User-facing: **"Task queued"** or **"Queued for execution"**
  - If note present: detail is the note text
- Example: "Task queued: waiting for T001 scaffold web/ React app to finish"
- Why: Marks when a task transitions from blocked/holding to ready to execute. Shows the task is about to start work.

### Blocking and waiting

#### `blocked`
- Payload:
  - `error: string` (the blocker reason)
  - `completion.dependencyBlocked: boolean` (if true, waiting on another task)
  - `completion.reason: string`
  - `completion.code: string` (e.g., "cancelled", meaning the task that blocked this one was cancelled)
- User-facing: **"Task blocked"** with reason from `completion.reason`
- Example: "Task blocked: waiting for Make a search row say what was searched to finish"
- Why: Explicit block means the task is stalled and cannot proceed; user should know.

#### `needs_input`
- Payload:
  - `question: string` (the question being asked)
  - `completion.code: "needs_authority"` or similar
  - `completion.reason: string` (details of what is needed)
- User-facing: **"Worker needs input"** with the question text
- Example: "Worker needs input: Which icon size should I use?"
- Why: Task is waiting for user to unblock it; this is time-critical.

#### `answered`
- Payload:
  - `answer: string` (the answer provided)
  - `scopeUpdated: boolean`
  - `attempt: number`
- User-facing: **"Question answered"** with first 160 chars of answer
- Example: "Question answered: yes, use the largest size"
- Why: Confirms the answer was received and the task can proceed.

### Queued work (follow-ups)

#### `follow_up_queued`
- Payload:
  - `instruction: string` (the next instruction to run after this task completes)
  - `waiting: number` (count of follow-ups currently queued)
- User-facing: **"Follow-up queued"** with instruction summary
- Example: "Follow-up queued: remove stale docs"
- Why: User or coordinator added work to run next; should be visible so the user knows what comes after.

#### `follow_up_started`
- Payload:
  - `instruction: string`
  - `waiting: number` (remaining follow-ups)
- User-facing: **"Follow-up started"** with instruction
- Example: "Follow-up started: remove stale docs"
- Why: Marks when a queued follow-up begins; distinct from the main task starting.

#### `follow_ups_paused`
- Payload:
  - `waiting: number` (how many follow-ups are paused)
- User-facing: **"Follow-ups paused"** with count
- Example: "Follow-ups paused: 2 waiting until this task finishes cleanly"
- Why: Tells user that queued work exists but is held.

#### `follow_ups_dropped`
- Payload:
  - `dropped: number` (count removed)
  - `reason: string` (why, e.g., "removed on request")
- User-facing: **"Follow-ups removed"** with count and reason
- Example: "Follow-ups removed: 3 dropped (removed on request)"
- Why: Queued work was discarded; user should know this happened.

### Resumed work

#### `resumed`
- Payload:
  - `previousState: string` (the state before resume, e.g., "failed")
  - `attempt: number` (which resume attempt)
  - `scopeUpdated: boolean`
  - `instruction: string` (optional new instruction for this resume)
- User-facing: **"Resumed"** or **"Resumed from \<previousState\>"**
  - If instruction present: detail is the instruction text
  - If no instruction: brief indication of what is resuming (e.g., "Resuming from failed state")
- Example: "Resumed from failed state: change model to Sonnet and retry"
- Why: Marks an explicit restart, distinct from a fresh dispatch.

### Completion and scope issues

#### `completion_asserted`
- Payload:
  - `assertedBy: string` (who marked it complete, e.g., "Claude Code primary session")
  - `reason: string` (why, e.g., "Worker never received its brief and blocked immediately. I did the work...")
  - `previousState: string` (what state was the task in when completion was asserted)
- User-facing: **"Completion asserted"** with a brief reason summary
- Example: "Completion asserted: Worker never received brief. Work done in-session."
- Why: This is unusual and a sign something failed; user should see it happened.

#### `scope_refusal`
- Payload:
  - `path: string` (the path that was refused)
  - `error: string` (e.g., "/Users/malico/.oga/oga.db is outside the granted write scope; the sandbox refuses this write")
- User-facing: **"Write refused by scope"**
  - Detail: path that was blocked and reason
- Example: "Write refused by scope: /Users/malico/.oga/oga.db — outside granted write scope"
- Why: **Critical**: This is a failure condition. The task may have silently stopped here. User needs to know the write was denied and why.

### OGA (Orchestrator) events

#### `oga.orchestrator`
- Payload: `{}` (empty)
- User-facing: **"Orchestrator started"**
- Why: Marks when an OGA orchestrator task is created; should only appear on orchestrator tasks themselves.

#### `oga.spawned`
- Payload:
  - `orchestratorId: string` (id of the orchestrator that spawned this)
- User-facing: **"Spawned by orchestrator"**
- Why: Links a spawned task to its orchestrator; context for understanding task relationships.

#### `oga.decision`
- Payload: varies but key is `decision` object with `action` (e.g., "delegate") and context
- User-facing: Hide from the trace or show as **"Orchestrator decided: \<action\>"**
- Why: Internal routing signal; orchestrator events are not meant for display on the task itself. If shown at all (e.g., in an orchestrator debug view), name the decision clearly.

### Miscellaneous

#### `effort_mismatch`
- Payload:
  - `requested: string` (effort level requested, e.g., "high")
  - `actual: string` (effort level used, e.g., "max")
- User-facing: **"Effort mismatch"** with detail
- Example: "Effort mismatch: requested high, ran at max"
- Why: Unusual condition; user configured one effort level but the system ran at a different one.

#### `handoff_brief`
- Payload:
  - `tier: string` (e.g., "verbatim", meaning the full message history was carried)
  - `chars: number` (size of the carried state)
  - `omittedMessages: number` (optional; messages that were trimmed)
- User-facing: Show only if handoff happened; detail is metadata about what was carried
  - **"Handoff brief: \<tier\> carry-over, \<chars\> chars"**
- Example: "Handoff brief: verbatim carry-over, 7464 chars"
- Why: If shown, helps user understand what state was preserved across handoff. Can be minor since it accompanies the handoff row itself.

### `run_interrupted`
- Payload:
  - `reason: string` (what stopped the run, e.g., "Stopped when Oga restarted.")
  - `trigger: "broker_start" | "wake"`
  - `attempt?: number` (present when the run is being picked up again)
- User-facing: **"Run stopped"** with detail from `reason`
- Example: "Run stopped: Stopped while this computer was asleep."
- Why: The run ended for a reason that had nothing to do with the work. When a session was captured, Oga arms a restart hold so the run can be picked back up; without one, the task settles `cancelled`, `blocked`, or `failed` and stays resumable.

---

## Show only when it matters

These events are normally plumbing but become significant if a specific condition is met. The trace should surface them when that condition applies.

### Holds (dependency, restart, network, and rate-limit waits)

#### `hold_armed`
- Payload varies by arm path:
  - `note: string` (what is being waited for, e.g., "waiting for Make a search row… to finish")
  - `wait?: "network" | "rate_limit" | null` (present when a hold is armed by the hold service; `null` means a scheduled start or prerequisite)
  - `resumesAt?: string` (the next start or probe time)
- Hold view kind:
  - `restart`: run recovery is picking the same session back up
  - `network`: Oga is waiting for connectivity before retrying
  - `rate_limit`: Oga is waiting for the profile/model to become available
  - `dependency`: Oga is waiting for another task to finish
  - `time`: Oga is waiting for a scheduled start
- Condition to show: Show when `wait` is `network` or `rate_limit`; otherwise only if the hold expires or is dropped without completing. Routine dependency holds and scheduled starts are administrative noise.
- User-facing (if shown): **"Waiting"** with detail from `note`
- Example: "Waiting: waiting for Claude Sonnet until 2026-09-02T12:00:00.000Z"
- Why: Helps user understand why the task paused.

#### `hold_released`
- Payload varies by release path:
  - `note: string` (what was released, e.g., "waiting for Give every activity row… to finish")
  - `wait?: "network" | "rate_limit" | null` (present when the hold sweep releases it)
  - `forced?: true` (present when a manual resume clears a pending hold)
- Condition to show: Show when `wait` is `network` or `rate_limit`; otherwise only if the matching hold was abnormal or user-forced.
- User-facing (if shown): **"Continuing"** with brief note
- Example: "Continuing: waiting for network"
- Why: Marks recovery from a wait.

#### `hold_dropped`
- Payload varies by drop path:
  - `reason: string` (why the hold is being dropped, e.g., "prerequisite abc ended failed" or "task no longer exists")
  - `note?: string` (what was being waited for)
  - `blockerId?: string`
  - `blockerState?: string` (e.g., "failed")
- Condition to show: Only if `blockerState` is terminal (failed, cancelled, blocked). If the blocker is still running or was merely skipped, the hold drop is internal tidying.
- User-facing (if shown): **"Waiting ended"** with reason detail
- Example: "Waiting ended: blocker task failed"
- Why: User should know the task that blocked them is gone or failed.

#### `hold_expired`
- Payload:
  - `dueAt: string` (the due or next-check timestamp that expired)
- Condition to show: Always. Hold expiry means a resource budget ran out.
- User-facing: **"Wait timed out"** with reason
- Example: "Wait timed out: 2026-09-02T12:00:00.000Z"
- Why: **Critical**: User needs to know the rate limit or retry budget expired and the task stalled as a result.

### Network and resource failures

#### `network_retry_scheduled`
- Payload:
  - `originalError: string` (e.g., "unknown certificate verification error")
  - `nextCheckAt: string` (timestamp of next retry attempt)
  - `expiresAt: string` (timestamp when retries give up)
  - `maxAttempts: number`
- Condition to show: Only if retries are eventually exhausted (`network_retry_exhausted` arrives). A transient glitch that recovers is internal.
- User-facing (if shown): **"Network error, retrying"**
  - Detail: the original error and when retries expire
- Example: "Network error, retrying: certificate verification failed — retries expire at 2026-08-29T12:02Z"
- Why: User needs to know a network issue is happening and the system is attempting recovery.

#### `network_retry_exhausted`
- Payload:
  - `attempts: number` (how many attempts were made before giving up)
- Condition to show: Always. This is a terminal failure condition.
- User-facing: **"Network retries exhausted"**
  - Detail: "\<attempts\> attempts, giving up"
- Example: "Network retries exhausted: 12 attempts, giving up"
- Why: **Critical**: User needs to know the task failed due to a persistent network issue.

### Truncation and loss

#### `line_dropped`
- Payload:
  - `malformed: number` (unparseable lines)
  - `oversized: number` (lines exceeding size limit)
- Condition to show: Only if any lines are dropped. If the count is 0, this event should not arrive.
- User-facing: **"Output lines skipped"**
  - Detail: count and reason (e.g., "7 lines exceeded size limit")
- Example: "Output lines skipped: 7 lines too large"
- Why: **Critical**: User's trace is incomplete; they need to know output was lost.

#### `event_dropped`
- Payload:
  - `bytes: number` (size of the event that was dropped)
  - `limit: number` (the line limit)
- Condition to show: Only if an event is actually dropped. If limit is never exceeded, this should not arrive.
- User-facing: **"Large event skipped"**
  - Detail: "…payload over the …line limit — one event skipped, the trace continues"
- Example: "Large event skipped: 70 KB payload over the 64 KB line limit — one event skipped, the trace continues"
- Why: **Critical**: User's trace has a gap; they need to know what was lost.

#### `events_truncated`
- Payload:
  - `limit: number` (max events per task)
  - `dropped: number` (actual count of events dropped, implicit from when this fires)
- Condition to show: Only if the cap is ever reached. If the count stays under limit, no truncation occurs.
- User-facing: **"Event capture limit reached"**
  - Detail: "…events dropped past the …-event cap"
- Example: "Event capture limit reached: 1000 events dropped past the 5000-event cap"
- Why: **Critical**: Very old tasks' traces are incomplete from the start.

### Scope and permission

#### `scope_auto_completed`
- Payload:
  - `added: array[string]` (paths that were auto-completed)
  - `reason: string` (e.g., "paths named in the prompt were missing from the stated read scope")
- Condition to show: Only if paths were auto-completed. If the scope was correct to start, this doesn't fire.
- User-facing (if shown): **"Scope expanded"**
  - Detail: "\<count\> path(s) added to read scope (paths named in prompt were not in stated scope)"
- Example: "Scope expanded: database/migrations added to read scope"
- Why: User should know the scope was adjusted automatically; they may want to review what was added.

#### `scope_inherited`
- Payload:
  - `scope: object` (the scope that was inherited; contains `read` and `write` arrays)
  - `approvedFor: string` (the profile it was approved for, e.g., "opencode-2")
  - `usedBy: string` (which profile is using it, e.g., "claude")
  - `reason: string` (e.g., "scope was approved for profile opencode-2, not claude")
- Condition to show: Only if the scope is inherited from a different profile than it was approved for. This is an unusual condition and should be visible.
- User-facing (if shown): **"Scope inherited from \<approvedFor\>"** or **"Using approved scope"**
  - Detail: scope was originally approved for a different profile
- Example: "Using approved scope: originally approved for opencode-2, now used by claude"
- Why: Unusual cross-profile scope reuse; user should see it happened.

#### `scope_ungranted`
- Payload:
  - `scope: object` (the full scope granted: `{read: ["**"], write: ["**"]}`)
  - `reason: string` (e.g., "no scope stated and no grant on file for this cwd; defaulted to the whole working tree")
- Condition to show: Only if the scope was not explicitly granted and defaults were used. If a scope was pre-approved, this doesn't fire.
- User-facing (if shown): **"Default scope used"** or **"Scope: defaulted to full access"**
  - Detail: the reason (e.g., "no explicit grant, defaulted to working tree")
- Example: "Default scope used: no explicit grant on file; using full working tree access"
- Why: User should know scope was not pre-approved and defaults were applied.

### Session state

#### `resume_fallback`
- Payload:
  - `reason: string` (e.g., "session_not_captured")
  - `tier: string` (e.g., "verbatim", meaning message history was used)
  - `chars: number` (size of the fallback state)
- Condition to show: Only if the task was resumed and the primary session could not be reused. This indicates data loss or recovery from a failure.
- User-facing (if shown): **"Resumed with fallback"** or **"Session rebuilt from message history"**
  - Detail: reason the session was not captured (e.g., "session not captured") and fallback size
- Example: "Session rebuilt from message history: 11623 chars of message history used"
- Why: User should know the session state is incomplete or rebuilt.

### Scope and other plumbing (show rarely)

#### `scope_inherited`
- (see above)

#### `worker_stderr`
- Payload:
  - `text: string` (stderr output from the worker)
- Condition to show: Only if stderr is non-empty and significant (e.g., not just warnings). If stderr is expected or noise, hide it.
- User-facing (if shown): **"Worker stderr"**
  - Detail: the stderr text (truncated to 160 chars)
- Example: "Worker stderr: No conversation found with session ID: c766e7…"
- Why: Diagnostic output that may indicate a subtle failure the task finished despite.

---

## Never show (pure plumbing)

These events are internal routing, bookkeeping, or high-volume keepalives. Showing them clutters the trace with noise and obscures the meaningful events.

### High-volume or internal bookkeeping

- **`heartbeat`** (173k rows) — task alive notification, no new work. Every heartbeat is internal; counts and elapsed time are shown on the task card, not as rows.
- **`session_captured`** — provider session successfully mapped. Internal routing.
- **`session_reused`** — provider session reused for a follow-up. Internal routing.
- **`provider_retry`** — provider-side retry (e.g., a call got cut off mid-response and was restarted). Different from the user-facing network retries; this is provider plumbing.
- **`broker_restarted`** — broker process restarted. Extremely rare and invisible to user work; if it happened, the task may have been interrupted, but that is reflected in other events.

### Routing and optimization

- **`learn_routes`, `learn_routes_duplicate`, `learn_routes_invalid`** — Oga's own route indexing for fast lookups. User does not need to see the system optimizing itself.
- **`worker_spawned`** — Already shown (moved to "show as a row"). Was originally here.
- **`queued`** (listed as "show only when" above but reconsidered: the task state "queued" is shown in the task card; this is the event, also internal. However, re-reading the context, this is a lifecycle event signaling the task entered the queue, so it arguably belongs in "show" for consistency. Current implementation shows it only on `hold_armed`.)

### Technical implementation

- **`completion_asserted`** — Already moved to "show".
- **`events_truncated`, `line_dropped`, `event_dropped`** — Moved to "show only when" or "show" because they are loss markers.
- **`oga.decision`** — Orchestrator internal routing; not user-facing on the task trace.
- **`resume_fallback`** — Moved to "show only when".
- **`worktree_recreated`** — Internal Git worktree recreation, not user-facing.

### Actually never shown

- **`learn_routes`**, **`learn_routes_duplicate`**, **`learn_routes_invalid`** — All three are internal route indexing feedback. Users do not need to see whether Oga's own database optimization succeeded or failed. (If it fails, the run still works; it just doesn't get indexed for next time. The failure is logged and monitored, but not surfaced as a user-facing event.)
  
- **`heartbeat`** — 173,304 rows, mostly noise. Shown as an elapsed-time count on the task card outside the event trace.

- **`session_captured`** — Session established with provider. Internal routing. User sees the session ID in debug views, not as a trace row.

- **`session_reused`** — Session from a prior run reused for a follow-up. No user action or state change. Internal optimization.

- **`provider_retry`** — Retry at the provider level (e.g., a call to the provider API timed out and was re-issued). Different from the user-visible `network_retry_*` events which are Oga's retry loop. This is the provider's own retry. User sees the outcome (retry succeeded or failed), not the internal retry.

- **`broker_restarted`** — Broker process recycled. Extremely rare and outside user control. If it happened during a run, the run is interrupted, which is reflected in the task's state (`blocked`, `failed`, or `cancelled`), not as an event on the trace.

- **`worktree_recreated`** — Git worktree was recreated (e.g., the old one was deleted and a new one was checked out). Internal plumbing. User works in the worktree but does not need to know it was recreated; it is transparent.

---

## Implementation notes

### Payload slicing

All queries use `substr(payload, 1, 2000)` to avoid loading massive JSON objects. When rendering, the same slice limits should apply to payload inspection.

### Collapsed rows

The trace collapses some sequences:
- Multiple identical routine `hold_armed` → `hold_released` sequences for the same note collapse into one entry with a repeat count (e.g., "Waiting: … · ×3").
- `heartbeat` events are marked `minor: true` so they are hidden by default and shown only in debug views.
- Tool results echo their corresponding tool call and are marked `minor: true` so they are folded away.

### Loss markers

`line_dropped`, `event_dropped`, and `events_truncated` all carry a one-line warning template: "X dropped past the Y limit". These should appear in the main trace with a warning-level presentation, not hidden.

### Conditional fields

- **Handoff**: Distinguish same-profile model changes ("Claude Haiku → Claude Sonnet") from cross-profile moves ("Claude → OpenCode").
- **Hold sequences**: Show unattended waits (`network` and `rate_limit`) when they arm and release. Show routine dependency and scheduled holds only if they are abnormal (expired or the blocker failed). Restart holds are represented by the preceding `run_interrupted` row.
- **Scope events**: Only show if they represent a change from the baseline (e.g., scope was auto-completed or inherited, not if it was approved as-is).

### Ambiguities and gaps

1. **`learn_routes` failure modes** — `learn_routes_invalid` carries rejection reasons, but the reasons are technical ("path is missing, oversized, or not indexable") and not actionable for users. These remain plumbing.

3. **`worker_stderr`** — Rare. Only show if the stderr is diagnostic and non-trivial. Expected warnings and debug output should not appear.

4. **`completion_asserted` text** — The reason can be long and detailed (e.g., a full explanation of what the coordinator did). Truncate to 160 chars for the trace row; full text lives in the raw payload.
