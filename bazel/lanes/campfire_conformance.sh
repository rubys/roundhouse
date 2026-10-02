#!/bin/bash
# ci.yml campfire-conformance: the strict-emit ceiling, then campfire's own suite against the emit, held to the floor.
set -euo pipefail
gem install sqlite3 bcrypt nokogiri webmock mocha rqrcode ruby-vips sentry-ruby platform_agent concurrent-ruby net-http-persistent web-push rails-html-sanitizer --no-document >/dev/null
ruby tests/campfire_suite_bcrypt.rb
errors=$( { "$ROUNDHOUSE_BINS/roundhouse" --target spinel "$CAMPFIRE_APP" -o /tmp/campfire-spinel 2>&1 || true; } | grep -c 'error\[' || true)
echo "campfire strict emit (spinel, no --allow-unsupported): $errors errors (ceiling 0)"
[ "$errors" -le 0 ]
scripts/campfire-suite --tally /tmp/campfire-tally.txt --fail-log /tmp/campfire-failures.txt --json /tmp/campfire-summary.json "$CAMPFIRE_APP"
tests=$(awk -F'|' '{p += $3} END {print p+0}' /tmp/campfire-tally.txt)
files=$(grep -c '^PASS' /tmp/campfire-tally.txt || true)
echo "campfire conformance: $tests tests, $files files green (floor ${FLOOR_TESTS}/${FLOOR_FILES})"
[ "$tests" -ge "$FLOOR_TESTS" ] && [ "$files" -ge "$FLOOR_FILES" ]
