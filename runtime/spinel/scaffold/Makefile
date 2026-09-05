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

.PHONY: all assets build seed test lint dev run clean

all: assets test lint

# ── Static assets ────────────────────────────────────────────────
#
# Files land under static/assets/<name> to match the Propshaft-shape
# URLs the importmap pins (config/importmap.rb) and the layout's
# stylesheet_link_tag emit — `/assets/<name>.{css,js}`. The spinel
# binary's Main.dispatch serves these via sendfile over sp_net; the CRuby
# overlay's config.ru serves the identical layout via Rack::Static.
# Root icons land at static/<icon> to match the layout's /icon.* hrefs.

# One target per module the import map pins.
#
# GENERATED — `project.rs::apply_makefile_asset_list` rewrites this list
# (and the gem rules below it) from the ingested app's own
# `config/importmap.rb`. The blog fixture pins seven modules; campfire
# pins ninety-five, three directory levels deep. The list below is the
# blog's, and it is also the anchor the generator matches on: editing it
# by hand edits a generated file.
#
# Each pin's bytes come from one of three roots, and the generator sorts
# every pin into one: the app's `app/javascript/`, its
# `vendor/javascript/` (importmap-rails' other source root), or a gem
# that ships a prebuilt bundle. The first two are served by the pattern
# rules further down; a gem asset needs an explicit rule, because only
# `bundle` can say where the gem dir is.
ASSET_JS := $(ASSETS)/turbo.min.js \
            $(ASSETS)/stimulus.min.js \
            $(ASSETS)/stimulus-loading.js \
            $(ASSETS)/application.js \
            $(ASSETS)/controllers/application.js \
            $(ASSETS)/controllers/index.js \
            $(ASSETS)/controllers/hello_controller.js

# One target per stylesheet on the app's Propshaft load path.
#
# GENERATED, the same way ASSET_JS is, and from the same list the
# `stylesheet_link_tag` group expansion renders `<link>` tags from — so
# the page cannot ask for a stylesheet the build does not produce. The
# blog fixture has two (a manifest and a Tailwind build); campfire has
# twenty-six hand-written files and no Tailwind at all.
ASSET_CSS := $(ASSETS)/application.css \
             $(ASSETS)/tailwind.css

# Images, discovered rather than generated. There is no manifest to
# derive this from — `image_tag "add.svg"` is an ordinary call in a view,
# not a declaration — so the tree itself is the list, and it stays
# correct without a generator. `find` at make-time also picks up the
# BINARY images, which arrive by a different route (`write_binary_assets`,
# after the text file set is assembled) and so are not visible to one.
#
# campfire draws its interface in SVG and needs all eighty; the blog
# fixture has an empty images dir and gets an empty list.
ASSET_IMG := $(patsubst app/assets/images/%,$(ASSETS)/%,\
               $(shell find app/assets/images -type f 2>/dev/null))

assets: $(ASSET_CSS) $(ASSET_JS) $(ASSET_IMG) \
        $(STATIC)/icon.png $(STATIC)/icon.svg

# Stylesheets copied verbatim out of the app's two Propshaft roots,
# both walked into the emitted tree. `app/assets/builds/` is where a CSS
# *build* lands (tailwindcss-rails writes tailwind.css there), so it is
# tried second — and an app that really builds its Tailwind gets the
# explicit rule below instead, which beats both patterns.
$(ASSETS)/%.css: app/assets/stylesheets/%.css
	@mkdir -p $(dir $@)
	cp $< $@

$(ASSETS)/%.css: app/assets/builds/%.css
	@mkdir -p $(dir $@)
	cp $< $@

# The Tailwind v4 build, scanning the emitted Ruby for utility classes.
# Tailwind needs both `tailwindcss` (engine) and `@tailwindcss/cli`
# (binary) resolvable in node_modules — npx alone fetches only the CLI.
# The sentinel gates the install so it runs once per checkout.
#
# GENERATED: present only for an app whose stylesheet list actually
# includes `tailwind`. An app that writes plain CSS gets neither this
# rule nor the npm install behind it, which is the difference between
# `make assets` needing Node and not — campfire does not, and used to
# build a Tailwind stylesheet it never links.
$(ASSETS)/tailwind.css: app/assets/tailwind.css $(RUBY_SRC) node_modules/.installed
	@mkdir -p $(dir $@)
	npx @tailwindcss/cli \
	  --input app/assets/tailwind.css \
	  --output $@ \
	  --minify \
	  --content 'app/**/*.rb' \
	  --content 'runtime/**/*.rb'

