# Oga

Oga is a local broker that delegates bounded tasks to external AI provider
CLIs — Claude Code, Codex, OpenCode, Antigravity, and Pi. You hand it a prompt
and a working directory; it picks a model, spawns a sandboxed worker on a
different account, and returns a task ID the moment dispatch succeeds. The
worker runs independently while you get on with other work. When the task needs
input, hits a question, or finishes, Oga tells you — through a backgrounded
`oga watch` process that sleeps for free and reports the moment anything
settles.

Oga runs on macOS as a menu-bar app with a built-in SQLite store, a local
HTTP+MCP API on `127.0.0.1:7331`, and a unix event socket for push delivery.
It is for developers and coding agents who want to fan out implementation,
research, review, and analysis across providers and accounts without paying
context-window cost for work they are not actively steering.

## Why delegation

A bounded task — implement this module, review that diff, research a question —
can run on an external account with scoped filesystem access. The caller writes
the prompt, approves a data scope, and gets back a task ID. From there the
worker runs unattended; the caller checks in when it settles.

This matters for three reasons:

**Separate accounts, separate limits.** Each provider has session caps and
weekly quotas. Delegating spreads work across accounts so a rate limit on one
does not block the whole session.

**No context tax.** A delegated worker's prompt and output do not consume the
caller's context window. The caller pays a few hundred characters for the task
ID and state — not 80,000 characters of re-shipped prompt every time it checks
in.

**Provider choice per task.** A mechanical rename goes to a cheap fast model;
a concurrency audit goes to a frontier model. Oga routes automatically from
the prompt and the project's `.oga.yaml` policy, or the caller names a
profile and model explicitly.

## Install

Oga requires macOS 14 or later.

**curl:**

```bash
curl -fsSL https://downloads.desgn.space/oga/install.sh | sh
```

**Homebrew:**

```bash
brew install --cask DesgnSpace/tap/oga
```

**Build from source:**

```bash
git clone https://github.com/yondifon/oga.git
cd oga
bun install
make install
```

The installer adds Oga to Applications and links `oga` in
`~/.local/bin`. Open Oga to start the broker. Confirm it is running with:

```bash
curl http://127.0.0.1:7331/health
```

### After install

**MCP client config:** The app's **Install MCP** button writes a global MCP
entry for every installed client — Claude Code, Codex, OpenCode, and
Antigravity — pointing at `http://127.0.0.1:7331/mcp`. Existing configs get a
`.bak`.

**First launch** detects which provider CLIs are installed and lets you add,
edit, and enable profiles. No accounts are hardcoded. Each profile stores its
provider, default model, and environment variables; secret-like values
(`KEY`, `TOKEN`, `SECRET`, `PASS`) are masked on every surface.

## Command line

`make install` links the compiled binary onto your PATH as `oga`. It carries
its own runtime, so every command below works with no Bun and no checkout.

| Command | What it does |
| --- | --- |
| `oga` | Print help — what Oga is, the commands, and a first run. |
| `oga serve` | Run the broker. The menu-bar app starts it for you. |
| `oga watch <task-id>...` | Wait for a task; prints one line when it settles. |
| `oga inflight` | List the tasks still running, so you know what a restart interrupts. |
| `oga config [cwd]` | Print the effective config for a directory — profiles, models, routes, worker rules — and which file each setting came from. |
| `oga love [worker/model]` | Show or set the model used for work that names no model. Add `--clear` to return to per-task selection or `--global` to apply the choice to every project. |
| `oga version` | Print which build this is. |

## Set up workers

Open Oga's Settings screen after the first launch. Add or edit a worker with a
worker ID, display name, command-line tool, default model, capabilities, and
environment variables. The supported provider names are exactly:
`claude`, `codex`, `opencode`, `opencode-2`, `antigravity`, and `pi`.

Each worker is one provider account. The account directory always comes from
the worker, never from the shell Oga was started in. Claude workers each get
their own — `$HOME/.claude` for the worker named `claude`, `$HOME/.<worker-id>`
for the rest — while `CODEX_HOME` defaults to `$HOME/.codex` and
`PI_CODING_AGENT_DIR` to `$HOME/.pi/agent`. Set one of them in a worker's
environment to point that worker at a directory you have already signed into.
Secret-like environment keys containing `KEY`, `TOKEN`, `SECRET`, or `PASS` are
masked in the app and in `oga config` output.

The Settings screen controls which workers and models can receive work. A model
must be enabled for a worker before Oga can send work there. You can mark one
worker/model pair as loved, which becomes the default for unnamed work.

You can inspect the effective setup from the terminal:

```sh
oga config
oga love
```

To choose the default explicitly, use the worker ID and model ID:

```sh
oga love opencode/openai/gpt-5.6-luna
```

Use `oga love --clear` to let routing choose per task again. Add `--global` to
any `love` command to read or write the user-wide setting instead of the current
project's `.oga.toml`.

