# OpenCode 2 provider

`opencode-2` drives [opencode v2](https://opencode.ai/v2/docs), installed and
run as the **`opencode2`** binary (`@opencode-ai/cli@beta`). It is a separate
provider from `opencode`, which keeps driving v1's `opencode` binary; both can
be installed side by side, and switching an account between them is a profile
config change.

Every flag and payload below comes from the v2 docs or the real binary
(`opencode2 v0.0.0-next-15788`) — see [Verified](#verified).

## Dispatch

```
opencode2 run --format json --model <provider/model[#variant]> --auto <prompt>
```

| Flag | Why |
| --- | --- |
| `--format json` | One session event per JSON line on stdout. |
| `--model` | v2 spells models `providerID/modelID`, with the reasoning variant folded onto the id as `#variant` (`opencode/deepseek-v4-flash-free#high`). There is no separate variant flag like v1's `--variant`. Only variants the model publishes are sent — the catalog supplies each model's ladder. |
| `--auto` | Auto-approves permissions that are not explicitly denied, so a headless run cannot stall on approval. |
| `<prompt>` | Trailing positional. |

There is **no cwd flag** on `run`. The spawn's working directory scopes the run,
like pi. Oga sets `PWD` to the task cwd for every worker, so the task
directory is the only directory the worker learns about.

## Resume

```
opencode2 run --format json --model <model> --auto --session <id> <prompt>
```

The id is the stream's top-level `sessionID` (`ses_…`), present on every line;
Oga captures it from the opening `step_start`. A resumed run reports the same
id back, so Oga confirms the session was actually reused rather than forked.
`--continue/-c` continues the *last* session and is not addressable by id;
`--fork` copies instead of continuing.

## Models

v2 has no `models` subcommand. The catalog is a server route:

```
opencode2 api GET /api/model --param location[directory]=<dir>
```

which answers `{location, data: [Model.Info]}`. A row's addressable id is
`providerID/modelID`; `variants[].id` is the effort ladder; `cost[0]`,
`limit.context` and `capabilities.tools` carry pricing, context window, and
tool calling. Rows with `enabled: false` are skipped.

The route runs against opencode 2's shared background service. If no service is
running, this call starts one — a side effect of how v2 works, not something
Oga does deliberately. A cold start slower than Oga's discovery timeout
falls back to the profile's configured default model.

## Sessions and effort readback

v2 writes its sessions into `~/.local/share/opencode/opencode-next.db`, next to
v1's `opencode.db`, with the same shape: the session row's `model` column holds
JSON whose `variant` field is the reasoning level the run actually used. The
same readback answers both providers.

## Failure detection

A failed generation exits nonzero and prints one line:

```json
{"type":"error","timestamp":…,"sessionID":"","error":{"type":"…","message":"Model unavailable: …"}}
```

so exit codes stay meaningful, unlike pi. An aborted-but-exited-0 turn closes on
`step_finish` with an abort reason and zero output tokens — `abortedTurn` reads
that shape unchanged from v1. Normal closes use `reason: "stop"` or
`"tool-calls"`.

## What v2 does not have

- **Live steering.** `run` reads no framed stdin and exits at the end of its
  turn, exactly like v1's `run`. Steering would take an owned `opencode2`
  server plus its prompt-delivery route (`POST /api/session/{id}/prompt`); the
  refusal copy names this.
- **Per-task MCP injection.** There is no inline `--mcp-config` equivalent on
  the CLI (`mcp add` edits config files). Same posture as v1: workers get no
  injected MCP server.
- **Usage tracking.** No quota surface is documented or exposed; reported as
  unsupported, same as v1.

## Routing policy

Rules in `.oga.yaml` name the provider as `"opencode-2"`. Until a rule names
it, automatic routing treats its models as outside the allow list (and may drop
that constraint when nothing else can run), while a caller naming the profile or
model directly always gets through with a warning.

## Verified

Against **opencode2 v0.0.0-next-15788**, model `opencode/x-preview-f-free`:

- **Flags.** Every flag above exists in `opencode2 run --help`; there is no
  `--dir`, `--variant`, or `--verbose`.
- **Event stream.** A real run emits `step_start`, `text`, `tool_use`,
  `step_finish` JSONL with top-level `sessionID` and payloads under `part` —
  all shapes as parsed. Tool calls arrive once, settled:
  `part.type: "tool"` with `callID`, `tool`, and `state.status/input`.
- **Variants.** `--model 'opencode/x-preview-f-free#low'` ran clean; the
  session DB recorded `variant: "low"`.
- **Resume works.** `--session` with the captured id reported the same session
  id and recalled the previous turn's answer.
- **Tools.** A real write produced `probe-write.txt` via a `tool_use` line
  carrying `state.input.path`.
- **Failure.** A nonexistent model exited 1 with the single `type:"error"`
  line above.
- **Catalog.** `/api/model` returned 29 parseable rows from an arbitrary
  directory.

## Version sensitivity

The beta self-describes as changing: APIs, configuration, and plugin surfaces
may break between releases. Re-run the probes above after any `opencode2`
upgrade.
