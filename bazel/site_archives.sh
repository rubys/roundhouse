#!/bin/bash
# `roundhouse --site` over the fixture with the baked asset graph; copies the
# archives asked for out of _site/browse.
# Usage: site_archives.sh ROUNDHOUSE FIXTURE_TAR ASSETS_TAR OUT_DIR TARGET...
set -euo pipefail
roundhouse="$PWD/$1"; fixture_tar="$PWD/$2"; assets_tar="$PWD/$3"; out_dir="$PWD/$4"; shift 4
work="$(mktemp -d)"
mkdir -p "$work/real-blog" "$work/assets"
tar -xf "$fixture_tar" -C "$work/real-blog"
tar -xf "$assets_tar" -C "$work/assets"
ROUNDHOUSE_ASSETS_DIR="$work/assets" "$roundhouse" --site "$work/real-blog" -o "$work/_site" >/dev/null
for t in "$@"; do cp "$work/_site/browse/$t.tgz" "$out_dir/$t.tgz"; done
