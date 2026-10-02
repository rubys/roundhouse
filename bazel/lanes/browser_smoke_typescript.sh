#!/bin/bash
# ci.yml browser-smoke-typescript: the SharedWorker profile emitted, vite-built and driven.
set -euo pipefail
cd tests/browser_smoke
npm ci --no-audit --no-fund
npx playwright install chromium
npm test
