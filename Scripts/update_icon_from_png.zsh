#!/bin/zsh
set -euo pipefail

root_dir="$(cd -- "$(dirname -- "$0")/.." && pwd)"
source_png="${1:?usage: update_icon_from_png.zsh /path/to/icon.png}"
master_png="$root_dir/assets/app-icon-master.png"
output_icns="$root_dir/assets/AppIcon.icns"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/km003c-icon.XXXXXX")"
iconset_dir="$work_dir/AppIcon.iconset"

cleanup() {
  rm -rf -- "$work_dir"
}
trap cleanup EXIT

if [[ ! -f "$source_png" ]]; then
  print -u2 "icon source does not exist: $source_png"
  exit 2
fi

mkdir -p "$iconset_dir"
sips -s format png "$source_png" --out "$master_png" >/dev/null
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$master_png" --out "$iconset_dir/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$master_png" --out "$iconset_dir/icon_${size}x${size}@2x.png" >/dev/null
done
if ! iconutil -c icns "$iconset_dir" -o "$output_icns"; then
  print -u2 "iconutil rejected the iconset; using the deterministic PNG-backed ICNS builder"
  python3 "$root_dir/Scripts/build_icns.py" "$iconset_dir" "$output_icns"
fi

print "Updated icon master: $master_png"
print "Updated AppIcon: $output_icns"
