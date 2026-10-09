# Sourced by the packaging scripts: disk image operations.
#
# macOS 26 and later deprecate `hdiutil create` and `hdiutil attach` in favour
# of `diskutil image`. Older build hosts and CI runners do not have it, so the
# tool is detected once and hdiutil remains the fallback. `hdiutil verify` is
# not deprecated and has no diskutil equivalent; it is used on every host.
# DISK_IMAGE_TOOL=hdiutil forces the fallback, e.g. to test it on a new host.

if [[ -z "${DISK_IMAGE_TOOL:-}" ]]; then
  # Older image subcommands can convert disks but cannot package a folder.
  if disk_image_create_help="$(diskutil image create from --help 2>&1)" && [[ "$disk_image_create_help" == *--volumeName* ]]; then
    DISK_IMAGE_TOOL="diskutil"
  else
    DISK_IMAGE_TOOL="hdiutil"
  fi
fi
case "$DISK_IMAGE_TOOL" in
  diskutil | hdiutil) ;;
  *) echo "DISK_IMAGE_TOOL must be 'diskutil' or 'hdiutil', got: $DISK_IMAGE_TOOL" >&2; exit 2 ;;
esac

# dmg_create <source folder> <destination .dmg> <volume name>
dmg_create() {
  if [[ "$DISK_IMAGE_TOOL" == "diskutil" ]]; then
    diskutil image create from --format UDZO --volumeName "$3" "$1" "$2"
  else
    hdiutil create -volname "$3" -srcfolder "$1" -ov -format UDZO -imagekey zlib-level=9 "$2"
  fi
}

# dmg_attach_readonly <image> <mount point>
dmg_attach_readonly() {
  if [[ "$DISK_IMAGE_TOOL" == "diskutil" ]]; then
    diskutil image attach --readOnly --nobrowse --mountPoint "$2" "$1" >/dev/null
  else
    hdiutil attach -nobrowse -readonly -mountpoint "$2" "$1" >/dev/null
  fi
}

# dmg_detach <mount point>; quiet, never fails (used from cleanup traps).
dmg_detach() {
  if [[ "$DISK_IMAGE_TOOL" == "diskutil" ]]; then
    diskutil eject "$1" >/dev/null 2>&1 || true
  else
    hdiutil detach "$1" -quiet >/dev/null 2>&1 || true
  fi
}