## Following a task

A delegated task runs unattended. Following it should cost nothing while it
works. Oga gives you three layers, from cheapest to richest:

**1. Background `oga watch <taskId>`** — the default. A backgrounded process
that connects to the broker's unix event socket at `~/.oga/oga.sock` for
instant push delivery, with a SQLite fallback when the socket is absent. It
sleeps for free, prints lifecycle and error lines as they happen, and exits
when any watched task settles. Exit codes carry the news on their own — 0 means
something settled, 1 means the deadline passed with nothing, 2 means bad input.

In Claude Code, a background command that exits re-invokes the agent — so
`oga watch` *is* the notification.

```sh
oga watch 8f2c1a94-... --timeout 30m &
```

**2. MCP `wait`** — a short deliberate block. Blocks up to 30 seconds and
returns the instant attention is needed. Use `until: "attention"` for a sanity
check right after dispatch, or in a harness with no background shell.

**3. `inspect`** — read one task's full record: output, scope, spend,
completion, and (on request) prompt and attempts.

Read [docs/follow-along.md](docs/follow-along.md) for the full story.

## Tool surface

Every tool is available over MCP (`http://127.0.0.1:7331/mcp`) and the REST API.

| Tool | What it does |
| --- | --- |
| `delegate` | Start scoped work. Auto-routes or takes an explicit profile/model. |
| `models` | List preferred, enabled models ready for `delegate`. |
| `wait` | Block briefly for progress or attention. |
| `inspect` | Full record of one task. |
| `tasks` | List tasks by state, time, profile, or fan-out batch. |
| `reply` | Answer a `needs_input` question on the same provider session. |
| `resume` | Retry a failed, cancelled, or blocked task on the same session. |
| `handoff` | Move a dead task to a different profile, keeping the same task ID. |
| `cancel` | Stop a task and its worker process tree. |
| `complete` | Assert completion when work demonstrably landed but the worker never attested it. |
| `archive` | Soft-hide old tasks without deleting history. |
| `memory` | Durable project facts shared across callers and workers. |
| `health` | Broker and MCP contract versions. |

Every task-returning tool accepts a `fields` selector that controls the
response payload. Defaults are minimal — `cancel` returns just `id` and
`state` — because the caller already has the data it sent. Read
[docs/fields.md](docs/fields.md) for per-tool defaults and the group table.

## Architecture

```
  MCP client       MCP / HTTP        Oga broker       sandbox-exec       provider CLI
  (Claude,    ─────────────────→    127.0.0.1:7331   ────────────────→     worker
   Codex, …)  ←── task view           Rust broker      ←── events, text    (subprocess)
                                   │
                           ┌───────┴────────┐
                           │  SQLite (WAL)   │
                           │  ~/.oga/      │
                           │  oga.db       │
                           └───────┬────────┘
                                   │
                           ┌───────┴────────┐
                           │  event socket   │
                           │  ~/.oga/      │
                           │  oga.sock     │
                           └───────┬────────┘
                                   │
                           ┌───────┴────────┐
                           │  desktop app    │
                           │  Tauri + Leptos │
                           └────────────────┘
```

- **Broker** (`rust/apps/oga-cli` and `rust/crates/oga-service`): Rust HTTP
  server on loopback-only `127.0.0.1:7331`, started by `oga serve`. Handles
  delegation, lifecycle, MCP, and REST.
- **Providers**: Claude Code, Codex, OpenCode, Antigravity, and Pi. Each
  spawned as a sandboxed subprocess with per-task read/write scope enforced by
  `sandbox-exec`. Scope is relative to the task's `cwd`; literal file paths stay
  literal, `dir/**` is recursive, `**` grants the whole tree.
- **Event socket** (`rust/crates/oga-events`): Unix-domain socket at
  `~/.oga/oga.sock`. Pushes task-event batches to local subscribers on the
  NDJSON protocol (contract EC-001). `oga watch` consumes it for zero-poll
  follow-along; the desktop app can consume it too.
- **Desktop app** (`rust/apps/oga-desktop`): Tauri shell over the Leptos UI in
  `rust/crates/oga-ui`, showing broker health, recent tasks with full event
  traces, and profile management. Ships the broker binary as a bundled sidecar.

## Configuration

Configuration resolves through three layers, highest first:

1. `<project>/.oga.yaml` — the project's own file, resolved from the task's `cwd`
2. `~/.oga.yaml` — your personal config
3. Oga's built-in defaults — discovered accounts and the shipped worker rules

A missing file at any layer is normal. A file that fails to parse fails the
read that consulted it, naming the file. `oga config [cwd]` prints the
effective config for a directory and which file each setting came from.

### Routing policy

A `.oga.yaml` constrains which provider/model pairs may run in a project.
Rules name providers and models, never local profile IDs. Oga still chooses
between matching local accounts using their availability.

