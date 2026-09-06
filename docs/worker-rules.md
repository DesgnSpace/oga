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
It is plain text you own, top to bottom: reorder the sections, rewrite one,
or drop one, and nothing is added behind your back. Delete everything and
write one sentence, and one sentence is sent. Placeholders are filled in per
task:

- `{{brief}}` — the task itself.
- `{{scope}}` — what the work may read and change.
- `{{context_map}}` — the code map, or nothing when there is none.
- `{{memories}}` — the project facts section, or nothing when there are none.
- `{{attribution}}` — the supervision stamp section, or nothing when off.
- `{{reporting}}` — how the worker signals questions and blockers.
- `{{task_id}}`, `{{provider}}`, `{{model}}`, `{{effort}}` — the run itself.

Anything else in `{{braces}}` is left exactly as written. The section
placeholders expand to a whole section or nothing, since there is no
conditional syntax to skip a heading with.

One thing is structurally required, because without it there is no
delegation to perform: the task slot. A prompt without `{{brief}}` gets the
slot first, with your words and order untouched — that is how prompts
customized before templates existed keep working. Everything else, including
whether the worker may hand work onward and which markers it uses, lives in
your text and yours alone.

Worker rules should state durable project conventions and delivery expectations.
Do not put credentials or temporary task status in them.

## Attribution

Work a delegated worker ships carries a short stamp naming the provider type,
model, and effort that ran it: a `Supervised-by:` trailer on commits it
creates, and a footer on pull request bodies it opens. It reads as
supervision, not authorship — Oga supervised the task, here is what ran it.
It is on by default, and it never names your own profile — only the provider
behind it, such as `claude`.

Turn it off for a project in `.oga.yaml`:

```yaml
worker:
  attribution: false
```

Set the same key in `~/.oga.yaml` to turn it off everywhere. A project's own
worker rules can also forbid it in words; when they do, that wins over this
default. The stamp never lands twice on one commit, and never on anything you
wrote yourself.
