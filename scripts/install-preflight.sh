#!/bin/sh
# Warn about tasks in flight before the retiring broker stops them. The new
# build's `inflight` answers:
#   0  nothing in flight — continue silently
#   1  tasks in flight — warn and continue
#   2  could not check — the running broker's database is not readable by
#      this build, which is the normal case on an upgrade (the new binary
#      refuses an older schema); warn and continue, the install is about to
#      start the new broker anyway.
# Anything else means the binary did not run at all — a signal exit says macOS
# killed it — so the install stops before it replaces a working one.
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
