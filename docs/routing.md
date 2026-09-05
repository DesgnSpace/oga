# Selecting workers and models

Oga chooses an enabled worker and model for delegated work unless the caller
names one. The app's **Settings → Workers** controls which workers and models
can receive work.

`oga love` sets the default for work that names no model:

```sh
oga love claude:opus
oga love codex:gpt-5.6-luna:high --when reasoning
oga love --clear
```

`--when` accepts `context`, `mechanical`, `build`, `reasoning`, or `general`.
Use `--global` to apply a rule outside the current project.

Configuration resolves from the project `.oga.yaml`, then `~/.oga.yaml`, then
built-in defaults. Run `oga config [cwd]` to see the resolved profiles, models,
routes, and the files that supplied them.
