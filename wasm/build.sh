#!/usr/bin/env bash
# Build the in-browser compiler wasm and drop it where the demos load it
# (lib/roundhouse_wasm.wasm — gitignored; CI's build-wasm job does the same for
# the published site). Rebuilds from source via the in-repo vendored
# ruby-rbs-sys (see vendor/ruby-rbs-sys/README.md), so the only external need is
# the WASI SDK.
#
#   WASI_SDK_PATH=/opt/wasi-sdk wasm/build.sh        # build + copy
#   WASI_SDK_PATH=/opt/wasi-sdk wasm/build.sh --verify   # + run the playground smoke
set -euo pipefail

cd "$(dirname "$0")"
: "${WASI_SDK_PATH:=/opt/wasi-sdk}"
export WASI_SDK_PATH

if [ ! -d "$WASI_SDK_PATH" ]; then
  echo "WASI_SDK_PATH=$WASI_SDK_PATH not found." >&2
  echo "Install the WASI SDK (https://github.com/WebAssembly/wasi-sdk/releases) and set WASI_SDK_PATH." >&2
  exit 1
fi

echo "building roundhouse_wasm (wasm32-wasip1, release)…"
cargo build --release --target wasm32-wasip1
cp target/wasm32-wasip1/release/roundhouse_wasm.wasm lib/roundhouse_wasm.wasm
echo "→ lib/roundhouse_wasm.wasm ($(du -h lib/roundhouse_wasm.wasm | cut -f1))"

if [ "${1:-}" = "--verify" ]; then
  echo "serving wasm/ and running the playground smoke…"
  python3 -m http.server 8099 >/tmp/rh-wasm-build-server.log 2>&1 &
  srv=$!
  trap 'kill $srv 2>/dev/null || true' EXIT
  sleep 1
  (cd playground && node verify-playground.mjs)
fi
