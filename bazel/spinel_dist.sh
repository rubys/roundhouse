#!/bin/bash
# Builds matz/spinel at SHA and stages the toolchain tree ci.yml's build-spinel uploads.
# Usage: spinel_dist.sh SHA OUT_TAR PACK_PY
set -euo pipefail
sha="$1"; out_tar="$PWD/$2"; pack="$PWD/$3"
work="$(mktemp -d)"
curl -fsSL "https://codeload.github.com/matz/spinel/tar.gz/$sha" | tar -xz -C "$work"
src="$(echo "$work"/spinel-*)"
make -C "$src" deps >/dev/null
make -C "$src" -j"$(nproc)" all >/dev/null
dist="$work/dist"; mkdir -p "$dist"
cp -L "$src/spinel" "$dist/spinel"
cp "$src/bin/spin" "$dist/spin"
cp "$src/build/spinel_rbs_extract" "$dist/"
cp -R "$src/lib" "$src/packages" "$src/builtins" "$dist/"
python3 "$pack" "$dist" "$out_tar"
