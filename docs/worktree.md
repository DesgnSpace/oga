# Separate checkouts and branches

A delegated task can run in its own Git worktree. Oga creates the checkout
under `~/.oga/worktrees/` and gives its branch an `oga/` name.

Use a worktree for changes that should end in a commit or pull request, or when
two tasks could otherwise edit the same files. Review and merge the resulting
branch from the original repository.

## Archive the task and branch

`oga archive <task-id>` archives the task and keeps its branch by default. To
also ask Git to remove the local branch:

```sh
oga archive <task-id> --delete-branch
```

Oga removes the branch only when Git confirms it is safe. Unmerged commits or
another checkout keep it. Uncommitted work keeps both the checkout and branch;
the archive result gives the status in `branchOutcome` and the reason in
`branchReason`.

The Oga app offers the same action from a task's menu and asks for confirmation.

Cleanup can remove an eligible, clean worktree after its task is archived. It
does not remove the branch. Worktrees with uncommitted changes stay in place.
