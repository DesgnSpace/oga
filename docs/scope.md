# Data scope (`scope`)

`scope` records the area a caller approved before a task's prompt and files go
to an external provider account. It shapes what `oga query` and the
optional rendered map can answer, and documents the approval; it is not a
filesystem boundary. It has two lists, `read` and `write`, each made of rules
resolved relative to the task's `cwd`.

## Writing a rule

| Rule | Describes |
| --- | --- |
| `pwa` (a path that exists and is a directory) | The directory itself and everything under it |
| `pwa` (a path that does not exist yet) | That exact path only — it stays literal |
| `pwa/**` | The directory and everything under it, explicitly |
| `rust/crates/oga-runner` | That one crate |
| `**` | The whole working tree, including hidden files and `.git` |

An existing directory named without `/**` still covers its subtree — Oga
expands it before recording the scope, because callers naming a directory
almost always mean everything in it.

A path that does not exist yet cannot be checked against the filesystem, so it
keeps literal semantics — it grants exactly that path and nothing created
under it. Name planned output directories with an explicit `/**` suffix:
`out/**`, not `out`.

Read rules also cover write in the recorded scope description; write rules do
not cover read on their own.

## Permission is not a reading list

`scope` records what the caller approved before a prompt and files go to an
external provider account. It never says what to open, and it belongs in the
`scope` argument only — never repeated as prose in the prompt.

| Concern | Where it lives |
| --- | --- |
| What may be opened | The `scope` argument — recorded approval metadata |
| What the work must achieve | The prompt — goal, context, behavior, guardrails, output |
| How to navigate the project | `oga query`, or the rendered map with `[map] ship = true` |
| Which files to actually open | The worker's decision, inside the grant |

So `read: ["**"]` is the recommended default and lets `oga query` and the
map answer from anything under the working directory. It is not an instruction to read the
repository, and it is not a substitute for one either.

A prompt has **no Scope section**. Writing one costs twice: the approval is
already recorded in the argument, so the prose is dead weight, and a guessed
file list replaces the worker's own discovery with the caller's guess. That
discovery is usually the reason the work was delegated — the caller does not
know which files the fix touches, and the worker is the one positioned to find
out.

```
Wrong, in the prompt:            Right:
## Scope                         (nothing — pass scope: {read: ["**"],
Read permission: `**`.            write: ["src/**"]} as the argument)
Begin with the project map.
Inspect callers and tests.
```

Oga ships the scope line and a pointer to `oga query` on its own, so no
caller has to write them. The scope line explains that the area is recorded
approval, not a reading list; the pointer tells the worker how to ask where
code lives, at its own discretion.

## Grants persist per cwd and profile

Stating `scope` on `delegate` records it as that cwd and profile's grant.
Omitting it reuses the newest grant recorded for the cwd — the caller does not
have to restate scope on every dispatch once it has been approved once.

Only a cwd with **no grant at all** falls back to the whole working tree, and
that task is flagged (`scope_ungranted`) so the fallback is never silent.

Reusing a grant approved for a different profile still runs — approval names a
destination account, not just a folder — but the task carries a
`scope_inherited` warning naming the profile the grant was actually approved
for. State `scope` explicitly to approve the new destination and clear it.

`reply`, `resume`, and `handoff` can each replace a task's scope after fresh
approval; the replacement becomes the cwd's new grant.

## Gotchas

### A narrow read scope leaves the map and `oga query` incomplete

Both only answer from the recorded read area. Existing paths named in the
prompt are added to that area automatically for the rendered map, but paths
discovered mid-run are not, and `oga query` never sees outside the read
area at all. If the task needs broad project context, record the directory it
will explore in, not just the files the prompt names.

### Nonexistent output paths need an explicit `/**`

A path that does not exist when the task is scoped stays literal even if the
worker is expected to create a directory tree under it. If the task will write
`out/report.json` and `out/` does not exist yet, scope `out/**`, not `out`.
