set -euo pipefail

PACKAGING_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -- "$PACKAGING_DIR/.." && pwd)"
TARGET="${TAURI_ENV_TARGET_TRIPLE:-}"

if [[ -z "$TARGET" ]]; then
    TARGET="$(rustc -vV | awk '$1 == "host:" { print $2 }')"
fi

if [[ -z "$TARGET" ]]; then
    printf '%s\n' "Could not determine the Rust target triple." >&2
    exit 1
fi

if ! command -v bun >/dev/null 2>&1; then
    printf '%s\n' "bun is required to build the Oga web UI. Install it before packaging." >&2
    exit 1
fi

(
    cd "$ROOT/../web"
    bun install --frozen-lockfile
    bun run build
)

SIDECAR_DIR="$ROOT/apps/oga-desktop/binaries"
SIDECAR="$SIDECAR_DIR/oga-server-$TARGET"
mkdir -p "$SIDECAR_DIR"
if [[ "$TARGET" == "universal-apple-darwin" ]]; then
    # Tauri compiles the app once per architecture and expects a sidecar
    # under each triple; it lipos them into the universal bundle itself.
    for architecture in aarch64-apple-darwin x86_64-apple-darwin; do
        cargo build --manifest-path "$ROOT/Cargo.toml" --package oga-cli --release --locked --target "$architecture"
        install -m 755 "$ROOT/target/$architecture/release/oga-cli" "$SIDECAR_DIR/oga-server-$architecture"
    done
    lipo -create \
        "$ROOT/target/aarch64-apple-darwin/release/oga-cli" \
        "$ROOT/target/x86_64-apple-darwin/release/oga-cli" \
        -output "$SIDECAR"
else
    cargo build --manifest-path "$ROOT/Cargo.toml" --package oga-cli --release --locked --target "$TARGET"
    install -m 755 "$ROOT/target/$TARGET/release/oga-cli" "$SIDECAR"
fi
chmod 755 "$SIDECAR"
printf 'Staged broker sidecar: %s\n' "$SIDECAR"
