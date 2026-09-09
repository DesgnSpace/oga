#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Linux" ]]; then
    printf '%s\n' "Linux capability checks skipped outside Linux."
    exit 0
fi

TARGET="${1:-x86_64-unknown-linux-gnu}"
missing=()
for command_name in cargo rustc rustup pkg-config bwrap trunk; do
    if ! command -v "$command_name" >/dev/null 2>&1; then
        missing+=("$command_name")
    fi
done

if ((${#missing[@]} > 0)); then
    printf 'Missing Linux portability tools: %s\n' "${missing[*]}" >&2
    exit 1
fi

printf 'Linux host: %s\n' "$(uname -srmo)"
printf 'Rust: %s\n' "$(rustc --version)"
printf 'Cargo: %s\n' "$(cargo --version)"
printf 'Trunk: %s\n' "$(trunk --version)"
printf 'Tauri: %s\n' "$(cargo tauri --version)"
printf 'Bubblewrap: %s\n' "$(bwrap --version)"

installed=0
while IFS= read -r installed_target; do
    if [[ "$installed_target" == "$TARGET" ]]; then
        installed=1
        break
    fi
done < <(rustup target list --installed)
if ((installed == 0)); then
    printf 'Rust target is not installed: %s\n' "$TARGET" >&2
    exit 1
fi
printf 'Rust target: %s\n' "$TARGET"

packages=(gtk+-3.0 webkit2gtk-4.1 ayatana-appindicator3-0.1 openssl librsvg-2.0)
for package in "${packages[@]}"; do
    if ! pkg-config --exists "$package"; then
        printf 'Missing pkg-config module: %s\n' "$package" >&2
        exit 1
    fi
    printf 'pkg-config %s: %s\n' "$package" "$(pkg-config --modversion "$package")"
done

if ! bwrap \
    --die-with-parent \
    --new-session \
    --unshare-user \
    --unshare-pid \
    --unshare-uts \
    --unshare-ipc \
    --ro-bind / / \
    --proc /proc \
    --dev /dev \
    --tmpfs /tmp \
    -- /usr/bin/true; then
    printf '%s\n' "Bubblewrap cannot create the required Linux namespaces." >&2
    exit 1
fi
printf '%s\n' "Bubblewrap namespace probe: available"
