#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=Scripts/app_version.sh
source "$ROOT_DIR/Scripts/app_version.sh"
# shellcheck source=Scripts/disk_image.sh
source "$ROOT_DIR/Scripts/disk_image.sh"
DIST_DIR="${DIST_DIR:-$ROOT_DIR/dist}"
DMG_PATH="${1:-$DIST_DIR/$DMG_NAME}"
MOUNT_POINT="$(mktemp -d "${TMPDIR:-/tmp}/km003c-dmg.XXXXXX")"

cleanup() {
  dmg_detach "$MOUNT_POINT"
  rmdir "$MOUNT_POINT" >/dev/null 2>&1 || true
}
trap cleanup EXIT

[[ -f "$DMG_PATH" ]]
hdiutil verify "$DMG_PATH"
EXPECTED="$(shasum -a 256 "$DMG_PATH" | awk '{print $1}')"
if [[ -f "$DMG_PATH.sha256" ]]; then
  printf '%s  %s\n' "$EXPECTED" "$(basename "$DMG_PATH")" | diff -u "$DMG_PATH.sha256" -
fi
dmg_attach_readonly "$DMG_PATH" "$MOUNT_POINT"

APP="$MOUNT_POINT/$APP_NAME"
APP_PLIST="$APP/Contents/Info.plist"
[[ -d "$APP" ]]
[[ -L "$MOUNT_POINT/Applications" ]]
[[ -f "$APP/Contents/Resources/WITRN-RS-参考迁移.md" ]]
[[ -f "$MOUNT_POINT/WITRN-RS-参考迁移.md" ]]
plutil -lint "$APP_PLIST"

# The packaged bundle must describe exactly what Distribution/Info.plist
# declares; a stale DMG from an earlier build fails here.
check_plist() {
  local actual
  actual="$(/usr/libexec/PlistBuddy -c "Print :$1" "$APP_PLIST")"
  if [[ "$actual" != "$2" ]]; then
    echo "$1 is '$actual', expected '$2'" >&2
    exit 1
  fi
}
check_plist CFBundleIdentifier "$APP_BUNDLE_ID"
check_plist CFBundleShortVersionString "$APP_VERSION"
check_plist CFBundleVersion "$APP_BUILD"
check_plist LSMinimumSystemVersion "$APP_MIN_MACOS"

BINARY="$APP/Contents/MacOS/KM003CWorkbench"
lipo -info "$BINARY"
for arch in $ARCHS; do
  lipo "$BINARY" -verify_arch "$arch"
done
codesign --verify --deep --strict "$APP"
file "$BINARY"
echo "DMG verification passed: $DMG_PATH"