node_modules/.installed:
	npm install --silent --no-save tailwindcss @tailwindcss/cli
	@touch node_modules/.installed

# JS bundles copied out of the gems that ship them. Paths are queried at
# make-time so version bumps don't require Makefile edits. GENERATED
# alongside ASSET_JS above — only the bundles this app's import map
# actually pins get a rule, and each names the gem it comes from.
$(ASSETS)/turbo.min.js:
	@mkdir -p $(dir $@)
	cp "$$(bundle exec ruby -e 'puts Gem::Specification.find_by_name(%q(turbo-rails)).gem_dir')/app/assets/javascripts/turbo.min.js" $@

$(ASSETS)/stimulus.min.js:
	@mkdir -p $(dir $@)
	cp "$$(bundle exec ruby -e 'puts Gem::Specification.find_by_name(%q(stimulus-rails)).gem_dir')/app/assets/javascripts/stimulus.min.js" $@

$(ASSETS)/stimulus-loading.js:
	@mkdir -p $(dir $@)
	cp "$$(bundle exec ruby -e 'puts Gem::Specification.find_by_name(%q(stimulus-rails)).gem_dir')/app/assets/javascripts/stimulus-loading.js" $@

# App-side and vendored JS, copied verbatim out of the source app's
# `app/javascript/` and `vendor/javascript/` — both walked into the
# emitted tree, both roots importmap-rails resolves `pin_all_from`
# against.
#
# `%` spans slashes in GNU make, so ONE rule per root covers every depth
# a recursive `pin_all_from` can reach. That matters: campfire pins
# `lib/autocomplete/custom_elements/suggestion_option` three levels down,
# and the per-directory rules this replaced would have needed a new line
# per directory per app.
#
# Two rules rather than one because make takes the first pattern whose
# prerequisite exists, which resolves a pin against `app/javascript/`
# first and `vendor/javascript/` second — importmap-rails' own
# precedence. Gem bundles never reach either: their explicit rules above
# win outright, as explicit rules always do.
$(ASSETS)/%.js: app/javascript/%.js
	@mkdir -p $(dir $@)
	cp $< $@

$(ASSETS)/%.js: vendor/javascript/%.js
	@mkdir -p $(dir $@)
	cp $< $@

# Images — SVG, PNG and the animated GIFs campfire's `/play` command
# serves out of `images/sounds/`. One rule, any extension, any depth:
# unlike JS and CSS these have no import map or load path to enumerate
# them, so the pattern has to be as wide as `image_tag` is.
$(ASSETS)/%: app/assets/images/%
	@mkdir -p $(dir $@)
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

# Populate storage/development.sqlite3 (the Rails-traditional path, and
# the binary's default when BLOG_DB is unset) with the demo blog's seed
# data (db/seed.sql). The binary boots against an empty DB fine
# (Schema.load! is idempotent), but a self-contained run/repro wants rows
# to serve — `make seed` provides them with nothing but sqlite3. Override
# the path with BLOG_DB=… at runtime.
seed: storage/development.sqlite3
storage/development.sqlite3: db/seed.sql
	@mkdir -p storage
	sqlite3 $@ < db/seed.sql

# spinel AOT-compiles main.rb (+ its whole require graph) straight to a
# native binary. The HTTP/WebSocket transport now rides spinel's built-in,
# auto-linked `sp_net` — there's no vendored C extension to compile or
# link (the former runtime/tep/sphttp.c + `@TEP_SPHTTP_O@` placeholder
# were retired in the sp_net migration, matz/spinel#1466).
# spin.toml is a prerequisite because it CHANGES THE BINARY: it carries
# the package dependencies and, since matz/spinel#4344, the allocator
# this program links. Without it here, editing the manifest leaves make
# with nothing to do and the next `ldd` reads a binary built before the
# edit — which is how this was found.
$(BUILD)/blog: $(RUBY_SRC) $(RBS_SRC) spin.toml
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
# correct behavior — the spinel binary carries its own HTTP server
# (runtime/tep/server_threaded.rb over sp_net), not a Ruby dev server.
dev:
	bundle exec rake dev

# Single-shot CGI smoke (renders a full page to stdout). Useful for
# eyeballing layout/asset wiring.
run:
	REQUEST_METHOD=GET PATH_INFO=/articles bundle exec ruby main.rb

# ── Clean ────────────────────────────────────────────────────────

clean:
	rm -rf $(BUILD) $(STATIC)
