#!/usr/bin/env bash
set -euo pipefail

PACKAGING_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -- "$PACKAGING_DIR/.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BINARY=""
DATABASE="${OGA_DB:-${HOME:?}/.oga/oga.db}"
BACKUP="${OGA_CUTOVER_BACKUP:-}"

usage() {
    cat <<'EOF'
Usage: install-source.sh [--prefix <path>] [--database <path>] [--backup <path>] [--binary <path>]

Builds the Rust CLI from this checkout unless --binary supplies an existing
executable, then installs it as <prefix>/bin/oga.
EOF
}

while (($# > 0)); do
    case "$1" in
        --prefix)
            [[ $# -ge 2 ]] || { printf '%s\n' "--prefix needs a value" >&2; exit 2; }
            PREFIX="$2"
            shift 2
            ;;
        --prefix=*)
            PREFIX="${1#*=}"
            shift
            ;;
        --database)
            [[ $# -ge 2 ]] || { printf '%s\n' "--database needs a value" >&2; exit 2; }
            DATABASE="$2"
            shift 2
            ;;
        --database=*)
            DATABASE="${1#*=}"
            shift
            ;;
        --backup)
            [[ $# -ge 2 ]] || { printf '%s\n' "--backup needs a value" >&2; exit 2; }
            BACKUP="$2"
            shift 2
            ;;
        --backup=*)
            BACKUP="${1#*=}"
            shift
            ;;
        --binary)
            [[ $# -ge 2 ]] || { printf '%s\n' "--binary needs a value" >&2; exit 2; }
            BINARY="$2"
            shift 2
            ;;
        --binary=*)
            BINARY="${1#*=}"
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            printf 'Unknown option: %s\n' "$1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [[ -z "$BACKUP" ]]; then
    BACKUP="${DATABASE}.cutover-backup"
fi

if [[ -z "$BINARY" ]]; then
    stamp="${OGA_BUILD_STAMP:-$(git -C "$ROOT/.." rev-parse --short HEAD 2>/dev/null || printf nogit)-$(date +%Y%m%d%H%M%S)}"
    OGA_BUILD_STAMP="$stamp" cargo build \
        --manifest-path "$ROOT/Cargo.toml" \
        --package oga-cli \
        --release \
        --locked
    BINARY="$ROOT/target/release/oga-cli"
fi

"$PACKAGING_DIR/cutover.sh" install \
    --binary "$BINARY" \
    --target "$PREFIX/bin/oga" \
    --database "$DATABASE" \
    --backup "$BACKUP"
printf 'Installed source build: %s\n' "$PREFIX/bin/oga"
printf '%s\n' "This source install does not receive signed desktop updates."
