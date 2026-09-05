#!/usr/bin/env bash
set -euo pipefail

# Turns a copy of the release bundle into the development app that sits beside
# a released Oga.app. LaunchServices, the Dock and Spotlight key off the bundle
# identifier and the bundle name, so rewriting those is what makes the copy a
# second app rather than a competing copy of the first. The release bundle
# itself is left alone: patching the copy here keeps `dist/Oga.app`, which
# notarization and publishing take, exactly as it was built.
#
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
