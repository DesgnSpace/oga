# Files a task may use

Delegated work receives the paths approved for it. A scope separates paths a
worker may read from paths it may change.

Use `**` for the whole project. A directory rule covers its existing contents.
For files that will be created under a new directory, grant `out/**`, not just
`out`.

Scope is a permission boundary, not an instruction to read every approved file.
Keep write paths narrow where practical. A later task in the same project may
reuse an existing approval; widening a scope requires a new approval.
