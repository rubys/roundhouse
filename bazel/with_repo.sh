#!/bin/bash
# Runs a lane's command from a writable copy of the repository, with the
# Bazel-built roundhouse binaries standing in for `cargo run --bin`, and the
# spinel toolchain on PATH when one is given.
# Usage: with_repo.sh [--bin BINARY] [--spinel DIST_TAR] [--campfire SRC_TAR] -- COMMAND...
set -euo pipefail
work="${TEST_TMPDIR:-$(mktemp -d)}"; repo="$work/repo"
export HOME="$work/home"; mkdir -p "$HOME" "$repo" "$work/bins"
while [[ "$1" != -- ]]; do
  case "$1" in
    --bin) cp "$PWD/$2" "$work/bins/"; shift 2 ;;
    --spinel)
      mkdir -p "$work/spinel-dist"; tar -xf "$PWD/$2" -C "$work/spinel-dist"
      chmod +x "$work/spinel-dist/spinel" "$work/spinel-dist/spin" "$work/spinel-dist/spinel_rbs_extract"
      export PATH="$work/spinel-dist:$PATH"; shift 2 ;;
    --campfire)
      mkdir -p "$work/campfire"; tar -xzf "$PWD/$2" -C "$work/campfire" --strip-components=1
      export CAMPFIRE_APP="$work/campfire"; shift 2 ;;
    *) echo "with_repo: unknown option $1" >&2; exit 2 ;;
  esac
done
shift
for b in roundhouse emit_preview; do [[ -f "$b" ]] && cp "$b" "$work/bins/$b"; done
export ROUNDHOUSE_BINS="$work/bins"
# Not the runfiles tree: the lanes write build/, tmp/ and node_modules/ under the repository.
# Dangling links (a runfiles tree's unbuilt outputs) are skipped; any other copy error stops the lane.
if ! cp -RL . "$repo/" 2>"$work/cp.err"; then
  if grep -v 'No such file or directory' "$work/cp.err" >&2; then exit 1; fi
fi
chmod +x "$repo/bazel/shim/cargo"
export PATH="$repo/bazel/shim:/usr/local/bundle/bin:$PATH"
cd "$repo"
exec bash -c "$*"