Route names describe the work: `mechanical`, `context`, `build`, `reasoning`,
`general`. Each route has a provider/model `allow` list and optional
`preference` and `min_quality`.

Routes layer like everything else: a project file's `[routes]` table merges
over the user file's, per class. Scalar fields (`preference`, `min_quality`)
override; an `allow` list the project writes replaces the user's whole list,
because allow lists are written best-first and merging two would scramble their
meaning. A class neither file mentions stays unconstrained.

### Profiles

A `[profiles.<id>]` table tunes one profile:

```yaml
profiles:
  claude-work:
    label: Work Claude
  claude-personal:
    enabled: false
```

Two rules carry the semantics:

- **A declared table changes that profile and no other**: turning one account
  off leaves every other account exactly as it was, and accounts no file
  mentions stay available. A disabled profile cannot be dispatched to, routed
  to, or listed in that project.
- **A named id merges field by field onto the same id below it**: a layer
  overrides only the fields it writes, so the user file's `label` and the
  project file's `model` both land. A file may also introduce an id no lower
  layer has; `provider` is required then, and `model` falls back to the
  provider's default.

Profile `env` is one field. A project `env` map replaces the user `env` map
for that profile, and every value supports a leading `$HOME` or `~`. For a
Claude profile, set `CLAUDE_CONFIG_DIR` in `env` to choose its account state:

```yaml
profiles:
  claude-work:
    env:
      CLAUDE_CONFIG_DIR: $HOME/.claude-work
```

The same resolved environment runs Claude tasks and reads `claude /usage`.
Without an override, `claude` uses `~/.claude`; every other Claude profile
uses `~/.<profile-id>`. Log into each directory once with the Claude CLI.

To restrict a project to a chosen set of accounts, name them in `only`. Every
id it leaves out is excluded in that scope — nothing else narrows the list:

```yaml
profiles:
  only: [claude-work, opencode]
```

Profiles you manage in the Oga app stay in the app's own store — the file
layer only reads. The narrowing binds where it matters: `delegate`, `handoff`,
and every routing read resolve profiles against the task's `cwd`, so a project
file's `only` list is what a dispatch into that project sees.

### Models

Model settings are defaults at the user level and overrides at the project
level. A project can enable a model disabled globally, or set its own preferred
models:

```yaml
models:
  openai/gpt-5.6-luna:
    enabled: true
    preferred: true
    capabilities: [reasoning, long-context]
```

Unlisted models are enabled but not preferred. If a project declares no
preferred model, `models` falls back to every enabled model. Derived capabilities are
`reasoning`, `long-context`, `tool-use`, and `free`; a model entry's `capabilities`
array replaces those derived tags.

### Worker rules

A project writes the rules its workers read in its own file:

```yaml
worker:
  prompt: |
    1. Blocked means stop …
    5. Open your final report with `## TL;DR` …
```

The block ships verbatim under `## Worker rules` and is read again on every
dispatch. While it is there it replaces whatever Settings → Prompts holds for
that scope, and `prompt` is the only key `worker` takes — see
[docs/worker-rules.md](docs/worker-rules.md).

## Releasing

The Tauri packaging workflow builds unsigned macOS and Linux bundles and the
updater artifacts consumed by the desktop app. Run a local package build from
the Rust workspace with `cargo tauri build --no-sign`; CI uses the matrix in
`rust/packaging/release.yml` for platform builds and broker smoke tests.

---

## Docs

| File | What |
| --- | --- |
| [docs/fields.md](docs/fields.md) | Response shape selector — per-tool defaults, group table, worked examples |
| [docs/follow-along.md](docs/follow-along.md) | Following a task without paying for it — watch / wait / inspect |
| [docs/complete.md](docs/complete.md) | Asserting completion when the worker never attested it |
| [docs/handoff.md](docs/handoff.md) | Moving a dead task across profiles |
| [docs/pi.md](docs/pi.md) | Pi provider reference — dispatch, resume, sandbox, and verification |
| [docs/worktree.md](docs/worktree.md) | Task worktrees — a checkout and branch per task, and committing inside the sandbox |
| [docs/scope.md](docs/scope.md) | Data scope rules — bare dir vs. `/**` vs. `**`, grants, and EPERM gotchas |
| [docs/gotchas.md](docs/gotchas.md) | Failure modes worth knowing before you hit them, and their fixes |

## License

Oga is licensed under the [GNU Affero General Public License v3.0](LICENSE.md).
You may use, modify, and share it freely. If you modify Oga and offer it to
others over a network, you must publish your modified source under the same
license.

Contact: `yong@malico.me`

See [LICENSE.md](LICENSE.md) for the full terms.

## Status

Early and moving fast. The broker, app, and MCP surface work. The event socket
and `oga watch` are new.
Expect rough edges and rapid iteration.
