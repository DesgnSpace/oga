#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    printf 'Usage: %s /path/to/oga-cli\n' "$0" >&2
    exit 2
fi

BINARY="$1"
if [[ ! -x "$BINARY" ]]; then
    printf 'Smoke-test binary is not executable: %s\n' "$BINARY" >&2
    exit 2
fi

command -v curl >/dev/null 2>&1 || {
    printf '%s\n' "curl is required for the package smoke test." >&2
    exit 2
}

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/oga-package-smoke.XXXXXX")"
PORT="${OGA_SMOKE_PORT:-17331}"
DB="$TMP_DIR/oga.db"
LOG="$TMP_DIR/server.log"
SERVER_PID=""

cleanup() {
    if [[ -n "$SERVER_PID" ]]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

VERSION="$(
    OGA_DB="$DB" \
    OGA_PORT="$PORT" \
    OGA_BUILD_STAMP=package-smoke \
    "$BINARY" version
)"

OGA_DB="$DB" \
OGA_PORT="$PORT" \
OGA_BUILD_STAMP=package-smoke \
"$BINARY" serve >"$LOG" 2>&1 &
SERVER_PID=$!

HEALTH=""
for ((attempt = 0; attempt < 30; attempt++)); do
    if HEALTH="$(curl -fsS --max-time 2 "http://127.0.0.1:$PORT/health" 2>/dev/null)"; then
        break
    fi
    sleep 1
done

if [[ -z "$HEALTH" ]]; then
    printf 'Broker did not answer /health. Server log:\n' >&2
    printf '%s\n' "$(<"$LOG")" >&2
    exit 1
fi

if [[ "$HEALTH" != "$VERSION" ]]; then
    printf 'The package identity differs between oga version and /health.\n' >&2
    printf 'oga version: %s\n/health: %s\n' "$VERSION" "$HEALTH" >&2
    exit 1
fi

printf 'oga version: %s\n' "$VERSION"
printf '/health: %s\n' "$HEALTH"
