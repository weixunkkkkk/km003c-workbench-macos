#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${DIST_DIR:-$ROOT_DIR/dist}"
BUILD_DIR="$DIST_DIR/build"
APP_NAME="KM003C 工作台.app"
APP_DIR="$DIST_DIR/$APP_NAME"
APP_CONTENTS="$APP_DIR/Contents"
APP_BINARY="$APP_CONTENTS/MacOS/KM003CWorkbench"

APP_VERSION="${APP_VERSION:-0.1.0}"
APP_BUILD="${APP_BUILD:-1}"
SIGNING_MODE="${SIGNING_MODE:-adhoc}"
SIGNING_IDENTITY="${SIGNING_IDENTITY:-}"

mkdir -p "$DIST_DIR"
rm -rf -- "$APP_DIR" "$BUILD_DIR"
mkdir -p "$BUILD_DIR" "$APP_CONTENTS/MacOS" "$APP_CONTENTS/Resources"

# Keep compiler/module caches project-local so a release build does not alter
# unrelated workspaces. Cargo still uses the user's normal registry cache.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
export CLANG_MODULE_CACHE_PATH="${CLANG_MODULE_CACHE_PATH:-$ROOT_DIR/.build-cache/clang}"
export SWIFTPM_MODULECACHE_OVERRIDE="${SWIFTPM_MODULECACHE_OVERRIDE:-$ROOT_DIR/.build-cache/swiftpm}"
export XDG_CACHE_HOME="${XDG_CACHE_HOME:-$ROOT_DIR/.build-cache/xdg}"
mkdir -p "$CLANG_MODULE_CACHE_PATH" "$SWIFTPM_MODULECACHE_OVERRIDE" "$XDG_CACHE_HOME"

rust_target_for() {
  case "$1" in
    arm64) echo "aarch64-apple-darwin" ;;
    x86_64) echo "x86_64-apple-darwin" ;;
    *) echo "Unknown architecture: $1" >&2; exit 2 ;;
  esac
}

for arch in arm64 x86_64; do
  target="$(rust_target_for "$arch")"
  rustup target add "$target"
  cargo build --release --locked -p km003c-egui --target "$target"
done

lipo -create \
  "$CARGO_TARGET_DIR/$(rust_target_for arm64)/release/KM003CWorkbench" \
  "$CARGO_TARGET_DIR/$(rust_target_for x86_64)/release/KM003CWorkbench" \
  -output "$APP_BINARY"
chmod 755 "$APP_BINARY"

ICON_MASTER="$ROOT_DIR/assets/app-icon-master.png"
ICONSET_DIR="$BUILD_DIR/AppIcon.iconset"
mkdir -p "$ICONSET_DIR"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$ICON_MASTER" --out "$ICONSET_DIR/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$ICON_MASTER" --out "$ICONSET_DIR/icon_${size}x${size}@2x.png" >/dev/null
done
sips -z 1024 1024 "$ICON_MASTER" --out "$ICONSET_DIR/icon_512x512@2x.png" >/dev/null
if ! iconutil -c icns "$ICONSET_DIR" -o "$APP_CONTENTS/Resources/AppIcon.icns"; then
  # Some macOS releases reject an iconset they can extract themselves. Build
  # the same modern PNG-backed ICNS container deterministically in that case.
  echo "iconutil rejected the generated iconset; using deterministic ICNS builder" >&2
  python3 "$ROOT_DIR/Scripts/build_icns.py" "$ICONSET_DIR" "$APP_CONTENTS/Resources/AppIcon.icns"
fi

cp "$ROOT_DIR/Distribution/Info.plist" "$APP_CONTENTS/Info.plist"
cp "$ROOT_DIR/Distribution/安装说明.md" "$APP_CONTENTS/Resources/安装说明.md"
cp "$ROOT_DIR/Distribution/WITRN-RS-参考迁移.md" "$APP_CONTENTS/Resources/WITRN-RS-参考迁移.md"
cp "$ROOT_DIR/LICENSE-MIT" "$APP_CONTENTS/Resources/LICENSE-MIT"
cp "$ROOT_DIR/LICENSE-APACHE" "$APP_CONTENTS/Resources/LICENSE-APACHE"

/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $APP_VERSION" "$APP_CONTENTS/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $APP_BUILD" "$APP_CONTENTS/Info.plist"
plutil -lint "$APP_CONTENTS/Info.plist"

if [[ "$SIGNING_MODE" == "adhoc" ]]; then
  codesign --force --deep --sign - --timestamp=none "$APP_DIR"
elif [[ -n "$SIGNING_IDENTITY" ]]; then
  codesign --force --deep --options runtime --sign "$SIGNING_IDENTITY" --timestamp "$APP_DIR"
else
  echo "SIGNING_MODE=$SIGNING_MODE requires SIGNING_IDENTITY" >&2
  exit 2
fi

codesign --verify --deep --strict "$APP_DIR"
echo "Built: $APP_DIR"
echo "Binary: $APP_BINARY"
lipo -info "$APP_BINARY"
codesign -dv --verbose=2 "$APP_DIR" 2>&1 | sed -n '1,12p'
