#!/bin/bash
# ci.yml browser-smoke-ide: the in-browser IDE, playground and studio over the wasm build.
# Usage: browser_smoke_ide.sh WASM MASTODON_SHA RUBY_BENCH_SHA CAMPFIRE_SHA
set -euo pipefail
wasm="$1"; mastodon_sha="$2"; bench_sha="$3"; campfire_sha="$4"
cp "$wasm" wasm/lib/roundhouse_wasm.wasm
t="$(mktemp -d)"
curl -fsSL "https://codeload.github.com/mastodon/mastodon/tar.gz/$mastodon_sha" | tar -xz -C "$t" && mv "$t"/mastodon-* "$t/mastodon"
node wasm/ide/bundle-src.mjs "$t/mastodon" wasm/ide/app-src.json --name mastodon --commit "$mastodon_sha" --open app/controllers/statuses_controller.rb
mkdir -p "$t/lobsters"
curl -fsSL "https://codeload.github.com/ruby/ruby-bench/tar.gz/$bench_sha" | tar -xz -C "$t/lobsters" --strip-components=3 "ruby-bench-$bench_sha/benchmarks/lobsters"
node wasm/ide/bundle-src.mjs "$t/lobsters" "$t/app-lobsters.json" --name lobsters --commit "$bench_sha" --open app/controllers/stories_controller.rb
mkdir -p "$t/campfire"
curl -fsSL "https://codeload.github.com/basecamp/once-campfire/tar.gz/$campfire_sha" | tar -xz -C "$t/campfire" --strip-components=1
node wasm/ide/bundle-src.mjs "$t/campfire" "$t/app-campfire.json" --name campfire --commit "$campfire_sha" --open app/models/message.rb
node wasm/ide/bundle-src.mjs fixtures/real-blog wasm/ide/app-blog.json --name blog --open app/controllers/articles_controller.rb
node wasm/ide/bundle-src.mjs fixtures/store wasm/ide/app-store.json --name store --open app/views/products/index.html.erb
node wasm/ide/bundle-src.mjs fixtures/roda-blog wasm/lib/app-roda.json --name roda-blog --open app.rb
cp "$t/app-lobsters.json" wasm/ide/app-lobsters.json; cp "$t/app-campfire.json" wasm/ide/app-campfire.json
printf '%s\n' '{"default":"store","apps":[{"name":"store","label":"Rails Guides store","src":"app-store.json"},{"name":"blog","label":"Rails blog","src":"app-blog.json"},{"name":"lobsters","label":"Lobsters","src":"app-lobsters.json"},{"name":"campfire","label":"Campfire","src":"app-campfire.json"},{"name":"mastodon","label":"Mastodon","src":"app-src.json"}]}' > wasm/ide/apps.json
cp "$t/app-lobsters.json" wasm/lib/app-lobsters.json; cp "$t/app-campfire.json" wasm/lib/app-campfire.json; cp wasm/ide/app-src.json wasm/lib/app-mastodon.json
printf '%s\n' '{"default":"blog","apps":[{"name":"blog","label":"Rails blog","src":"fixture.json","open":"app/models/article.rb"},{"name":"roda","label":"Roda + Sequel blog","src":"app-roda.json","open":"app.rb"},{"name":"lobsters","label":"Lobsters","src":"app-lobsters.json","open":"app/models/story.rb"},{"name":"campfire","label":"Campfire","src":"app-campfire.json","open":"app/models/message.rb"},{"name":"mastodon","label":"Mastodon","src":"app-mastodon.json","open":"app/models/status.rb"}]}' > wasm/lib/apps.json
(cd tests/browser_smoke && npm ci --no-audit --no-fund && npx playwright install chromium)
(cd wasm && python3 -m http.server 8099 >/tmp/ide-server.log 2>&1 &)
for _ in $(seq 1 30); do curl -sf -o /dev/null http://localhost:8099/ && break; sleep 1; done
curl -sf -o /dev/null http://localhost:8099/ || { echo "wasm/ server did not start" >&2; exit 1; }
export NODE_PATH="$PWD/tests/browser_smoke/node_modules"
(cd wasm/ide && node verify-ide.mjs)
(cd wasm/playground && node verify-playground.mjs)
(cd wasm/studio && node verify-studio.mjs)
