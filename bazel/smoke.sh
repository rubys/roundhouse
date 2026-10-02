#!/bin/bash
# scripts/smoke over a published archive.
# Usage: smoke.sh TARGET TGZ [SPINEL_DIST_TAR]
set -euo pipefail
target="$1"; tgz="$PWD/$2"
export HOME="${TEST_TMPDIR:-$(mktemp -d)}/home"; mkdir -p "$HOME"
export PATH="/usr/local/bundle/bin:$PATH"
if [[ -n "${3:-}" ]]; then
  dist="$HOME/spinel-dist"; mkdir -p "$dist"; tar -xf "$PWD/$3" -C "$dist"
  chmod +x "$dist/spinel" "$dist/spin" "$dist/spinel_rbs_extract"; export PATH="$dist:$PATH"
fi
exec scripts/smoke --tgz "$tgz" "$target"
