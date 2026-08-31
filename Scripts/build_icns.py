#!/usr/bin/env python3
"""Build a modern PNG-backed ICNS container without relying on iconutil.

Some macOS releases can extract an iconset but reject the same valid set when
packing it again. ICNS PNG chunks are a small documented container format, so
this deterministic fallback keeps release packaging independent of that bug.
"""

from __future__ import annotations

import argparse
import struct
from pathlib import Path


def chunk(kind: str, png: bytes) -> bytes:
    payload_length = len(png) + 8
    return kind.encode("ascii") + struct.pack(">I", payload_length) + png


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("iconset", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    entries = [
        ("icp4", "icon_16x16.png"),
        ("icp5", "icon_32x32.png"),
        ("ic11", "icon_16x16@2x.png"),
        ("icp6", "icon_32x32@2x.png"),
        ("ic12", "icon_32x32@2x.png"),
        ("ic07", "icon_128x128.png"),
        ("ic08", "icon_256x256.png"),
        ("ic13", "icon_128x128@2x.png"),
        ("ic09", "icon_512x512.png"),
        ("ic14", "icon_256x256@2x.png"),
        ("ic10", "icon_512x512@2x.png"),
    ]
    body = b"".join(chunk(kind, (args.iconset / filename).read_bytes()) for kind, filename in entries)
    args.output.write_bytes(b"icns" + struct.pack(">I", len(body) + 8) + body)


if __name__ == "__main__":
    main()
