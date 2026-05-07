# Build orchestration for spinel-blog.
#
# Targets:
#   make             — assets + tests + lint (default)
#   make assets      — build static/{application,tailwind}.css and static/turbo.min.js
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
BUILD  := build

# Source files that trigger a rebuild when changed. Tailwind also
# rescans these to discover new utility-class references.
RUBY_SRC := main.rb $(shell find runtime app config -type f -name '*.rb' 2>/dev/null)

.PHONY: all assets build test lint dev run clean

all: assets test lint

# ── Static assets ────────────────────────────────────────────────

assets: $(STATIC)/application.css $(STATIC)/tailwind.css $(STATIC)/turbo.min.js

# Two stylesheet outputs to match Rails' Propshaft asset shape:
#   - application.css: empty placeholder, mirrors a Rails app's
#     app/assets/stylesheets/application.css (typically just a
#     manifest comment with no rules of its own).
#   - tailwind.css: the Tailwind v4 build, scanning views + helpers
#     for utility classes. Tailwind needs both `tailwindcss` (engine)
#     and `@tailwindcss/cli` (binary) resolvable in node_modules — npx
#     alone fetches only the CLI. Sentinel file gates the install so
#     it runs once per checkout, not on every build.
$(STATIC)/application.css:
	@mkdir -p $(STATIC)
	@touch $@

$(STATIC)/tailwind.css: app/assets/tailwind.css $(RUBY_SRC) node_modules/.installed
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
	$(SPINEL) $< -o $@

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

# `make dev` (CRuby HTTP+WebSocket dev server) was retired alongside
# server/dev_server.rb in the lint-spinel cleanup; the dev server's
# Thread/Mutex/module_function usage is fundamentally outside the
# spinel subset. Native HTTP is staged for civetweb in collaboration
# with the spinel team once the backend lands. See memory:
# project_ruby_dev_server_retirement.

# Single-shot CGI smoke (renders a full page to stdout). Useful for
# eyeballing layout/asset wiring.
run:
	REQUEST_METHOD=GET PATH_INFO=/articles bundle exec ruby main.rb

# ── Clean ────────────────────────────────────────────────────────

clean:
	rm -rf $(BUILD) $(STATIC)
