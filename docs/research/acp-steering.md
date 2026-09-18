# Mid-turn steering over ACP

How Oga can hand a running worker a new instruction *while its turn is still in
flight*, per agent, and what the protocol itself offers.

Researched 2026-09-18 against `agent-client-protocol-schema` 1.7.0,
`@agentclientprotocol/claude-agent-acp` 0.79.0 (commit `d421f56`),
`sst/opencode` commit `5a83358`, and Google's `antigravity-acp` 1.1.1.

## TL;DR

- **Today Oga never steers mid-turn.** `steer` always falls through to the
  queue: `control_state` returns `steerable: false` on every transport, so the
  instruction is stored and replayed as a *new run* after the current one ends
  (`rust/crates/oga-service/src/steer.rs:61`, `:115`).
- **Claude Code — possible, natively.** `claude-agent-acp` implements a
  non-standard request, `_session/steering`, that injects the message into the
  turn the SDK is currently running and answers `{"outcome":"injected"}`. It
  advertises support at `InitializeResponse._meta.steering.supported`.
- **OpenCode — possible, by accident of design.** A second `session/prompt` on a
  busy session appends the user message and joins the in-flight run instead of
  starting a second one; the running loop re-reads the message list each step and
  picks the instruction up at the next step boundary. No cancel, no lost work.
- **Antigravity — unconfirmed.** Google ships a closed `agy_acp_server.par` with
  no public source and no published capability list. Nothing was verified; treat
  it as "no steering" until probed on the wire.
- **The protocol offers no steering.** ACP is silent on a second `session/prompt`
  mid-turn. The only thing it *sanctions* mid-turn is `session/set_mode`, which
  carries a mode id, not free text.
- **CLI transport can never steer.** CLI workers are spawned with
  `Stdio::null()` stdin (`rust/crates/oga-runner/src/lib.rs:382`), so there is no
  channel to write to.
- **Recommendation:** capability-detected steering — `_session/steering` where
  advertised, a concurrent `session/prompt` for OpenCode, and the existing queue
  as the fallback. Never cancel-and-re-prompt; it throws away the turn's work and
  is indistinguishable from a user cancel downstream.

## What Oga does today

### Over ACP

One run is one `session/prompt`. `converse` sends the prompt once and then only
races it against the update stream and the turn deadline — there is no branch
that can write anything else to the session while the prompt future is pending:

- `rust/crates/oga-service/src/acp_run.rs:293` — `converse` builds a single
  `session.prompt(...)` future, pins it, and `select!`s it against the deadline
  and `updates.recv()`.
- `rust/crates/oga-acp/src/session.rs:359` — `AcpSession::prompt` is the only
  path that sends `session/prompt`, and it is `&self`-async, awaiting one
  response.
- `rust/crates/oga-acp/src/session.rs:384` — `AcpSession::cancel` sends
  `session/cancel`; `rust/crates/oga-service/src/acp_run.rs:87` — `AcpRun::cancel`
  additionally terminates the process, so cancel is a *stop*, not a nudge.

The live run handle reachable from the dispatcher is
`ActiveRun::Acp(Arc<AcpRun>)` (`rust/crates/oga-service/src/lifecycle.rs:76`),
and its only behaviour is `cancel`/`cancel_now`
(`rust/crates/oga-service/src/lifecycle.rs:81`). The `AcpSession` inside it is a
private field, so nothing outside `acp_run` can talk to the session at all.

### What `steer` actually does

`steer` refuses or queues; it never reaches a worker.

- `rust/crates/oga-service/src/steer.rs:45` — `control_state` returns
  `steerable: false` in *all three* branches, the last one unconditionally:
  `"this runner has no live stdin control channel"`.
- `rust/crates/oga-service/src/steer.rs:114` — with an instruction and no model
  change, it calls `queue_for_later`.
- `rust/crates/oga-service/src/steer.rs:131` — `queue_for_later` writes a FIFO
  follow-up row and returns `queued: true`. Its own comment states the current
  reality: *"Nothing can reach the worker mid-run, so the instruction waits its
  turn behind the current one."*
- `rust/crates/oga-service/src/dispatch.rs:930` — after the run settles,
  `drain_follow_ups` takes the head of the queue and issues a **`resume`**, i.e.
  a fresh run with `session/load` (or `session/resume`) plus a new
  `session/prompt` (`rust/crates/oga-acp/src/session.rs:473` `open_session`).

