#!/bin/bash
# ci.yml's build-campfire-compare-spinel: the spinel emit of campfire, compiled to build/blog.
# Usage: campfire_spinel_build.sh ROUNDHOUSE CAMPFIRE_TAR SPINEL_DIST_TAR OUT_TAR PACK_PY
set -euo pipefail
roundhouse="$PWD/$1"; campfire_tar="$PWD/$2"; dist_tar="$PWD/$3"; out_tar="$PWD/$4"; pack="$PWD/$5"
work="$(mktemp -d)"
export HOME="$work/home"; mkdir -p "$HOME" "$work/campfire" "$work/dist"
tar -xzf "$campfire_tar" -C "$work/campfire" --strip-components=1
tar -xf "$dist_tar" -C "$work/dist"
chmod +x "$work/dist/spinel" "$work/dist/spin" "$work/dist/spinel_rbs_extract"
export PATH="$work/dist:/usr/local/bundle/bin:$PATH"
"$roundhouse" --target spinel "$work/campfire" -o "$work/emit" --allow-unsupported >/dev/null 2>&1
make -C "$work/emit" build >"$work/build.log" 2>&1 || { tail -40 "$work/build.log"; exit 1; }
python3 "$pack" "$work/emit" "$out_tar"
