#!/bin/bash
# ci.yml writebook-inventory's "Save complete check report": one summary line, and an exit status that agrees with it.
set -uo pipefail
bin="$PWD/$1"; wb="${TEST_TMPDIR:-$(mktemp -d)}/writebook"; mkdir -p "$wb"
tar -xzf "$2" -C "$wb" --strip-components=1
"$bin" check --continue "$wb" > "$wb.txt" 2>&1
status=$?
set -e
cat "$wb.txt"
test "$(grep -c '^roundhouse-check: .* — ' "$wb.txt")" -eq 1
summary=$(grep '^roundhouse-check: .* — ' "$wb.txt")
pattern='^roundhouse-check: .* — ([0-9]+) parse error\(s\), ([0-9]+) error\(s\), [0-9]+ warning\(s\), [0-9]+ gap-attributed note\(s\), [0-9]+ survey gap\(s\)$'
[[ "$summary" =~ $pattern ]]
expected=0
if (( BASH_REMATCH[1] + BASH_REMATCH[2] > 0 )); then expected=1; fi
test "$status" -eq "$expected"
