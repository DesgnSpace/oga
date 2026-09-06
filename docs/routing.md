# Selecting workers and models

Oga chooses an enabled worker and model for delegated work unless the caller
names one. The app's **Settings → Workers** controls which workers and models
can receive work.

`oga love` sets the default for work that names no model:

```sh
oga love claude:opus
oga love opencode:muse --when ui
oga love codex:beast --when backend,refactor
oga love codex:gpt-5.6-luna:high --when reasoning
oga love opencode:luna:max claude:opus:low --when ui
oga love --clear
```

A rule holds an ordered list of destinations, each `worker:model:effort`
with the model or the effort left out: `claude` alone means its default
model, `claude:high` means that effort on it. The first destination that can
take the work runs it; when it cannot — rate-limited, out of credits, turned
off, or simply not offered — the next one runs instead, and the task record
says which one that was and why it was not the first. Only when no
destination can take the work does the task fall back to the usual per-task
choice.

`--when` accepts the classes `context`, `mechanical`, `build`, `reasoning`,
`general`, and the subjects `ui`, `backend`, `database`, `docs`, `tests`,
`review`, `research`, `refactor`. `frontend`, `db`, `doc`, and `test` are read
as `ui`, `database`, `docs`, and `tests`. Oga reads the subject off the task
text; `oga delegate --kind <subject>` names it instead, and replaces whatever
the text reads like.

When several rules match one task, the subject rule wins over the class rule,
and the class rule wins over the rule with no `when`. Within one tier the
first rule in the file wins. A subject no rule names falls back to the class,
then to the rule with no `when`. The same kind in two rules is refused when
it is written, so the fleet reads top to bottom. A bad destination is refused
the same way, naming which part is wrong: an unknown worker, a model the
worker does not offer, or an effort outside `minimal` … `max`.

In the config file a chain reads as a `models` list; a single destination
keeps the `model` shape it always had:

```yaml
love:
  - models: [opencode:luna:max, claude:opus:low]
    when: [ui]
  - model: claude:opus
```

Use `--global` to apply a rule outside the current project.

Configuration resolves from the project `.oga.yaml`, then `~/.oga.yaml`, then
built-in defaults. Run `oga config [cwd]` to see the resolved profiles, models,
routes, and the files that supplied them.
