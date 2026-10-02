#!/bin/bash
# Runs a test binary's --ignored toolchain lane, as ci.yml's `cargo test --test X -- --ignored` does.
# Usage: run_ignored.sh [--spinel SPINEL_DIST_TAR] [--writebook WRITEBOOK_TAR] TEST_BINARY [ARGS...]
set -euo pipefail
if [[ "${1:-}" == --spinel ]]; then
  dist="${TEST_TMPDIR:-$(mktemp -d)}/spinel-dist"; mkdir -p "$dist"
  tar -xf "$2" -C "$dist"; chmod +x "$dist/spinel" "$dist/spin" "$dist/spinel_rbs_extract"
  export PATH="$dist:$PATH"; shift 2
fi
if [[ "${1:-}" == --writebook ]]; then
  wb="${TEST_TMPDIR:-$(mktemp -d)}/writebook"; mkdir -p "$wb"
  tar -xzf "$2" -C "$wb" --strip-components=1
  export WRITEBOOK_ROOT="$wb"; shift 2
fi
bin="$1"; shift
exec "$bin" --ignored "$@"