So `oga steer <id> -m "..."` is really "queue a follow-up turn". The MCP
description — *"leaves an instruction for one still running"* — is accurate about
the queueing and silent about the delay.

### Over the CLI transport

Impossible by construction. A captured CLI run is spawned with
`std::process::Stdio::null()` for stdin
(`rust/crates/oga-runner/src/lib.rs:382`). Only `spawn_duplex`
(`rust/crates/oga-runner/src/lib.rs:434`) pipes stdin, and that exists solely for
ACP's JSON-RPC transport. `control_state`'s "no live stdin control channel"
message is literally true there.

## What the protocol offers

**A second `session/prompt` mid-turn is undefined.** The spec describes a turn as
a request that runs to a stop reason, and only says what happens *after*:

> "Once a prompt turn completes, the Client may send another `session/prompt` to
> continue the conversation, building on the context established in previous
> turns."
> — <https://agentclientprotocol.com/protocol/prompt-turn>

There is no MUST, SHOULD, or MAY covering a prompt sent *during* a turn, and no
queueing semantics an agent is obliged to implement. Any client that sends one is
relying on the agent's implementation, not on the protocol.

**Cancel is a stop, not a nudge.**

> "Clients **MAY** cancel an ongoing prompt turn at any time by sending a
> `session/cancel` notification"
> — <https://agentclientprotocol.com/protocol/prompt-turn#cancellation>

The agent must stop model requests and abort in-flight tool calls, then answer
the original `session/prompt` with `StopReason::Cancelled`
(`agent-client-protocol-schema-1.7.0/src/v1/agent.rs:3176`). Nothing in the
notification carries a payload, so cancel cannot itself deliver an instruction.

**`session/set_mode` is the one method the spec allows mid-turn.**

> "This method can be called at any time during a session, whether the Agent is
> idle or actively generating a response."
> — `agent-client-protocol-schema-1.7.0/src/v1/agent.rs:4898`, mirrored at
> <https://agentclientprotocol.com/protocol/session-modes> ("The current mode can
> be changed at any point during a session, whether the Agent is idle or
> generating a response.")

It takes a `SessionModeId` from the agent's advertised `availableModes`
(`.../v1/agent.rs:1892`) — a switch between "ask"/"architect"/"code"-style modes,
not free text. Useful for tightening permissions mid-run; useless as a steer.

