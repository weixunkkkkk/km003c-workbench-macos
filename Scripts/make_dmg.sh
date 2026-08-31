#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${DIST_DIR:-$ROOT_DIR/dist}"
APP_DIR="$DIST_DIR/KM003C 工作台.app"
STAGING_DIR="$DIST_DIR/dmg-staging"
DMG_PATH="$DIST_DIR/KM003C-Workbench-v0.1.0-macOS-universal.dmg"

if [[ ! -d "$APP_DIR" ]]; then
  "$ROOT_DIR/Scripts/package_app.sh"
fi

rm -rf -- "$STAGING_DIR" "$DMG_PATH"
mkdir -p "$STAGING_DIR"
cp -R "$APP_DIR" "$STAGING_DIR/"
ln -s /Applications "$STAGING_DIR/Applications"
cp "$ROOT_DIR/Distribution/安装说明.md" "$STAGING_DIR/安装说明.md"
cp "$ROOT_DIR/Distribution/WITRN-RS-参考迁移.md" "$STAGING_DIR/WITRN-RS-参考迁移.md"
cp "$ROOT_DIR/LICENSE-MIT" "$STAGING_DIR/LICENSE-MIT"
cp "$ROOT_DIR/LICENSE-APACHE" "$STAGING_DIR/LICENSE-APACHE"

hdiutil create \
  -volname "KM003C 工作台" \
  -srcfolder "$STAGING_DIR" \
  -ov \
  -format UDZO \
  -imagekey zlib-level=9 \
  "$DMG_PATH"

(
  cd "$DIST_DIR"
  shasum -a 256 "$(basename "$DMG_PATH")" > "$(basename "$DMG_PATH").sha256"
)
echo "Built: $DMG_PATH"
echo "SHA-256: $DMG_PATH.sha256"
