#!/bin/bash
# ci.yml store-check: the analyzer reports nothing on the Rails Guides store.
set -euo pipefail
"$ROUNDHOUSE_BINS/roundhouse" check --continue fixtures/store 2>&1 | tee /tmp/store-check.txt
grep -E "^roundhouse-check: .* 0 error\(s\), 0 warning\(s\), 0 gap-attributed note\(s\), 0 survey gap\(s\)" /tmp/store-check.txt
