#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
"$ROOT_DIR/Scripts/package_app.sh"
"$ROOT_DIR/Scripts/make_dmg.sh"
"$ROOT_DIR/Scripts/verify_dmg.sh"
