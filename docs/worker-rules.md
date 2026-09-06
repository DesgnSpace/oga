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
