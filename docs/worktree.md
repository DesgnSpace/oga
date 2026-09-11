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
another checkout keep it. Uncommitted work keeps both the checkout and branch;
the archive result first reports `removing_checkout` while this check runs in
the background. The branch stays until removal finishes; the task history then
records the final result and reason.

The Oga app offers the same action from a task's menu and asks for confirmation.

Cleanup can remove an eligible, clean worktree after its task is archived. It
does not remove the branch. Worktrees with uncommitted changes stay in place.
