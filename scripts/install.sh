#!/bin/bash
set -euo pipefail

BASE_URL="https://downloads.desgn.space/oga"
APP_NAME="Oga"

say() {
    printf '%s\n' "$*"
}

if [ "$(uname -s)" != "Darwin" ]; then
    say "Oga requires macOS."
    exit 1
fi

MACOS_VERSION="$(sw_vers -productVersion)"
MACOS_MAJOR="${MACOS_VERSION%%.*}"
if [ "$MACOS_MAJOR" -lt 14 ]; then
    say "Oga requires macOS 14 or later. This Mac runs ${MACOS_VERSION}."
    exit 1
fi

command -v curl >/dev/null || { say "curl is required. Install it, then run this command again."; exit 1; }
command -v ditto >/dev/null || { say "ditto is required. Install the macOS command line tools, then try again."; exit 1; }

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/oga-install.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT INT TERM

say "Finding the latest Oga release..."
LATEST_JSON="$(curl -fsSL "${BASE_URL}/latest")" || {
    say "Could not reach the Oga download server. Check your connection, then try again."
    exit 1
}
VERSION="$(printf '%s' "$LATEST_JSON" | sed -n 's/.*"version":"\([^"]*\)".*/\1/p')"
DOWNLOAD_URL="$(printf '%s' "$LATEST_JSON" | sed -n 's/.*"url":"\([^"]*\)".*/\1/p')"
[ -n "$VERSION" ] || { say "The download server returned an invalid release. Try again later."; exit 1; }
[ -n "$DOWNLOAD_URL" ] || DOWNLOAD_URL="${BASE_URL}/${APP_NAME}-${VERSION}.zip"

say "Downloading Oga ${VERSION}..."
ZIP_PATH="${TMP_DIR}/${APP_NAME}-${VERSION}.zip"
curl -fsSL --retry 3 -o "$ZIP_PATH" "$DOWNLOAD_URL" || {
    say "The download failed. Check your connection, then try again."
    exit 1
}

mkdir -p "${TMP_DIR}/app"
ditto -x -k "$ZIP_PATH" "${TMP_DIR}/app"
[ -d "${TMP_DIR}/app/${APP_NAME}.app" ] || {
    say "The download did not contain Oga.app. Nothing was installed."
    exit 1
}

DEST="/Applications"
if [ ! -w "$DEST" ]; then
    DEST="${HOME}/Applications"
    mkdir -p "$DEST"
fi

pkill -x Oga 2>/dev/null || true
pkill -f 'Contents/Resources/oga-server' 2>/dev/null || true
rm -rf "${DEST}/${APP_NAME}.app"
ditto "${TMP_DIR}/app/${APP_NAME}.app" "${DEST}/${APP_NAME}.app"
xattr -dr com.apple.quarantine "${DEST}/${APP_NAME}.app" 2>/dev/null || true

BINDIR="${HOME}/.local/bin"
mkdir -p "$BINDIR"
ln -sf "${DEST}/${APP_NAME}.app/Contents/MacOS/oga-server" "${BINDIR}/oga"

say "Oga ${VERSION} is installed at ${DEST}/${APP_NAME}.app."
say "Open Oga from Applications to start the broker."
case ":${PATH}:" in
    *":${BINDIR}:"*) ;;
    *) say "Add ${BINDIR} to PATH to use the oga command." ;;
esac
