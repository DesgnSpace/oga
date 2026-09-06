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
oga love --clear
```

`--when` accepts the classes `context`, `mechanical`, `build`, `reasoning`,
`general`, and the subjects `ui`, `backend`, `database`, `docs`, `tests`,
`review`, `research`, `refactor`. `frontend`, `db`, `doc`, and `test` are read
as `ui`, `database`, `docs`, and `tests`. Oga reads the subject off the task
text; `oga delegate --kind <subject>` names it instead, and replaces whatever
the text reads like.

When several rules match one task, the subject rule wins over the class rule,
and the class rule wins over the rule with no `when`. Within one tier the
first rule in the file wins. A task whose subject no rule names falls back to
its class, then to the rule with no `when`, and a task with no subject at all
starts at its class. Naming the same kind in two rules is refused where it is
written, so no task is ever claimed twice. `oga love` and **Settings** list the
rules in that same order, not the file's.

Use `--global` to apply a rule outside the current project.

Configuration resolves from the project `.oga.yaml`, then `~/.oga.yaml`, then
built-in defaults. Run `oga config [cwd]` to see the resolved profiles, models,
routes, and the files that supplied them.
