#!/usr/bin/env bash
set -euo pipefail

PACKAGING_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -- "$PACKAGING_DIR/.." && pwd)"
TARGET=""
BUNDLES=""
APP_ONLY=0
SIGNED=0

usage() {
    cat <<'EOF'
Usage: ./packaging/build.sh [--target <triple>] [--bundles <bundles>] [--app-only] [--signed]

Build the Oga desktop bundles and broker sidecar. Builds are unsigned by
default. Use --signed only in an environment that provides the updater and
platform signing credentials.
EOF
}

while (($# > 0)); do
    case "$1" in
        --target)
            [[ $# -ge 2 ]] || { printf '%s\n' "--target needs a value" >&2; exit 2; }
            TARGET="$2"
            shift 2
            ;;
        --target=*)
            TARGET="${1#*=}"
            shift
            ;;
        --bundles)
            [[ $# -ge 2 ]] || { printf '%s\n' "--bundles needs a value" >&2; exit 2; }
            BUNDLES="$2"
            shift 2
            ;;
        --app-only)
            APP_ONLY=1
            shift
            ;;
        --signed)
            SIGNED=1
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

ARGS=(tauri build)
if [[ -n "$TARGET" ]]; then
    ARGS+=(--target "$TARGET")
fi
if [[ -n "$BUNDLES" ]]; then
    ARGS+=(--bundles "$BUNDLES")
fi
if ((APP_ONLY)); then
    ARGS+=(--bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}')
fi

if ((SIGNED)); then
    : "${TAURI_PUBLIC_KEY:?TAURI_PUBLIC_KEY is required for signed updater artifacts}"
    : "${TAURI_SIGNING_PRIVATE_KEY:?TAURI_SIGNING_PRIVATE_KEY is required for signed updater artifacts}"
    if [[ "$TAURI_PUBLIC_KEY" == *'"'* || "$TAURI_PUBLIC_KEY" == *'\\'* ]]; then
        printf '%s\n' "TAURI_PUBLIC_KEY contains JSON control characters." >&2
        exit 2
    fi
    updater_config="$(printf '{\"plugins\":{\"updater\":{\"pubkey\":\"%s\"}}}' "$TAURI_PUBLIC_KEY")"
    ARGS+=(--config "$updater_config")
else
    ARGS+=(--no-sign)
fi

cd "$ROOT"
cargo "${ARGS[@]}"
