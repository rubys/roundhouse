# Build orchestration for spinel-blog.
#
# Targets:
#   make             — assets + tests + lint (default)
#   make assets      — build static/app.css and static/turbo.min.js
#   make build       — spinel main.rb → build/blog (requires spinel on PATH)
#   make test        — bundle exec rake test
#   make lint        — bundle exec rake lint (spinel-subset linter)
#   make dev         — run server/dev_server.rb (HTTP + WebSocket + watcher)
#   make run         — single-shot CRuby smoke (REQUEST_METHOD=GET PATH_INFO=/articles)
#   make clean       — rm -rf build/ static/
#
# Pattern follows railcar's CI: turbo-rails gem (already in Gemfile)
# ships the JS bundle; tailwind comes via npm + @tailwindcss/cli.
#
# spinel itself is treated as an external tool; `make build` only
# succeeds when `spinel` is on PATH. Override via `SPINEL=path/to/spinel`.

SPINEL ?= spinel
STATIC := static
BUILD  := build

# Source files that trigger a rebuild when changed. Tailwind also
# rescans these to discover new utility-class references.
RUBY_SRC := main.rb $(shell find runtime app config -type f -name '*.rb' 2>/dev/null)

.PHONY: all assets build test lint dev run clean

all: assets test lint

# ── Static assets ────────────────────────────────────────────────

assets: $(STATIC)/app.css $(STATIC)/turbo.min.js

# CSS: scan view + helper sources for utility classes; emit minified.
# Tailwind v4 needs both `tailwindcss` (the engine) and
# `@tailwindcss/cli` (the binary) resolvable in node_modules — npx
# alone fetches only the CLI. Sentinel file gates the install so it
# runs once per checkout, not on every build.
$(STATIC)/app.css: app/assets/tailwind.css $(RUBY_SRC) node_modules/.installed
	@mkdir -p $(STATIC)
	npx @tailwindcss/cli \
	  --input app/assets/tailwind.css \
	  --output $@ \
	  --minify \
	  --content 'app/**/*.rb' \
	  --content 'runtime/**/*.rb'

node_modules/.installed:
	npm install --silent --no-save tailwindcss @tailwindcss/cli
	@touch node_modules/.installed

# JS: copy out of the turbo-rails gem dir. Bundler ensures the gem
# is installed; the path is queried at make-time so version bumps
# don't require Makefile edits.
$(STATIC)/turbo.min.js:
	@mkdir -p $(STATIC)
	cp "$$(bundle exec ruby -e 'puts Gem::Specification.find_by_name(%q(turbo-rails)).gem_dir + %q(/app/assets/javascripts/turbo.min.js)')" $@

# ── Spinel binary ────────────────────────────────────────────────

build: $(BUILD)/blog

$(BUILD)/blog: $(RUBY_SRC)
	@mkdir -p $(BUILD)
	$(SPINEL) main.rb -o $@

# ── Tests / lint ─────────────────────────────────────────────────

test:
	bundle exec rake test

lint:
	bundle exec rake lint

# ── Dev workflow ─────────────────────────────────────────────────

# Runs the dev server — pure-Ruby HTTP+WebSocket front end that
# dispatches requests to main.rb and forwards broadcasts over
# WebSockets to subscribed Turbo clients.
dev: assets
	bundle exec ruby server/dev_server.rb

# Single-shot CGI smoke (renders a full page to stdout). Useful for
# eyeballing layout/asset wiring.
run:
	REQUEST_METHOD=GET PATH_INFO=/articles bundle exec ruby main.rb

# ── Clean ────────────────────────────────────────────────────────

clean:
	rm -rf $(BUILD) $(STATIC)
