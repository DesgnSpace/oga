#!/bin/sh
# Warn about tasks in flight before the retiring broker stops them.
# Status 2 means this build cannot read the old database; the install still proceeds.
bin=$1

"$bin" inflight
code=$?
case "$code" in
  0) exit 0 ;;
  1) ;;
  2)
    echo "install: could not check for in-flight tasks — the running broker's database is not readable by this build; continuing"
    exit 0
    ;;
  *)
    echo "install: FAILED: '$bin' exited $code instead of reporting in-flight tasks"
    echo "install: the build that is about to be installed cannot run; nothing was replaced"
    exit 1
    ;;
esac

echo "install: tasks are in flight; they will stop when the broker is replaced"
exit 0
