#!/bin/bash
# ci.yml smoke-campfire: the archive's README run against itself.
set -euo pipefail
(cd e2e/campfire && npm ci --no-audit --no-fund)
scripts/smoke --tgz "$1" campfire
