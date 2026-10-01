# Separate checkouts and branches

A delegated task can run in its own Git worktree. Oga creates the checkout
under `~/.oga/worktrees/` and gives its branch an `oga/` name.

Use a worktree for changes that should end in a commit or pull request, or when
two tasks could otherwise edit the same files. Review and merge the resulting
branch from the original repository.

Oga records the task before it copies linked files. Its state reads
`preparing_checkout` while the checkout is being prepared, then the worker
starts. Cancelling or archiving it during that step prevents the worker from
starting and cleans the checkout safely.

When no links are named, Oga copies ignored files and directories that help a
task run, but skips `target`, `node_modules`, `dist`, `build`, and `.venv` at
every depth. It also skips Oga's own working files and `.env*` exclusions. Name
any of these paths explicitly in `worktree.link` when the task needs a copy.

## Archive the task and branch

`oga archive <task-id>` archives the task and keeps its branch by default. To
also ask Git to remove the local branch:

```sh
oga archive <task-id> --delete-branch
```

Oga removes the branch only when Git confirms it is safe. Unmerged commits or
another checkout keep it. Uncommitted work keeps both the checkout and branch,
and the archive result and the task history say so. A checkout a live task
still uses is kept the same way, with the reason and the task using it.

The Oga app offers the same action from a task's menu and asks for confirmation.

Oga also collects stale checkouts on its own, at broker start and on the
cleanup cycle, so a checkout that failed to go when its task was archived, or
was left by an earlier run, is cleaned up later. A checkout goes once no live
task uses it, it has no uncommitted work, and at least one task using it is
older than the cleanup retention. The branch stays. A checkout a live task
still uses, or one with uncommitted work, is kept and the task history records
why.
