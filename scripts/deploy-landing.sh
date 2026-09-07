#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if ! bunx wrangler --version >/dev/null 2>&1; then
    echo "Error: wrangler is not available. Run 'bun add --dev wrangler' first." >&2
    exit 1
fi

cd "$ROOT"
make build-landing
bunx wrangler pages deploy dist/landing --project-name oga
echo "Deployed to https://oga.desgn.space"
