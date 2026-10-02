#!/bin/bash
# `scripts/compare --skip-emit TARGET` over a Bazel-built emit and fixture.
# Usage: compare.sh TARGET FIXTURE_TAR EMIT_TAR COMPARE_BIN [SPINEL_DIST_TAR]
set -euo pipefail
target="$1"; fixture_tar="$PWD/$2"; emit_tar="$PWD/$3"; compare_bin="$PWD/$4"; dist_tar="${5:+$PWD/$5}"
work="${TEST_TMPDIR:-$(mktemp -d)}"; repo="$work/repo"
export HOME="$work/home"; mkdir -p "$HOME" "$repo/tools/compare/target/release" "$repo/fixtures/real-blog"
export PATH="/usr/local/bundle/bin:$PATH"
# Not the runfiles tree: the scripts write build/ and tmp/ under the repository.
cp -RL scripts bin runtime "$repo/"
cp "$compare_bin" "$repo/tools/compare/target/release/roundhouse-compare"
tar -xf "$fixture_tar" -C "$repo/fixtures/real-blog"
mkdir -p "$repo/fixtures/real-blog/tmp/pids" "$repo/fixtures/real-blog/log"
(cd "$repo/fixtures/real-blog" && bundle install --quiet)
case "$target" in
  ruby|jruby|spinel) build_dir="$repo/build/transpiled-blog-$target" ;;
  typescript) build_dir=/tmp/rh-ts-pass2 ;; rust) build_dir=/tmp/rh-rs-pass2 ;; python) build_dir=/tmp/rh-py-pass2 ;;
  go) build_dir=/tmp/rh-go-pass2 ;; crystal) build_dir=/tmp/rh-cr-pass2 ;; kotlin) build_dir=/tmp/rh-kt-pass2 ;;
  swift) build_dir=/tmp/rh-swift-pass2 ;; csharp) build_dir=/tmp/rh-cs-pass2 ;; elixir) build_dir=/tmp/rh-ex-pass2 ;;
  *) echo "compare.sh: unknown target $target" >&2; exit 2 ;;
esac
rm -rf "$build_dir"; mkdir -p "$build_dir"; tar -xf "$emit_tar" -C "$build_dir"
if [[ -n "$dist_tar" ]]; then
  dist="$work/spinel-dist"; mkdir -p "$dist"; tar -xf "$dist_tar" -C "$dist"
  chmod +x "$dist/spinel" "$dist/spin" "$dist/spinel_rbs_extract"; export PATH="$dist:$PATH"
fi
if [[ "$target" == jruby ]]; then
  # Not the image's GEM_HOME: those are CRuby's gems, built extensions and all, and JRuby skips every one.
  mkdir -p "$work/jruby-shim"
  printf '#!/bin/bash\nexport GEM_HOME="%s/jruby-gems"; unset GEM_PATH BUNDLE_PATH\nexec /usr/local/bin/jruby "$@"\n' "$work" > "$work/jruby-shim/jruby"
  chmod +x "$work/jruby-shim/jruby"; export PATH="$work/jruby-shim:$PATH"
fi
cd "$repo"
exec scripts/compare --skip-emit "$target"
