"""Validate workbench release tags independently of the Rust library version."""

import argparse
import datetime
import plistlib
import re
import sys
from pathlib import Path


def validate_tag(tag: str, version: str) -> None:
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError(f"Invalid application version: {version!r}")
    if not tag or tag == f"v{version}":
        return
    match = re.fullmatch(rf"v{re.escape(version)}-(\d{{8}})", tag)
    if match is None:
        raise ValueError(f"Tag {tag!r} must be v{version} or v{version}-YYYYMMDD")
    datetime.datetime.strptime(match.group(1), "%Y%m%d")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", default="")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    try:
        with (root / "Distribution" / "Info.plist").open("rb") as source:
            version = plistlib.load(source)["CFBundleShortVersionString"]
        validate_tag(args.tag, version)
    except (OSError, ValueError, KeyError, plistlib.InvalidFileException) as error:
        print(f"Release version validation failed: {error}", file=sys.stderr)
        return 1
    print(f"version={version}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
