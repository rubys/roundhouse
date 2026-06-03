# Build orchestration for spinel-blog.
#
# Targets:
#   make             — assets + tests + lint (default)
#   make assets      — build static/assets/* (css + importmap JS) + root icons
#   make build       — spinel main.rb → build/blog (requires spinel on PATH)
#   make test        — bundle exec rake test
#   make lint        — bundle exec rake lint (spinel-subset linter)
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
ASSETS := $(STATIC)/assets
BUILD  := build

# Source files that trigger a rebuild when changed. Tailwind also
# rescans these to discover new utility-class references.
RUBY_SRC := main.rb $(shell find runtime app config -type f -name '*.rb' 2>/dev/null)

# Roundhouse emits a `sig/` tree of inferred RBS signatures (the same
# whole-program types every other target consumes). Seed spinel's
# analyzer with them via `--rbs` so the AOT compile keeps fields/params
# concrete that spinel's own inference would otherwise re-widen to the
# boxed `untyped` slow path. Advisory: spinel still trusts observed
# dataflow over a hint, so this narrows what it can and no-ops the rest.
# Guarded on `sig/` existing so emit configs without RBS still build.
RBS_SRC  := $(shell find sig -type f -name '*.rbs' 2>/dev/null)
RBS_FLAG := $(if $(wildcard sig),--rbs sig)

.PHONY: all assets build test lint dev run clean

all: assets test lint

# ── Static assets ────────────────────────────────────────────────
#
# Files land under static/assets/<name> to match the Propshaft-shape
# URLs the importmap pins (config/importmap.rb) and the layout's
# stylesheet_link_tag emit — `/assets/<name>.{css,js}`. The spinel
# binary's Main.dispatch serves these via sphttp sendfile; the CRuby
# overlay's config.ru serves the identical layout via Rack::Static.
# Root icons land at static/<icon> to match the layout's /icon.* hrefs.

# App-side JS copied verbatim from app/javascript/: the importmap entry
# `application.js` (`import "@hotwired/turbo-rails"` — this is what
# boots Turbo, and with it the <turbo-cable-stream-source> connection
# to /cable) plus the Stimulus controllers it pulls in.
ASSET_JS := $(ASSETS)/turbo.min.js \
            $(ASSETS)/stimulus.min.js \
            $(ASSETS)/stimulus-loading.js \
            $(ASSETS)/application.js \
            $(ASSETS)/controllers/application.js \
            $(ASSETS)/controllers/index.js \
            $(ASSETS)/controllers/hello_controller.js

assets: $(ASSETS)/application.css $(ASSETS)/tailwind.css $(ASSET_JS) \
        $(STATIC)/icon.png $(STATIC)/icon.svg

# Two stylesheet outputs to match Rails' Propshaft asset shape:
#   - application.css: empty placeholder, mirrors a Rails app's
#     app/assets/stylesheets/application.css (typically just a
#     manifest comment with no rules of its own).
#   - tailwind.css: the Tailwind v4 build, scanning views + helpers
#     for utility classes. Tailwind needs both `tailwindcss` (engine)
#     and `@tailwindcss/cli` (binary) resolvable in node_modules — npx
#     alone fetches only the CLI. Sentinel file gates the install so
#     it runs once per checkout, not on every build.
$(ASSETS)/application.css:
	@mkdir -p $(ASSETS)
	@touch $@

$(ASSETS)/tailwind.css: app/assets/tailwind.css $(RUBY_SRC) node_modules/.installed
	@mkdir -p $(ASSETS)
	npx @tailwindcss/cli \
	  --input app/assets/tailwind.css \
	  --output $@ \
	  --minify \
	  --content 'app/**/*.rb' \
	  --content 'runtime/**/*.rb'

node_modules/.installed:
	npm install --silent --no-save tailwindcss @tailwindcss/cli
	@touch node_modules/.installed

# JS bundles copied out of the turbo-rails / stimulus-rails gem dirs
# (Gemfile :assets group). Paths are queried at make-time so version
# bumps don't require Makefile edits.
$(ASSETS)/turbo.min.js:
	@mkdir -p $(ASSETS)
	cp "$$(bundle exec ruby -e 'puts Gem::Specification.find_by_name(%q(turbo-rails)).gem_dir')/app/assets/javascripts/turbo.min.js" $@

$(ASSETS)/stimulus.min.js:
	@mkdir -p $(ASSETS)
	cp "$$(bundle exec ruby -e 'puts Gem::Specification.find_by_name(%q(stimulus-rails)).gem_dir')/app/assets/javascripts/stimulus.min.js" $@

