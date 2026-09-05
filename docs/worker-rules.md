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
