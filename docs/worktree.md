# Task worktrees (`worktree`)

`worktree` on `delegate` gives the task its own checkout of the repository at
`cwd`, on a branch of its own. The worker runs there, commits there, and never
touches your working tree.

Use it when the work should land as commits, and whenever more than one task
runs against the same repository — two workers sharing one checkout share one
index, and they will overwrite each other.

## The input

`worktree: true` takes every default. Every choice lives in one object, so
`worktree: true` and `{}` mean the same thing:

```json
{
  "enabled": true,
  "from": "main",
  "branch": "review/thing",
  "link": ["node_modules", ".env"]
}
```

| Key | | |
| --- | --- | --- |
| `from` | optional | Commit, branch or tag the checkout starts from. Default: the repository's `HEAD`. |
| `branch` | optional | Branch the work lands on. Default: `oga/<slug-of-title>`, or `oga/<taskId>` without a slug. |
| `link` | optional | Untracked paths, relative to `cwd`, the checkout is seeded from. Default: the ignored directories and `.env` files beside `cwd`. |

## What Oga creates

| | |
| --- | --- |
| Checkout | `~/.oga/worktrees/<taskId>`, or beside `OGA_DB` when that is set |
| Branch | `branch`, or `oga/<slug-of-title>`, created at dispatch |
| Base | the commit `from` names, or the one `HEAD` points at when the task is dispatched |

The checkout lives in Oga's data directory, never inside the project: a
checkout under the repo would show up in every scan and every later worktree.
If `cwd` is a subdirectory of the repository, the worker runs in the matching
subdirectory of the checkout.

The repository must have at least one commit. A `cwd` outside a git repository,
or a repository with an unborn `HEAD`, fails the dispatch rather than the run.

## What stays on the original repository

Only the run moves. Scope grants, project memories, `.oga.yaml` rules, the
routing policy and the context map all stay keyed to the `cwd` you delegated
against, so a worktree task inherits the same approvals as any other task on
that repository and its results fold back into the same map.

Scope rules are relative, so they mean the same paths in the checkout that they
mean in the repository.

The context map stays canonical on the original repository, but a worktree
task inherits it without copying the index. The shipped map, map lookup route
and `oga query` use the origin rows, then verify returned candidates against
the checkout, so origin-only edits cannot become checkout facts. A worktree
run's writes are not folded back into the project's map.

## Seeding what the checkout does not have

A checkout holds only what git tracks. `node_modules` is untracked, so it is
not there:

```
bun install v1.3.14
EPERM: Operation not permitted: could not create the "node_modules" directory (mkdir)
```

`link` names the paths the checkout is seeded with:

```json
{ "enabled": true, "link": ["node_modules"] }
```

- Paths are relative to `cwd`. A list you name replaces the default; `[]` seeds
  nothing.
- A path that is not in the original directory is an error naming that path.
  You asked for it by name, so a typo is visible rather than silently skipped.
- A path git tracks is refused: the checkout already has it, and seeding it
  from another tree would put the task's branch out of step with what the
  worker edits.
- A path that leaves `cwd` is refused — no absolute paths, no `..`.
- Two paths where one sits inside the other are refused: the outer one is
  seeded first, so the inner one would be written through it.

Scope records the approved area for the task, but does not enforce filesystem
access.

## Everything in the checkout resolves inside the checkout

A seeded path is the checkout's own directory, not a symlink into yours.

That is the whole rule, and it is not an optimisation. Every language resolves
a file through symlinks before it decides where it is: PHP's `__DIR__`, Node's
module resolution, a virtualenv's `sys.prefix`. Reach `vendor` or
`node_modules` by symlink and the answer to "where am I" is the *original*
repository, so the run loads the original's code while the worker edits the
checkout's. In Laravel that surfaces as routes the checkout plainly defines
coming back `Route [...] not defined.`, facades with no root, and hundreds of
unrelated failures on a container that never became an application — none of
which points at a symlink. The same trap is one badly-behaved library away in
every other ecosystem.

Nothing detects a language or a package manager. Every seeded path is
materialised the same way, so no ecosystem needs its own case and the next one
is right by default.

Files are copied with the filesystem's own copy-on-write clone where it has one
— APFS, Btrfs, XFS — so the bytes are shared until something writes them.
Symlinks *inside* a seeded tree are preserved as symlinks: they are the
package's own structure, and they are relative, so they still land inside the
checkout. Nothing runs a package manager, so a fresh checkout is never dirty
and never needs the network.

### Why not hardlinks

Hardlinking the files would cost nothing on any filesystem, not just the ones
with clones. It is rejected because a shared inode is only safe for tools that
*replace* a file, and not every tool does. `bun install` in a checkout replaces
what it changes and leaves the original alone. `composer dump-autoload` writes
in place, and through a hardlink it rewrites the original repository's
`vendor/composer/autoload_static.php` — same inode, different content, no
warning. One such tool is enough to disqualify the mechanism for everything.

### What it costs

Measured on APFS, seeding the whole checkout:

| | | |
| --- | --- | --- |
| Laravel `vendor` | 87 MB, 10,149 entries | 1.6 s, ~5 MB on disk |
| `node_modules` | 395 MB, 45,286 entries | 7.9 s, ~50 MB on disk |
| Rust `target` | 23 GB, 137,565 entries | 27.7 s, ~57 MB on disk |

Time scales with the number of entries, not bytes. On a filesystem without
clones the bytes are really copied — a 395 MB `node_modules` takes about 20 s
and 414 MB — which is the price of a checkout whose paths mean what they say.

Git sees seeded paths as ordinary directories, so the repository's own ignore
rules apply to them unchanged: `git status` reports the worker's work and
nothing else, and `git add -A` cannot commit your dependencies.

Removing the checkout removes its copies. The originals are untouched, during
the run as well as after it.

## Committing from a worktree

A linked worktree holds only a `.git` pointer file. The objects and the branch
live in the origin repository, so a worktree task commits through that shared
repository:

The origin repository contains the full object store and branch refs. A
worktree task can therefore read repository history, and `worktree: true`
widens what leaves the machine. Scope documents the approval for the working
tree; it does not alter Git's repository layout.

## After the task

The checkout stays. Review the branch and merge it, or throw it away:

```
git -C <repo> log oga/<slug-of-title>
git -C <repo> merge oga/<slug-of-title>
```

`oga cleanup --delete` removes the checkouts of tasks it takes — finished,
archived, and untouched since the retention cutoff. It never deletes a branch,
so the work survives the checkout.

Archiving a task that ran in a worktree removes its checkout at the same time,
and keeps the branch — the archive outcome says `removed`. A checkout holding
uncommitted work is left in place and the outcome names the path, so you can
deal with it and archive again; a checkout that could not be removed is
reported the same way. A task that never ran in a worktree archives exactly as
before. Restoring a task never touches a checkout.

The `worktree-remove` MCP tool deletes a task's checkout on demand, without
waiting for cleanup and without archiving the task. Only a settled task that
ran with `worktree: true` has a checkout to remove; a task that is still
running or waiting on a reply is refused. The branch survives the removal
unless `deleteBranch: true` is passed, which deletes that one branch and
nothing else. Removing a checkout that is already gone reports it as already
gone instead of failing.

`resume`, `reply` and `handoff` all continue in the same checkout. Once it is
gone — cleaned up or removed by the tool — they refuse, naming the branch that
holds the work, rather than starting a second checkout from a `HEAD` that has
since moved.
