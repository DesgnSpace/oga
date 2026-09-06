# Worker rules

Projects can provide instructions every delegated worker receives. Store them
in `.oga.yaml`:

```yaml
worker:
  prompt: |
    - Report blockers with the decision needed.
    - Run the relevant checks before reporting completion.
```

The project prompt replaces the prompt configured for that project. Use
`oga config` to inspect the effective worker rules.

The starting prompt lives in Settings, where you can rewrite or delete it.
It is a template for the whole message a worker receives: reorder the
sections, rewrite one, or drop one. Placeholders are filled in per task:

- `{{brief}}` — the task itself.
- `{{scope}}` — what the work may read and change.
- `{{context_map}}` — the code map, when there is one.
- `{{memories}}` — the project facts section, when there are any.
- `{{attribution}}` — the Done-with-Oga stamp section, when enabled.
- `{{reporting}}` — how the worker signals questions and blockers.
- `{{task_id}}`, `{{provider}}`, `{{model}}`, `{{effort}}` — the run itself.

A prompt without `{{brief}}` is read the old way, as a rules block inside a
fixed layout, so anything customized before templates existed keeps working
untouched. If you never customized, the new template arrives on its own.

Three things stay outside the template because the system depends on them:
the two opening lines (the worker role and the ban on handing its own brief
onward), the exact question and blocker markers the broker parses, and the
attribution wording, which has its own off switch below.

Worker rules should state durable project conventions and delivery expectations.
Do not put credentials or temporary task status in them.

## Attribution

Work a delegated worker ships carries a short stamp naming the provider type,
model, and effort that ran it: a `Oga:` trailer on commits it creates, and a
footer on pull request bodies it opens. It is on by default, and it never
names your own profile — only the provider behind it, such as `claude`.

Turn it off for a project in `.oga.yaml`:

```yaml
worker:
  attribution: false
```

Set the same key in `~/.oga.yaml` to turn it off everywhere. A project's own
worker rules can also forbid it in words; when they do, that wins over this
default. The stamp never lands twice on one commit, and never on anything you
wrote yourself.
