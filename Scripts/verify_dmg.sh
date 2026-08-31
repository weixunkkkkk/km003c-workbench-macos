#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${DIST_DIR:-$ROOT_DIR/dist}"
DMG_PATH="${1:-$DIST_DIR/KM003C-Workbench-v0.1.0-macOS-universal.dmg}"
MOUNT_POINT="$(mktemp -d "${TMPDIR:-/tmp}/km003c-dmg.XXXXXX")"

cleanup() {
  hdiutil detach "$MOUNT_POINT" -quiet >/dev/null 2>&1 || true
  rmdir "$MOUNT_POINT" >/dev/null 2>&1 || true
}
trap cleanup EXIT

[[ -f "$DMG_PATH" ]]
hdiutil verify "$DMG_PATH"
EXPECTED="$(shasum -a 256 "$DMG_PATH" | awk '{print $1}')"
if [[ -f "$DMG_PATH.sha256" ]]; then
  printf '%s  %s\n' "$EXPECTED" "$(basename "$DMG_PATH")" | diff -u "$DMG_PATH.sha256" -
fi
hdiutil attach -nobrowse -readonly -mountpoint "$MOUNT_POINT" "$DMG_PATH" >/dev/null

APP="$MOUNT_POINT/KM003C 工作台.app"
[[ -d "$APP" ]]
[[ -L "$MOUNT_POINT/Applications" ]]
[[ -f "$APP/Contents/Resources/WITRN-RS-参考迁移.md" ]]
[[ -f "$MOUNT_POINT/WITRN-RS-参考迁移.md" ]]
plutil -lint "$APP/Contents/Info.plist"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")" == "com.weixun.km003cworkbench" ]]
[[ "$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$APP/Contents/Info.plist")" == "11.0" ]]
lipo -info "$APP/Contents/MacOS/KM003CWorkbench"
codesign --verify --deep --strict "$APP"
file "$APP/Contents/MacOS/KM003CWorkbench"
echo "DMG verification passed: $DMG_PATH"
