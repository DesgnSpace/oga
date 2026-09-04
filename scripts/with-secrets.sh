#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ -n "${OGA_OP_WRAPPED:-}" ]]; then
    exec "$@"
fi
export OGA_OP_WRAPPED=1

[[ -f .env.1password ]] || { echo "error: .env.1password is missing"; exit 1; }
command -v op >/dev/null 2>&1 || { echo "error: 1Password CLI (op) not found"; exit 1; }
exec op run --env-file=.env.1password -- "$@"
