#!/usr/bin/env bash
set -euo pipefail

# A distinct bundle identifier keeps this copy separate from a released Oga.app.
# Editing Info.plist breaks the bundle seal, so the copy is signed again.

APP="$1"
NAME="$2"
IDENTITY="$3"
ENTITLEMENTS="$4"

PLIST="$APP/Contents/Info.plist"
[[ -f "$PLIST" ]] || { echo "localize-app: no Info.plist in $APP"; exit 1; }

RELEASE_ID="$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$PLIST")"
case "$RELEASE_ID" in
    *.local) IDENTIFIER="$RELEASE_ID" ;;
    *) IDENTIFIER="$RELEASE_ID.local" ;;
esac

set_string() {
    /usr/libexec/PlistBuddy -c "Set :$1 $2" "$PLIST" 2>/dev/null ||
        /usr/libexec/PlistBuddy -c "Add :$1 string $2" "$PLIST"
}

set_string CFBundleIdentifier "$IDENTIFIER"
set_string CFBundleName "$NAME"
set_string CFBundleDisplayName "$NAME"

codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$IDENTITY" "$APP"

echo "install: $NAME installed as $IDENTIFIER"
