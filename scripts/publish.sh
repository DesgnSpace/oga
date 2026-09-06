#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ -z "${OGA_OP_WRAPPED:-}" ]]; then
    exec scripts/with-secrets.sh "$0" "$@"
fi

VERSION="$(tr -d ' \n' < VERSION)"
TAG="v$VERSION"
TARGET="universal-apple-darwin"
APP_PATH="rust/target/$TARGET/release/bundle/macos/Oga.app"
BUNDLE_DIR="rust/target/$TARGET/release/bundle"
# The repo is private, so releases live on R2 under the prefix install.sh and
# the updater endpoint already point at.
BASE_URL="${R2_PUBLIC_BASE_URL%/}/oga"
: "${APPLE_SIGNING_IDENTITY:?APPLE_SIGNING_IDENTITY required}"
: "${APPLE_ID:?APPLE_ID required}"
: "${APPLE_PASSWORD:?APPLE_PASSWORD required}"
: "${APPLE_TEAM_ID:?APPLE_TEAM_ID required}"
: "${TAURI_PUBLIC_KEY:?TAURI_PUBLIC_KEY required}"
: "${TAURI_SIGNING_PRIVATE_KEY:?TAURI_SIGNING_PRIVATE_KEY required}"
: "${R2_ACCOUNT_ID:?R2_ACCOUNT_ID required}"
: "${R2_ACCESS_KEY_ID:?R2_ACCESS_KEY_ID required}"
: "${R2_SECRET_ACCESS_KEY:?R2_SECRET_ACCESS_KEY required}"
: "${R2_BUCKET:?R2_BUCKET required}"
: "${R2_PUBLIC_BASE_URL:?R2_PUBLIC_BASE_URL required}"
command -v aws >/dev/null 2>&1 || { echo "error: aws CLI not found"; exit 1; }
command -v gh >/dev/null 2>&1 || { echo "error: gh CLI not found"; exit 1; }
[[ -z "$(git status --porcelain --untracked-files=no)" ]] || { echo "error: uncommitted changes"; exit 1; }
! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null || { echo "error: tag $TAG already exists"; exit 1; }
for target in aarch64-apple-darwin x86_64-apple-darwin; do
    rustup target list --installed | grep -qx "$target" || { echo "error: rustup target $target missing"; exit 1; }
done
security find-identity -v -p codesigning | grep -qF "$APPLE_SIGNING_IDENTITY" || { echo "error: signing identity not found"; exit 1; }

echo "Building Oga $VERSION ($TARGET)"
bash rust/packaging/build.sh --target "$TARGET" --bundles app,dmg --signed
[[ -d "$APP_PATH" ]] || { echo "error: app bundle not found at $APP_PATH"; exit 1; }
[[ -f "$APP_PATH/Contents/MacOS/oga-server" ]] || { echo "error: sidecar not found in app bundle"; exit 1; }
# Tauri signs, notarizes, and staples the app itself from the APPLE_* variables;
# only the dmg still needs a notary ticket.
DMG_PATH="$(find "$BUNDLE_DIR/dmg" -maxdepth 1 -name '*.dmg' -print -quit)"
UPDATER_ARCHIVE="$(find "$BUNDLE_DIR/macos" -maxdepth 1 -name '*.app.tar.gz' -print -quit)"
[[ -f "$DMG_PATH" && -f "$UPDATER_ARCHIVE" && -f "$UPDATER_ARCHIVE.sig" ]] || { echo "error: release artifacts missing"; exit 1; }
xcrun notarytool submit "$DMG_PATH" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" --wait
xcrun stapler staple "$DMG_PATH"
codesign --verify --deep --strict "$APP_PATH"
xcrun stapler validate "$APP_PATH"
xcrun stapler validate "$DMG_PATH"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT
# install.sh unpacks a zip of the stapled app; the dmg is for people.
ZIP_PATH="$TMP_DIR/Oga-$VERSION.zip"
ditto -c -k --keepParent "$APP_PATH" "$ZIP_PATH"

DMG_NAME="Oga-$VERSION.dmg"
ZIP_NAME="Oga-$VERSION.zip"
ARCHIVE_NAME="Oga-$VERSION.app.tar.gz"
RELEASED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
SIGNATURE="$(<"$UPDATER_ARCHIVE.sig")"

# The Tauri updater manifest, the install.sh pointer, and the release history.
MANIFEST="$TMP_DIR/latest.json"
LATEST="$TMP_DIR/latest"
RELEASES_JSON="$TMP_DIR/releases.json"
EXISTING_JSON="$TMP_DIR/releases-existing.json"
curl -fsS "$BASE_URL/releases.json" -o "$EXISTING_JSON" || printf '{"latest":"","releases":[]}\n' > "$EXISTING_JSON"
VERSION="$VERSION" SIGNATURE="$SIGNATURE" RELEASED_AT="$RELEASED_AT" CHANGELOG="$ROOT/CHANGELOG.md" \
ARCHIVE_URL="$BASE_URL/$ARCHIVE_NAME" ZIP_URL="$BASE_URL/$ZIP_NAME" DMG_URL="$BASE_URL/$DMG_NAME" \
MANIFEST="$MANIFEST" LATEST="$LATEST" RELEASES_JSON="$RELEASES_JSON" EXISTING_JSON="$EXISTING_JSON" python3 - <<'PY'
import json, os
import re
env = os.environ
platform = {"signature": env["SIGNATURE"], "url": env["ARCHIVE_URL"]}
with open(env["CHANGELOG"]) as f:
    changelog = f.read()
