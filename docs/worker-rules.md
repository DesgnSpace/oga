# Worker rules

Every task Oga dispatches ships one document, assembled in a fixed section
order: the worker-mode preamble, the query-first navigation line, the caller's
brief, the scope line, the worker rules, the optional project map, project
memories, and the reporting protocol. A section with nothing in it is left
out entirely — no empty heading. The navigation line always ships; the
project map follows later only when the project enables it.
Most of that document is machinery Oga reads back out of the worker's final
message. The worker rules section is not: it is one free-text field a project
can set for itself.

## The document

```
Worker mode: you are executing an assigned Oga task.
...

When you do not know which file or symbol to open, run `oga query "<what you are looking for>"` first — it answers a description, not just a name. When the file is already known, or the task needs every match rather than the right one, search directly with `rg`. Read the source before acting.

<the caller's brief>

Scope for this task, which also shapes its context map — read: …; write: …. …

## Worker rules
<whatever the project wrote — see below>

## Memories
<the cwd's saved memories, when there are any>

## Reporting
<the worker rules' own report-shape wording, if any, then the fixed status lines>
If work cannot be completed, end with: OGA_BLOCKED: <permission_denied|needs_authority|worker_error> | <short reason>
```

That navigation line ships before every brief. Set `.oga.yaml`'s `map`
ship = true` to also include a rendered `## Project map` section — a heading,
a header line, and one map entry per line
(`path#symbol # note`), same as before this default changed.

To see the real thing for a task that already ran, read its shipped prompt:
`inspect` with `fields: ["shippedPrompt"]`.

## Finding code without the map

`oga query "<question>"` answers a plain-language question with one
`path#symbol` when it is confident, a few candidates when it is not, or an
honest miss telling the worker to search instead — it never guesses. It reads
When run inside a task, `OGA_TASK_ID` keeps the answer scoped to that task's
readable files. From a normal shell, it answers for the current project; run
`oga query --init` first when that project has no index. An MCP-capable
worker reaches the same lookup as the `query` tool instead of shelling out.
Neither is a
substitute for reading the file the answer names.

## Where the worker rules text lives

A scope's rules come from its own `.oga.yaml` when it has one — see
"`.oga.yaml worker`" below — and otherwise from Settings → Prompts, which
holds one field per scope: one for all projects (the home directory) and one
per project that overrides it. Either way the text ships verbatim under
`## Worker rules`, with nothing merged in behind it: no built-in text
prepended, no template substitution. What you see is exactly what a worker
reads.

A project with no override of its own inherits the all-projects scope. Its
default value — before any project has written anything — is the wording
Oga shipped hardcoded before this field existed:

```
1. Blocked means stop. A command that will not run, a missing credential, an account or signup, a permission denial, a path outside your scope, a decision this brief does not answer — stop and report it, naming the blocker and the one decision you need.
2. Do not work around a blocker. No retry loops, no second tool for the same job, no creating accounts, no linking or authenticating anything, no faking or stubbing the result. Stop, finish what does not depend on it, then report the exact command or path and say whether you need the caller to decide or to run it and return the output.
3. Partial work is a valid result. Finish what is unblocked, then report what you stopped on.
4. Never report a result you did not observe. If you could not run a check, say so and say why, instead of describing an outcome you did not see.
5. Open your final report with `## TL;DR` — 1-3 plain-language sentences stating what was done or found and the outcome. Detail follows after; this applies to your final answer, not to intermediate messages.
6. Write that TL;DR as bullets — one idea per line, never a paragraph — and make it stand alone: no bullet may need the detail below it to make sense.
7. Keep the TL;DR to roughly ten lines or fewer: the verdict; what the work did or decided, one meaningful line per decision; checks run and their results, quoting failures exactly; and what is left, broken, or uncertain, or "nothing".
8. Describe meaning, not a file list. Name a file only when the file itself is the point, such as a moved file, deleted feature, or new entry point. Keep the branch line for worktree tasks.
```

Edit it like any other text: rewrite it, trim it, add your own rules on the
end. An empty field is valid — it drops the `## Worker rules` section
entirely, and the worker gets no rules beyond the fixed reporting protocol.

