#!/bin/bash
# ci.yml campfire-compare's db-differential step, and campfire-db-differential-spinel with --spinel.
set -euo pipefail
redis-server --daemonize yes --save "" --appendonly no >/dev/null
# Not left running: a daemon still holding the test's output keeps the test from ending.
trap 'redis-cli shutdown nosave >/dev/null 2>&1 || true' EXIT
scripts/campfire-oracle prepare --app "$CAMPFIRE_APP"
scripts/campfire-db-differential "$@" "$CAMPFIRE_APP"
