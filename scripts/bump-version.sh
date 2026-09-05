#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

[[ -z "$(git status --porcelain --untracked-files=no)" ]] || { echo "error: uncommitted changes — commit or stash them before publishing."; exit 1; }
CURRENT="$(tr -d ' \n' < VERSION)"
[[ "$CURRENT" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "error: VERSION must contain x.y.z"; exit 1; }
# The newest v* tag wins over VERSION, so a bump never lands on a taken tag.
LAST_TAG="$(git tag --list 'v*' --sort=-v:refname | sed -n '1s/^v//p')"
if [[ -n "$LAST_TAG" && "$(printf '%s\n%s\n' "$CURRENT" "$LAST_TAG" | sort -V | tail -1)" == "$LAST_TAG" ]]; then
    CURRENT="$LAST_TAG"
fi
IFS=. read -r MAJOR MINOR PATCH <<< "$CURRENT"
BUMP="${1:-}"
if [[ -z "$BUMP" ]]; then
    printf 'Release type [major/minor/fix]: '
    read -r BUMP
fi
case "$BUMP" in
    major) NEW_VERSION="$((MAJOR + 1)).0.0" ;;
    minor) NEW_VERSION="$MAJOR.$((MINOR + 1)).0" ;;
    fix) NEW_VERSION="$MAJOR.$MINOR.$((PATCH + 1))" ;;
    *) echo "Usage: $0 [major|minor|fix]"; exit 1 ;;
esac
printf 'Publish Oga %s from %s? [y/N]: ' "$NEW_VERSION" "$CURRENT"
read -r CONFIRM
[[ "$CONFIRM" =~ ^[Yy]$ ]] || { echo "Aborted."; exit 1; }
printf '%s\n' "$NEW_VERSION" > VERSION
make sync-version
if grep -q '^## Unreleased$' CHANGELOG.md; then
    sed -i '' "s/^## Unreleased$/## Unreleased\\
\\
## $NEW_VERSION - $(date +%Y-%m-%d)/" CHANGELOG.md
    make changelog
fi
git add VERSION CHANGELOG.md landing rust/Cargo.toml rust/Cargo.lock rust/apps/oga-desktop/tauri.conf.json
git commit -m "chore: release v$NEW_VERSION"
printf 'Oga version %s -> %s, committed\n' "$CURRENT" "$NEW_VERSION"
