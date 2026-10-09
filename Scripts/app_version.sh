# Sourced by the packaging scripts after they set ROOT_DIR.
#
# Distribution/Info.plist is the single source of the application version,
# build number and bundle identity. APP_VERSION / APP_BUILD / ARCHS from the
# environment still take precedence, e.g. `ARCHS=arm64` for a quick local
# Apple Silicon build.

APP_INFO_PLIST="$ROOT_DIR/Distribution/Info.plist"

plist_value() {
  /usr/libexec/PlistBuddy -c "Print :$1" "$APP_INFO_PLIST"
}

APP_VERSION="${APP_VERSION:-$(plist_value CFBundleShortVersionString)}"
APP_BUILD="${APP_BUILD:-$(plist_value CFBundleVersion)}"
APP_BUNDLE_ID="$(plist_value CFBundleIdentifier)"
APP_MIN_MACOS="$(plist_value LSMinimumSystemVersion)"
ARCHS="${ARCHS:-arm64 x86_64}"

case "$ARCHS" in
  "arm64 x86_64" | "x86_64 arm64") APP_ARCH_LABEL="universal" ;;
  arm64 | x86_64) APP_ARCH_LABEL="$ARCHS" ;;
  *) echo "ARCHS must be 'arm64 x86_64', 'arm64' or 'x86_64', got: $ARCHS" >&2; exit 2 ;;
esac

APP_NAME="KM003C 工作台.app"
DMG_NAME="KM003C-Workbench-v${APP_VERSION}-macOS-${APP_ARCH_LABEL}.dmg"
