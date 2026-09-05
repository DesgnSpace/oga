# Separate checkouts and branches

A delegated task can run in its own Git worktree. Oga creates the checkout
under `~/.oga/worktrees/` and gives its branch an `oga/` name.

Use a worktree for changes that should end in a commit or pull request, or when
two tasks could otherwise edit the same files. Review and merge the resulting
branch from the original repository.

Cleanup can remove an eligible, clean worktree after its task is archived. It
does not remove the branch. Worktrees with uncommitted changes stay in place.