**Extensions are the sanctioned escape hatch.** ACP reserves `_meta` on every
request and response for client/agent extensions
(<https://agentclientprotocol.com/protocol/extensibility>), and underscore-prefixed
methods are the convention agents use for non-standard requests. That is exactly
where Claude's steering lives.

## Claude Code — `@agentclientprotocol/claude-agent-acp`

**Verdict: possible.** A dedicated extension request injects into the running
turn.

**Method.** `_session/steering`, registered on the connection at
`/tmp/caacp/src/acp-agent.ts:10123` (repo:
<https://github.com/agentclientprotocol/claude-agent-acp>, commit `d421f56`,
version 0.79.0).

```
/** Custom (extension) request method a client uses to steer the turn that is
 *  currently running: the message is injected into the in-flight turn rather
 *  than queued as a separate `session/prompt`. Named `_session/steering` per the
 *  agreed ACP steering wire protocol; advertised to clients via the top-level
 *  `InitializeResponse._meta.steering.supported`. */
const STEER_METHOD = "_session/steering";
```
— `src/acp-agent.ts:445`

**Discovery.** `initialize` answers with a top-level `_meta.steering.supported =
true`, a sibling of `agentCapabilities` (`src/acp-agent.ts:2135`, `:2142`). The
comment there calls it "the existing ACP steering extension contract".

**Params and result** (`src/acp-agent.ts:503`, `:513`):

```ts
export type SteerRequest = {
  sessionId: string;
  prompt: PromptRequest["prompt"];   // same content blocks as session/prompt
  _meta?: { steering?: { idleBehavior?: "promptRequired" } } | null;
};

export type SteerResponse =
  | { outcome: "injected" }
  | { outcome: "startedNewTurn" }
  | { outcome: "promptRequired"; reason: "noRunningTurn" };
```

**Behaviour** (`src/acp-agent.ts:2998` onward):

- A turn is "running" when `session.turnQueue` holds an unsettled turn. The
  in-flight check and the push happen in one synchronous section, so the turn
  cannot settle in the gap (`src/acp-agent.ts:3010`–`:3016`).
- Running → it pushes an `SDKUserMessage` onto the same streaming input the SDK
  is consuming. It does **not** create a Turn or touch `turnQueue`, so the
  injected message does not become its own turn (`src/acp-agent.ts:3046`–`:3067`).
- Delivery priority is the agent's own choice, not the wire's: `now` pre-empts
  the current generation — "interrupting a single-shot response, or slotting in
  between a multi-step turn's tool calls" — and drops to `later` while a
  permission or elicitation is awaiting the user, because interrupting that
  callback cancels the ACP request (`src/acp-agent.ts:486`, `:2982`–`:2987`).
- Pre-empting *aborts* the current model cycle: the aborted cycle emits its own
  `result`, so the turn is marked (`Turn.steeredEchoes`) to settle at the SDK's
  `idle` rather than on that result (`src/acp-agent.ts:2989`–`:2991`). The
  original `session/prompt` still resolves once, with its normal stop reason.
- The steered message's output streams over `session/update` as usual; the
  `_session/steering` response carries no content (`src/acp-agent.ts:2986`).
- Idle → default is a detached new turn, answering `startedNewTurn`. A host that
  sets `_meta.steering.idleBehavior = "promptRequired"` instead gets
  `{"outcome":"promptRequired","reason":"noRunningTurn"}` with **nothing pushed
  and no queue mutated**, so the host keeps ownership of the text and can submit
  it as a normal `session/prompt` (`src/acp-agent.ts:3017`–`:3025`). This is the
  variant Oga wants: it removes the race where a steer lands just as a turn
  settles and silently starts a turn Oga is not awaiting.

**Plain `session/prompt` mid-turn** is also accepted and is FIFO-queued rather
than rejected — `turnQueue` is documented as "FIFO of in-flight prompts. The head
is the turn the SDK is currently processing; later entries are queued and will be
echoed in order" (`src/acp-agent.ts:698`). That is a *queued follow-up turn*, not
a steer: it does not reach the running turn. Zed's agent panel shows the same
distinction, and its queued-message path has known stalls behind long tool calls
(<https://github.com/zed-industries/zed/issues/57761>).

## OpenCode — `opencode acp`

**Verdict: possible**, via a plain concurrent `session/prompt`. There is no
steering extension; the behaviour falls out of how OpenCode's session runner
works. Repo: <https://github.com/sst/opencode>, commit `5a83358`.

**The ACP layer is a thin proxy.** `ACP.prompt` converts the content blocks and
calls the OpenCode server's `session.prompt` for the backing session, wrapped in
`runUntilIdle` so the ACP response waits for the session's `idle` event
(`packages/opencode/src/acp/service.ts:509`, `:524`; `runUntilIdle` at
`.../acp/service.ts:91` and `.../acp/event.ts:74`).

**A second prompt joins the running run rather than starting one.** The core
`prompt` writes the user message into the session and then calls `loop`
(`packages/opencode/src/session/prompt.ts:1057`, `:1070`). `loop` delegates to
`SessionRunState.ensureRunning` (`.../session/prompt.ts:1346`), and the runner's
`ensureRunning` in the `Running` state returns the *existing* run's deferred
instead of starting a second fiber (`packages/opencode/src/effect/runner.ts:115`,
`:120`–`:122`). No `BusyError` is raised: `assertNotBusy`
(`packages/opencode/src/session/run-state.ts:71`) guards revert and one HTTP
route, not the prompt path.

**The running loop picks the new message up at the next step.** `runLoop`
re-reads the message list at the top of every iteration
(`packages/opencode/src/session/prompt.ts:1092`) and only exits when the last
assistant message's parent is the last user message:

```
if (
  lastAssistant?.finish &&
  !["tool-calls", "unknown"].includes(lastAssistant.finish) &&
  !hasToolCalls &&
  lastAssistant.parentID === lastUser.id
) { ... break }
```
— `packages/opencode/src/session/prompt.ts:1111`–`:1129`

A message appended mid-turn makes `lastUser` the new message, so
`lastAssistant.parentID === lastUser.id` fails and the loop continues with the
instruction in context. That is a genuine mid-turn steer at step granularity: it
lands after the current model step and any in-flight tool call, not inside them.

**Consequences Oga must handle.** Both `session/prompt` calls resolve from the
same run and the same `idle` event, so the second one returns a `PromptResponse`
describing the *same* turn. Oga's `converse` awaits only its own prompt future,
so a steer sent as a second `session/prompt` would resolve alongside it. Unlike
Claude, the steer is not rendered as a separate lane — it becomes another user
message in the session transcript.

**Cancel is whole-session abort.** `session/cancel` maps to
`sdk.session.abort` for the backing session
(`packages/opencode/src/acp/service.ts:357`, `:336`), and
`SessionRunState.cancel` additionally cancels the session's background jobs
(`packages/opencode/src/session/run-state.ts:77`). Cancel-then-re-prompt here
costs the turn *and* any background work it started.

**Modes.** OpenCode advertises modes and implements `setSessionMode`
(`packages/opencode/src/acp/service.ts:507`); the mode id is passed as `agent` on
each prompt (`.../acp/service.ts:517`, `:534`). Mid-turn mode changes therefore
only take effect on the next prompt, not on the running one. *(Inferred from the
call site; not separately confirmed against a running server.)*

## Antigravity — `antigravity-acp` (`agy_acp_server.par`)

**Verdict: unconfirmed — assume no steering.**

Oga runs Google's own ACP server, pinned to build `agy_acp_server_1.1.1`
(`rust/crates/oga-providers/src/acp.rs:504`, `:514`). Facts that are confirmed:

- The ACP registry entry is published by Google and starts
  `./agy_acp_server.par` — <https://zed.dev/acp/agent/antigravity-acp>. That page
  lists no methods, capabilities, cancellation, or mode behaviour.
- It is a closed binary. There is no public source repository for
  `agy_acp_server.par`; the `agy` CLI itself has no ACP mode
  (<https://github.com/google-antigravity/antigravity-cli/issues/31>).
- Oga already configures it through standard session config options, `model` and
  `mode: yolo` (`rust/crates/oga-providers/src/acp.rs:530`), so it does
  implement `session/set_session_config_option`.
- Every ACP agent must support `session/new`, `session/prompt`, `session/cancel`
  and `session/update` (`agent-client-protocol-schema-1.7.0/src/v1/agent.rs:4002`),
  so those are safe to assume and nothing else is.

**Not confirmed, and not guessed at:**

- Whether it advertises `_meta.steering.supported`. Oga does not currently read
  that field from any agent, so no existing handshake log answers it.
- What it does with a `session/prompt` arriving during a running prompt: reject,
  queue, run concurrently, or corrupt the session.
- Whether it advertises `availableModes` and honours `session/set_mode` mid-turn.

The third-party `antigravity-acp`/`agy-acp` projects found on GitHub
(<https://github.com/sibbl/google-antigravity-acp>,
<https://github.com/jiridanek/agy-acp>) are **not** the binary Oga runs and their
source says nothing about Google's. Do not reason from them.

**How to settle it** — one probe, no code change: start a session, send a long
`session/prompt`, then send `_session/steering` and, separately, a second
`session/prompt`, and record the JSON-RPC responses and the handshake `_meta`.
That is the only primary source available for a closed binary.

## What Oga should do

### Recommendation: capability-detected steering, queue as fallback

Keep the existing follow-up queue as the floor, and add a live path in front of
it:

1. **Record what the agent offered.** `AcpSession` already retains the whole
   `InitializeResponse` (`rust/crates/oga-acp/src/session.rs:237`), so
   `_meta.steering.supported` is readable without another round trip. Expose it
   alongside `agent_capabilities()`.
2. **Reach the live session.** `ActiveRun::Acp(Arc<AcpRun>)` currently only
   exposes `cancel` (`rust/crates/oga-service/src/lifecycle.rs:81`). Add a
   `steer(&self, text) -> SteerOutcome` that forwards to the session. The
   transport already takes an arbitrary method name
   (`rust/crates/oga-acp/src/transport.rs:209`), so `_session/steering` needs no
   new plumbing.
3. **Per-agent strategy**, chosen from the handshake, not from the provider name:
   - `_meta.steering.supported == true` → send `_session/steering` with
     `_meta.steering.idleBehavior = "promptRequired"`. `injected` → report
     delivered. `promptRequired` → the turn settled under us; fall through to the
     queue, where the instruction becomes the next run.
   - No steering meta, agent known to accept a concurrent prompt (OpenCode) →
     send a second `session/prompt` on the same session, discarding its response
     (`converse` already owns the turn's outcome).
   - Anything else, including Antigravity until probed → queue, exactly as today.
4. **Make `control_state` tell the truth.** It should answer `steerable: true`
   with the mechanism, so `oga inspect` and the MCP `next` hints stop telling
   callers a live run cannot be steered when it can.

### Alternatives considered

**Cancel + re-prompt with the steer folded into the next prompt.** Universal —
every ACP agent must implement `session/cancel` — and needs no capability
detection. Rejected as a default:

- It destroys the turn's in-progress work. The agent must abort in-flight tool
  calls (<https://agentclientprotocol.com/protocol/prompt-turn#cancellation>);
  a half-finished edit or a running build is lost.
- Oga's own cancel path terminates the process
  (`rust/crates/oga-service/src/acp_run.rs:87`), so it would need a
  cancel-without-kill variant first.
- The run settles as `StopReason::Cancelled`, which Oga maps to a cancelled
  worker outcome (`rust/crates/oga-service/src/acp_run.rs:589`). Steering would
  become indistinguishable from a user cancel in the task record unless a new
  outcome is threaded through.
- For OpenCode it is strictly worse than doing nothing: cancel also kills the
  session's background jobs.

Keep it available as an *explicit* `--interrupt` flavour of steer, where the
caller has said the current work should be abandoned. Do not make it the default.

**A second `session/prompt` everywhere.** Tempting — it is a standard method —
but the spec does not define it mid-turn, so behaviour is per-agent: Claude
FIFO-queues it as a *separate* turn (not a steer), OpenCode folds it into the
running one, and Antigravity is unknown. Sending it blind risks a second turn
Oga is not awaiting, or a wedged session. Use it only where the agent's source
says it works.

**`session/set_mode` as the steering channel.** The only method the spec allows
mid-turn, and it works on any agent that advertises modes. But it carries a mode
id, not an instruction, so it can only express choices the agent already defined
— tighten permissions, switch to plan mode. Worth having for a future
`oga steer --mode plan`; it is not a general steer.

**Model change mid-turn.** Out of reach on every path. Model is a session config
option chosen before the prompt (`rust/crates/oga-providers/src/acp.rs:530`;
`rust/crates/oga-acp/src/session.rs:552` `configure`), and `steer` already
refuses an instruction+model pair with a pointer to `handoff`
(`rust/crates/oga-service/src/steer.rs:117`). That stays correct.

### Trade-offs of the recommendation

- **Costs:** three code paths instead of one, and per-agent behaviour that can
  drift when an adapter is upgraded. `_session/steering` is an extension, not a
  standard: Claude may rename or change it, and the pin in
  `rust/crates/oga-providers/src/acp.rs:314` is the only thing that would catch it.
- **Buys:** the instruction lands in seconds rather than after the turn, without
  losing the turn's work, on the two agents where it is verifiable — and the
  fallback is exactly today's behaviour, so nothing regresses where it is not.
- **Reporting:** a delivered steer and a queued steer are different events and
  should stay different in the task record. `SteerOutcome.queued`
  (`rust/crates/oga-service/src/steer.rs:16`) already carries the distinction;
  the CLI and MCP copy should say which happened, in the user's words.

## Open questions

1. **Antigravity's behaviour is entirely unmeasured.** Needs the wire probe
   described above before any claim is made about it.
2. **Is `_session/steering` spoken by anyone but Claude?** The adapter's comments
   call it "the agreed ACP steering wire protocol" and "the existing ACP steering
   extension contract" (`src/acp-agent.ts:447`, `:2133`), implying a cross-vendor
   agreement, but no published spec for it was found. Unconfirmed.
3. **Does a `now`-priority steer damage a tool call in flight?** The adapter says
   pre-empting aborts the current cycle and that it falls back to `later` only for
   pending permission/elicitation callbacks (`src/acp-agent.ts:2986`–`:2991`).
   Whether an in-flight `Bash` or edit is rolled back, completed, or orphaned was
   not verified.
4. **OpenCode step granularity in practice.** The loop-exit condition proves the
   instruction is picked up, but how long "next step boundary" takes during a long
   tool call was not measured.
5. **Does a steer need to survive a broker restart?** The queue does, by design
   (`rust/crates/oga-service/src/follow_ups.rs:1`). A live steer does not, and the
   two answers differ — a steer delivered into a turn whose broker then dies leaves
   no record that it was delivered.
6. **Turn accounting.** A steered message adds tokens and possibly tool calls to a
   turn Oga already billed as one prompt. Whether the usage report covers the
   injected work was not checked.