$(ASSETS)/stimulus-loading.js:
	@mkdir -p $(ASSETS)
	cp "$$(bundle exec ruby -e 'puts Gem::Specification.find_by_name(%q(stimulus-rails)).gem_dir')/app/assets/javascripts/stimulus-loading.js" $@

# App-side JS — copied verbatim from app/javascript/ (emitted into the
# tree from the source app). controllers/ land under
# static/assets/controllers/ to match the `controllers/*` importmap pins.
$(ASSETS)/application.js: app/javascript/application.js
	@mkdir -p $(ASSETS)
	cp $< $@

$(ASSETS)/controllers/%.js: app/javascript/controllers/%.js
	@mkdir -p $(ASSETS)/controllers
	cp $< $@

# Root-level icons from the source app's public/ (when present in the
# tree — binary PNGs are skipped by the text-only archive, so guard
# with a file test rather than failing the build).
$(STATIC)/icon.png:
	@mkdir -p $(STATIC)
	@if [ -f public/icon.png ]; then cp public/icon.png $@; fi

$(STATIC)/icon.svg:
	@mkdir -p $(STATIC)
	@if [ -f public/icon.svg ]; then cp public/icon.svg $@; fi

# ── Spinel binary ────────────────────────────────────────────────

build: $(BUILD)/blog

$(BUILD)/blog: $(RUBY_SRC) $(RBS_SRC)
	@mkdir -p $(BUILD)
	$(SPINEL) main.rb $(RBS_FLAG) -o $@

# ── Tests / lint ─────────────────────────────────────────────────

test:
	bundle exec rake test

lint:
	bundle exec rake lint

# ── Spinel-compiled tests ────────────────────────────────────────
#
# Each test compiles to its own binary; running each in turn is the
# test suite. The 4 model + controller tests are the same suites
# `tests/spinel_toolchain.rs` runs under CRuby — same source, swapped
# runner. Test files are emitted by `emit_spinel`; the binary list
# is hard-coded here to match.
#
# `make spinel-test` builds + runs all four. A failure aborts on the
# first non-zero exit (per `||` short-circuit) so the offending test
# is surfaced without scrolling through the rest.
#
# Per-test rule pattern: `<dir>/<stem>` depends on `<dir>/<stem>.rb`
# and produces a binary at `$(BUILD)/<dir>/<stem>`. Two-step (compile
# then run) so a build-only check (`make spinel-test-build`) can
# succeed independently of execution.

SPINEL_TESTS := \
	test/models/article_test \
	test/models/comment_test \
	test/controllers/articles_controller_test \
	test/controllers/comments_controller_test

SPINEL_TEST_BINS := $(addprefix $(BUILD)/,$(SPINEL_TESTS))
SPINEL_TEST_RBS  := $(addsuffix .rb,$(SPINEL_TESTS))

$(BUILD)/test/%: test/%.rb $(RUBY_SRC)
	@mkdir -p $(dir $@)
	$(SPINEL) --rbs sig $< -o $@

.PHONY: spinel-test spinel-test-build cruby-test
spinel-test-build: $(SPINEL_TEST_BINS)

spinel-test: $(SPINEL_TEST_BINS)
	@for t in $(SPINEL_TEST_BINS); do \
	  echo "==> $$t"; ./$$t || exit 1; \
	done

# Same 4 tests, CRuby runner. Symmetric pair to `spinel-test` —
# same source files, different runtime. Useful as a baseline gate
# before flipping to spinel-compiled binaries: a regression in the
# emitted Ruby surfaces here first.
cruby-test:
	@for t in $(SPINEL_TEST_RBS); do \
	  echo "==> $$t"; bundle exec ruby $$t || exit 1; \
	done

# ── Dev workflow ─────────────────────────────────────────────────

# `make dev` delegates to `rake dev` — the Rakefile task lives in
# the CRuby overlay (runtime/spinel/scaffold/ruby_overlay/Rakefile)
# and serves on :3000 via Puma + Rack adapter to Main.run. The
# previous in-tree dev_server.rb was retired alongside its
# Thread/Mutex/module_function usage (fundamentally outside the
# spinel subset). Spinel-target builds don't ship the overlay; for
# those, `make dev` errors because no rake task exists, which is the
# correct behavior — the spinel binary's HTTP layer is civetweb +
# FFI, not a Ruby dev server. See memory: project_ruby_dev_server_retirement.
dev:
	bundle exec rake dev

# Single-shot CGI smoke (renders a full page to stdout). Useful for
# eyeballing layout/asset wiring.
run:
	REQUEST_METHOD=GET PATH_INFO=/articles bundle exec ruby main.rb

# ── Clean ────────────────────────────────────────────────────────

clean:
	rm -rf $(BUILD) $(STATIC)
