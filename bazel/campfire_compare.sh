#!/bin/bash
# scripts/campfire-compare's ruby lane, as ci.yml's campfire-compare job runs it:
# Redis up, the Rails oracle prepared, the Bazel-built emit served beside it.
# Usage: campfire_compare.sh CAMPFIRE_TAR EMIT_TAR [campfire-compare options, e.g. --spinel]
set -euo pipefail
campfire_tar="$PWD/$1"; emit_tar="$PWD/$2"; shift 2
work="${TEST_TMPDIR:-$(mktemp -d)}"
export HOME="$work/home" PATH="/usr/local/bundle/bin:/usr/local/cargo/bin:$PATH"
mkdir -p "$HOME" "$work/repo" "$work/campfire" "$work/emit"
# Not the runfiles tree: the scripts write the oracle under the repository's build/.
cp -RL scripts runtime "$work/repo/"
tar -xzf "$campfire_tar" -C "$work/campfire" --strip-components=1
tar -xf "$emit_tar" -C "$work/emit"
redis-server --daemonize yes --save "" --appendonly no >/dev/null
# Not left running: a daemon still holding the test's output keeps the test from ending.
trap 'redis-cli shutdown nosave >/dev/null 2>&1 || true' EXIT
cd "$work/repo"
scripts/campfire-oracle prepare --app "$work/campfire"
scripts/campfire-compare "$@" --reuse "$work/emit" "$work/campfire"
