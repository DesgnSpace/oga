# Removing old task activity

`oga cleanup` previews old work first. It deletes nothing unless you pass both
a retention and `--delete`.

```sh
oga cleanup
oga cleanup --older-than 30d
oga cleanup --older-than 30d --delete
```

Only finished, archived tasks older than the selected retention are eligible.
Running or waiting tasks and project memories are never eligible. Cleanup
removes task activity; the task record remains. It also removes eligible task
worktrees when they have no uncommitted changes. The branch remains in the
repository.

The app offers the same preview in **Settings → Storage**. Review the preview
before deleting: cleanup is permanent.
