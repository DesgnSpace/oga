#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if ! bunx wrangler --version >/dev/null 2>&1; then
    echo "Error: wrangler is not available. Run 'bun add --dev wrangler' first." >&2
    exit 1
fi

cd "$ROOT"
bunx wrangler pages deploy landing --project-name oga
echo "Deployed to https://oga.desgn.space"