match = re.search(r"^## " + re.escape(env["VERSION"]) + r"(?:\\s+-[^\\n]*)?\\n(.*?)(?=^## |\\Z)", changelog, re.MULTILINE | re.DOTALL)
if not match:
    raise SystemExit(f"release notes missing for version {env['VERSION']}")
notes = match.group(1).strip()
with open(env["MANIFEST"], "w") as f:
    json.dump({"version": env["VERSION"], "notes": notes, "pub_date": env["RELEASED_AT"], "platforms": {"darwin-aarch64": platform, "darwin-x86_64": platform}}, f, indent=2)
    f.write("\n")
with open(env["LATEST"], "w") as f:
    json.dump({"version": env["VERSION"], "url": env["ZIP_URL"], "dmgUrl": env["DMG_URL"], "releasedAt": env["RELEASED_AT"]}, f)
    f.write("\n")
with open(env["EXISTING_JSON"]) as f:
    existing = json.load(f)
release = {"version": env["VERSION"], "date": env["RELEASED_AT"], "zipUrl": env["ZIP_URL"], "dmgUrl": env["DMG_URL"], "updaterUrl": env["ARCHIVE_URL"], "signature": env["SIGNATURE"]}
releases = [release] + [r for r in existing.get("releases", []) if r.get("version") != release["version"]]
with open(env["RELEASES_JSON"], "w") as f:
    json.dump({"latest": release["version"], "releases": releases}, f, indent=2)
    f.write("\n")
PY

export AWS_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID"
export AWS_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY"
export AWS_DEFAULT_REGION=auto
export AWS_EC2_METADATA_DISABLED=true
unset AWS_PROFILE AWS_DEFAULT_PROFILE
ENDPOINT_URL="https://$R2_ACCOUNT_ID.r2.cloudflarestorage.com"
upload() {
    local file="$1" name="$2" content_type="$3" cache_control="$4"
    aws s3 cp "$file" "s3://$R2_BUCKET/oga/$name" \
        --endpoint-url "$ENDPOINT_URL" \
        --content-type "$content_type" \
        --cache-control "$cache_control"
}

echo "Uploading Oga $VERSION to $BASE_URL/"
upload "$DMG_PATH" "$DMG_NAME" application/x-apple-diskimage "public, max-age=31536000, immutable"
upload "$ZIP_PATH" "$ZIP_NAME" application/zip "public, max-age=31536000, immutable"
upload "$UPDATER_ARCHIVE" "$ARCHIVE_NAME" application/gzip "public, max-age=31536000, immutable"
upload "$UPDATER_ARCHIVE.sig" "$ARCHIVE_NAME.sig" text/plain "public, max-age=31536000, immutable"
upload "$DMG_PATH" "Oga-latest.dmg" application/x-apple-diskimage "public, max-age=300"
upload "$ZIP_PATH" "Oga-latest.zip" application/zip "public, max-age=300"
upload "$MANIFEST" "latest.json" application/json "public, max-age=60"
upload "$LATEST" "latest" application/json "public, max-age=300"
upload "$RELEASES_JSON" "releases.json" application/json "public, max-age=60"
upload scripts/install.sh "install.sh" text/x-shellscript "public, max-age=300"

# The Homebrew cask installs the same zip install.sh does; the tap repo gets
# the regenerated file straight from here.
ZIP_SHA256="$(shasum -a 256 "$ZIP_PATH" | awk '{print $1}')"
cat > Casks/oga.rb <<EOF
cask "oga" do
  version "$VERSION"
  sha256 "$ZIP_SHA256"

  url "$BASE_URL/Oga-#{version}.zip"
  name "Oga"
  desc "Local broker for delegating bounded tasks to AI provider CLIs"
  homepage "https://oga.desgn.space"

  livecheck do
    url "$BASE_URL/releases.json"
    strategy :json do |json|
      json["latest"]
    end
  end

  depends_on macos: ">= :sonoma"

  app "Oga.app"
  binary "#{appdir}/Oga.app/Contents/MacOS/oga-server", target: "oga"
end
EOF
TAP_REPO="DesgnSpace/homebrew-tap"
TAP_SHA="$(gh api "repos/$TAP_REPO/contents/Casks/oga.rb" --jq .sha 2>/dev/null || true)"
gh api -X PUT "repos/$TAP_REPO/contents/Casks/oga.rb" \
    -f message="oga $VERSION" \
    -f content="$(base64 < Casks/oga.rb)" \
    ${TAP_SHA:+-f sha="$TAP_SHA"} >/dev/null
git add Casks/oga.rb
git commit -q -m "chore: cask for oga $VERSION" || true

git tag -a "$TAG" -m "Oga $VERSION

$BASE_URL/$DMG_NAME
$BASE_URL/$ZIP_NAME"
git push origin "$(git rev-parse --abbrev-ref HEAD)" "$TAG"
echo "Published Oga $VERSION to $BASE_URL/"
