#!/bin/bash
# The static asset graph the published archives ship, as ci.yml's build-site
# job bakes it: tailwind.css over the fixture's views, and the turbo /
# stimulus bundles from the gems the spinel scaffold pins.
# Usage: asset_graph.sh FIXTURE_TAR E2E_DIR SCAFFOLD_DIR OUT_TAR PACK_PY
set -euo pipefail
fixture_tar="$PWD/$1"; e2e="$PWD/$2"; scaffold="$PWD/$3"; out_tar="$PWD/$4"; pack="$PWD/$5"
work="$(mktemp -d)"
export HOME="$work/home"
# Not on Bazel's PATH: the image's gem executables.
export PATH="/usr/local/bundle/bin:$PATH"
mkdir -p "$work/real-blog" "$work/out/controllers" "$work/e2e" "$work/scaffold"
tar -xf "$fixture_tar" -C "$work/real-blog"
cp "$e2e/package.json" "$e2e/package-lock.json" "$work/e2e/"
(cd "$work/e2e" && npm ci --no-audit --no-fund >/dev/null)
printf '@import "tailwindcss";\n@source "%s/real-blog/app";\n' "$work" > "$work/e2e/.tailwind-input.css"
(cd "$work/e2e" && npx @tailwindcss/cli --input .tailwind-input.css --output "$work/out/tailwind.css" --minify >/dev/null 2>&1)
cp "$scaffold/Gemfile" "$scaffold/Gemfile.lock" "$work/scaffold/"
(cd "$work/scaffold" && bundle install --quiet)
gem_dir() { (cd "$work/scaffold" && bundle exec ruby -e "puts Gem::Specification.find_by_name('$1').gem_dir"); }
turbo="$(gem_dir turbo-rails)"; stimulus="$(gem_dir stimulus-rails)"
cp "$turbo/app/assets/javascripts/turbo.min.js" "$work/out/"
cp "$stimulus/app/assets/javascripts/stimulus.min.js" "$stimulus/app/assets/javascripts/stimulus-loading.js" "$work/out/"
cp "$work/real-blog/app/javascript/application.js" "$work/out/"
cp "$work/real-blog/app/javascript/controllers/"*.js "$work/out/controllers/"
python3 "$pack" "$work/out" "$out_tar"
