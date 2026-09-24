# OpenCode 2 workers

The `opencode-2` provider runs OpenCode 2.0 and later. OpenCode 1 keeps its own
`opencode` provider.

## Add a worker

In **Settings → Workers**, add a worker with:

| Field | Value |
| --- | --- |
| Worker ID | `opencode-2` (any id works) |
| Provider | `opencode-2` |
| Default model | a `provider/model` id OpenCode lists, such as `opencode/space-bunny-free` or `openai/gpt-6-luna` |

Then turn on the models it may use. The model list comes from the same
catalogue `opencode2 models` prints, with each model's effort levels.

Sign in to providers with `opencode2 auth login` first. The worker uses that
sign-in and that configuration.

## Which OpenCode it runs

The OpenCode 2 installer adds `opencode2` and also takes over `opencode`, while
an OpenCode 1 install still answers to `opencode`. So a worker picks its
program in this order:

1. The path in the worker's `OPENCODE2_BIN` environment value.
2. `opencode2` on the worker's `PATH`.
3. `opencode` on the worker's `PATH`, only if `opencode --version` reports 2.x.

With none of them, the task fails before it starts and names `opencode2` as
missing. An OpenCode 1 install is never run as OpenCode 2.

The `opencode` provider does the reverse. It runs the path in the worker's
`OPENCODE_BIN`, else the first `opencode` that reports version 1, looking on
the worker's `PATH` and then, when the worker sets no `PATH` of its own, in
`/opt/homebrew/bin`, `/usr/local/bin`, `~/.opencode/bin`, `~/.bun/bin`, and
`~/.local/bin`. If every `opencode` it finds reports another version, the task
fails before it starts, names each one with its version, and the model list
shows the same message on that worker's rows. Each install's version is asked
once and asked again only after the file changes.

```yaml
# Settings → Workers → Environment, when the install is somewhere else
OPENCODE2_BIN: /Users/me/.opencode/bin/opencode2
```

## How tasks run

- Over ACP (`opencode2 acp`) on OpenCode 2.0.x. On another release line the
  task runs on the command line instead.
- The command line is `opencode2 run --standalone --format json --model
  provider/model#effort --auto -- <prompt>`, started in the task's directory.
- Both start a private OpenCode server with the worker's environment rather
  than joining OpenCode's shared background service.
- The effort is the model's variant: the ACP `effort` setting, or the part
  after `#` on the command line.
- The session id is OpenCode's own, so `opencode2 --session <id>` continues a
  task in a terminal, and a follow-up continues the same session.
- An instruction sent while a task runs waits for the current turn to end:
  OpenCode 2 refuses a second prompt mid-turn.
- Tasks show cost from OpenCode. Account limits are not tracked.