Saving is explicit, per scope, with a conflict check: if the file backing
that scope changed since it was loaded, the save is refused rather than
overwriting the newer value.

## `.oga.yaml worker` — project rules

A project writes its rules in its own file, under one key:

```yaml
version: 1
worker:
  prompt: |
    1. Blocked means stop …
    5. Open your final report with `## TL;DR` …
```

The block ships verbatim under `## Worker rules`, and it is read again on
every dispatch — edit the file and the next task sees the change, with
nothing to import and nothing to re-save.

While `worker.prompt` is there it is the scope's rules: it wins over anything
Settings saved for the same scope, and Settings → Prompts shows it read-only,
naming the file to edit. Delete the key and the saved field takes over again,
then the all-projects default behind it.

`prompt` is the whole table. Every other key — including `tldr`,
`tldr_sentences`, `tldr_template`, `builtins`, `conduct` and `report`, which
this key replaces — fails the read naming the key and the shape above.

`.oga.yaml`'s `map` table is unaffected: `ship`, `ship_chars` and
`lookup` still apply live, on every dispatch. `ship` now defaults to `false`
— see "Finding code without the map" above for what ships instead.

## A resume that reuses its session ships the instruction alone

A worker-mode preamble, worker rules, the project map slot, memories and the
reporting protocol are all things a provider session already holds once it
has been dispatched to once — shipping them again on every follow-up just
re-explains the project to a session that already knows it.

- **A resume or reply that reopens the same provider session** ships only the
  follow-up instruction (or the resolved answer, for a reply) and a one-line
  statement of what changed. If the resume passed a new `scope`, that is
  stated in one line; otherwise nothing about scope ships at all. No
  preamble, no worker rules, no project map slot, no memories, no reporting
  protocol — the session already has them.
- **A reseeded session** — the provider refused to reopen the old one, and a
  fresh session starts under the same task id — ships the complete document,
  exactly as a first dispatch does. That session has never seen any of it.

`recordShippedPrompt` still records the real text either way, so `inspect`
with `fields: ["shippedPrompt"]` always answers "what was actually sent" for
that run.

## What a project cannot set

The rest of the document is how Oga and the worker understand each other,
and none of it is configurable:

- **The two status lines and the question marker.** Oga reads `OGA_RESULT`,
  `OGA_BLOCKED` and `OGA_NEEDS_INPUT` back out of the worker's final
  message. A clean exit with real output completes without a marker.
- **Whether questions are allowed.** Dispatching with `allowQuestions: false`
  replaces the question marker with an instruction to report blocked instead.
  That is the caller's decision per task, not the project's.
- **Where a worker stops.** The preamble draws the line: an obstacle that is
  local, reversible, inside scope and does not change the deliverable — a
  stray generated file in the way of a checkout, a stale lockfile, a missing
  directory — is the worker's to clear and report. Only an obstacle needing
  the caller — a credential, a scope or product decision, an irreversible or
  out-of-scope action — is a blocker.
- **The scope line.** It records the caller-approved area and shapes the context
  map. It is not an OS boundary or a reading list.
- **Section order.** The worker rules section always sits between the scope
  line and the project map slot; the reporting protocol is always last. A
  rule that says "never emit OGA_RESULT" is a rule the worker may follow,
  but it cannot delete the instruction that follows it.

Rules govern what the worker does and what it writes back. The envelope
around them is Oga's.

## When the config is wrong

A `worker` table Oga cannot read fails the `delegate` call, and the error
names the file and the exact key:

```
invalid config /Users/you/project/.oga.yaml at worker.tldr_sentences: unknown key; worker takes one key, prompt, holding the rules text
```

This is the same convention `routes` already follows. Nothing falls back to
defaults quietly — a rule you wrote and Oga ignored is worse than a dispatch
that stops and tells you why. The table is read on every dispatch, so a
broken one keeps failing until it is fixed.

A missing `.oga.yaml`, or one with no `worker` map, is not an error.
That is the normal case and it uses the defaults.
