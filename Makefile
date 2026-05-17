# Developer convenience targets.
#
# The real-blog fixture is derived from `scripts/create-blog` — a
# snapshot of the ruby2js upstream script, maintained here so the
# generator is reproducible without an external git checkout. The
# fixture directory itself is gitignored; populate it locally with
# `make real-blog`, and CI regenerates it on every run.

.PHONY: real-blog
real-blog:
	@echo "Regenerating fixtures/real-blog/ …"
	rm -rf fixtures/real-blog
	scripts/create-blog $(CURDIR)/fixtures/real-blog
	cd fixtures/real-blog && bundle install --quiet && bin/rails db:prepare

.PHONY: clean-real-blog
clean-real-blog:
	rm -rf fixtures/real-blog

# Run the end-to-end cross-rendering compare for a target. Drives
# regenerate → build → seed → start Rails + target → diff. See
# `scripts/compare --help` for flags. Default target is typescript.
.PHONY: compare
compare:
	scripts/compare $(or $(TARGET),typescript)

.PHONY: compare-rust
compare-rust:
	scripts/compare rust

.PHONY: compare-ts
compare-ts:
	scripts/compare typescript

.PHONY: compare-ruby
compare-ruby:
	scripts/compare ruby

# ── Ruby target: transpile real-blog to a runnable Ruby/CRuby app ──
#
# Produces $(RUBY_OUT) by overlaying lowered output from real-blog
# on top of the Ruby-target scaffold (runtime/spinel/scaffold/, retained
# under that path until a follow-up move) and runtime trees. Scaffold
# provides Gemfile, inner Makefile, main.rb, app/views.rb,
# app/assets/tailwind.css, tools/, .gitignore. Runtime is framework
# Ruby (runtime/ruby/) plus per-target primitives currently still under
# runtime/spinel/*.rb. Lowered emit fills app/{models,controllers,views},
# config/{schema,routes}.rb, and test/{test_helper,models,controllers,
# fixtures}. The result is runnable: `make ruby-dev` delegates to the
# inner Makefile's `dev` target → server boots on :3000.
#
# This is the same scaffold-overlay-emit pattern that
# `tests/ruby_toolchain.rs` uses for the toolchain CI job. The eventual
# Spinel-AOT target will reuse the same lowered emit through a parallel
# job that invokes the spinel binary; not yet wired.

RUBY_OUT ?= build/transpiled-blog

$(RUBY_OUT)/.stamp: fixtures/real-blog runtime/ruby runtime/spinel
	rm -rf $(RUBY_OUT)
	mkdir -p $(RUBY_OUT)
	# Verbatim scaffold for the output tree.
	cp -r runtime/spinel/scaffold/. $(RUBY_OUT)/
	# Strip the overlay subdir; its contents land at the top of the
	# emit tree below, AFTER the manifest explode (otherwise the
	# explode rewrites Rakefile from the scaffold's base copy).
	rm -rf $(RUBY_OUT)/ruby_overlay
	# Target-specific tests (broadcasts/cgi_io + integration/views/
	# models/tools subdirs). emit_spinel layers
	# test/{test_helper,models/{article,comment}_test,controllers/*,
	# fixtures/*} on top via the JSON explode below.
	mkdir -p $(RUBY_OUT)/test
	cp -r runtime/spinel/test/. $(RUBY_OUT)/test/
	# Runtime: framework Ruby (runtime/ruby/) + per-target
	# primitives (runtime/spinel/*.rb). Both land flat under
	# $(RUBY_OUT)/runtime/. runtime/ruby/test/ is roundhouse-side
	# test fixturing — not emitted here.
	mkdir -p $(RUBY_OUT)/runtime
	cp -r runtime/ruby/active_record \
	      runtime/ruby/action_view runtime/ruby/action_controller \
	      runtime/ruby/action_dispatch \
	      runtime/ruby/active_record.rb runtime/ruby/action_view.rb \
	      runtime/ruby/action_controller.rb runtime/ruby/action_dispatch.rb \
	      runtime/ruby/inflector.rb runtime/ruby/json_builder.rb \
	      $(RUBY_OUT)/runtime/
	cp runtime/spinel/*.rb $(RUBY_OUT)/runtime/
	# Runtime RBS lives under sig/runtime/, not runtime/, so every
	# .rbs in the tree sits under one sig/ root. spinel's --rbs DIR
	# and Steep both walk a single tree. Strip any .rbs that rode
	# along the subdir cp -r above, then mirror runtime/ruby/**/*.rbs
	# into sig/runtime/ (catches top-level files like inflector.rbs
	# that the by-name .rb copy above explicitly skipped).
	find $(RUBY_OUT)/runtime -name '*.rbs' -delete
	mkdir -p $(RUBY_OUT)/sig/runtime
	find runtime/ruby -name '*.rbs' | while IFS= read -r f; do \
	  rel="$${f#runtime/ruby/}"; \
	  mkdir -p "$(RUBY_OUT)/sig/runtime/$$(dirname "$$rel")"; \
	  cp "$$f" "$(RUBY_OUT)/sig/runtime/$$rel"; \
	done
	cargo run --release --bin build-site -- fixtures/real-blog $(RUBY_OUT)/.emit
	ruby -rjson -rfileutils -e ' \
	  m = JSON.parse(File.read(ARGV[0])); \
	  m["files"].each do |f|; \
	    p = File.join(ARGV[1], f["path"]); \
	    FileUtils.mkdir_p(File.dirname(p)); \
	    File.write(p, f["content"]); \
	  end' \
	  $(RUBY_OUT)/.emit/browse/spinel.json $(RUBY_OUT)
	rm -rf $(RUBY_OUT)/.emit
	# Per-target Db shim selection. `runtime/spinel/` carries both
	# variants — db.rb (FFI, for the future Spinel-AOT target) and
	# db_cruby.rb (gem-backed, for CRuby/MRI). The Ruby target's
	# emitted tree gets exactly one `db.rb` — the gem variant —
	# so main.rb's `require_relative "runtime/db"` resolves to a
	# CRuby-runnable shim. Applied after the manifest explode so
	# it's the final state regardless of what the archive emitted.
	mv $(RUBY_OUT)/runtime/db_cruby.rb $(RUBY_OUT)/runtime/db.rb
	# Ruby-target overlay: main.rb (CGI-shaped, replaces scaffold's
	# Tep-based dispatch), Rakefile (replaces base), config.ru,
	# config/puma.rb. Applied AFTER the manifest explode so the
	# overlay isn't reverted by the scaffold-walk that build-site
	# does. Spinel target doesn't get these (it uses the base
	# scaffold's Tep::Server-based main.rb + the vendored tep tree).
	cp -r runtime/spinel/scaffold/ruby_overlay/. $(RUBY_OUT)/
	# Source-app static files: app/javascript/* (importmap-served
	# JS modules) and public/* (icons, robots.txt). Build-site
	# doesn't include these — they're verbatim assets, not
	# transpilable Ruby. `rake assets` reads from app/javascript/
	# to populate static/assets/.
	if [ -d fixtures/real-blog/app/javascript ]; then \
	  mkdir -p $(RUBY_OUT)/app/javascript && \
	  cp -r fixtures/real-blog/app/javascript/. $(RUBY_OUT)/app/javascript/; \
	fi
	if [ -d fixtures/real-blog/public ]; then \
	  mkdir -p $(RUBY_OUT)/public && \
	  cp -r fixtures/real-blog/public/. $(RUBY_OUT)/public/; \
	fi
	# Seed the demo DB from real-blog's Rails-populated SQLite. The
	# Rails-generated schema (varchar/text/datetime affinities) reads
	# fine through SqliteAdapter; main.rb's Schema.load! is idempotent
	# (CREATE TABLE IF NOT EXISTS) so it no-ops over the existing
	# tables. Copy gives the demo three articles + comments out of
	# the box; mutations land on the demo's copy, so real-blog stays
	# pristine. `make clean-ruby` resets to seeded state.
	mkdir -p $(RUBY_OUT)/tmp
	cp fixtures/real-blog/storage/development.sqlite3 $(RUBY_OUT)/tmp/blog.sqlite3
	touch $(RUBY_OUT)/.stamp

.PHONY: ruby-transpile
ruby-transpile: $(RUBY_OUT)/.stamp

.PHONY: ruby-dev ruby-test ruby-run
ruby-dev ruby-test ruby-run: ruby-transpile
	cd $(RUBY_OUT) && $(MAKE) $(patsubst ruby-%,%,$@)

.PHONY: clean-ruby
clean-ruby:
	rm -rf $(RUBY_OUT)

# ── Spinel target: same lowered emit as the Ruby target, but the
# per-target Db shim is the FFI variant (`runtime/spinel/db.rb`)
# instead of the gem variant. Output tree is intended for
# `spinel main.rb -o build/blog` (AOT compile to a native binary).
# Until spinel can compile the framework runtime end-to-end (residual
# tracked in the issue cadence), this target produces a tree that's
# useful for spinel-AOT error triage but not yet runnable end-to-end.

SPINEL_OUT ?= build/transpiled-blog-spinel

$(SPINEL_OUT)/.stamp: fixtures/real-blog runtime/ruby runtime/spinel
	rm -rf $(SPINEL_OUT)
	mkdir -p $(SPINEL_OUT)
	cp -r runtime/spinel/scaffold/. $(SPINEL_OUT)/
	# Strip the CRuby-only overlay dir; spinel target keeps the base
	# Makefile and doesn't get config.ru / config/puma.rb.
	rm -rf $(SPINEL_OUT)/ruby_overlay
	mkdir -p $(SPINEL_OUT)/test
	cp -r runtime/spinel/test/. $(SPINEL_OUT)/test/
	mkdir -p $(SPINEL_OUT)/runtime
	cp -r runtime/ruby/active_record \
	      runtime/ruby/action_view runtime/ruby/action_controller \
	      runtime/ruby/action_dispatch \
	      runtime/ruby/active_record.rb runtime/ruby/action_view.rb \
	      runtime/ruby/action_controller.rb runtime/ruby/action_dispatch.rb \
	      runtime/ruby/inflector.rb runtime/ruby/json_builder.rb \
	      $(SPINEL_OUT)/runtime/
	cp runtime/spinel/*.rb $(SPINEL_OUT)/runtime/
	# Runtime RBS lives under sig/runtime/ (see RUBY_OUT block above
	# for rationale — one sig/ root for spinel --rbs DIR + Steep).
	find $(SPINEL_OUT)/runtime -name '*.rbs' -delete
	mkdir -p $(SPINEL_OUT)/sig/runtime
	find runtime/ruby -name '*.rbs' | while IFS= read -r f; do \
	  rel="$${f#runtime/ruby/}"; \
	  mkdir -p "$(SPINEL_OUT)/sig/runtime/$$(dirname "$$rel")"; \
	  cp "$$f" "$(SPINEL_OUT)/sig/runtime/$$rel"; \
	done
	# Vendored Tep transport (FFI HTTP server) — replaces CGI dispatch.
	# sphttp.o is precompiled here so spinel's ffi_cflags substitution
	# lands a path that exists at the moment spinel reads net.rb.
	mkdir -p $(SPINEL_OUT)/runtime/tep
	cp runtime/spinel/tep/*.rb runtime/spinel/tep/sphttp.c $(SPINEL_OUT)/runtime/tep/
	cc -O2 -c $(SPINEL_OUT)/runtime/tep/sphttp.c -o $(SPINEL_OUT)/runtime/tep/sphttp.o
	sed -i.bak 's|@TEP_SPHTTP_O@|runtime/tep/sphttp.o|' $(SPINEL_OUT)/runtime/tep/net.rb && rm $(SPINEL_OUT)/runtime/tep/net.rb.bak
	cargo run --release --bin build-site -- fixtures/real-blog $(SPINEL_OUT)/.emit
	ruby -rjson -rfileutils -e ' \
	  m = JSON.parse(File.read(ARGV[0])); \
	  m["files"].each do |f|; \
	    p = File.join(ARGV[1], f["path"]); \
	    FileUtils.mkdir_p(File.dirname(p)); \
	    File.write(p, f["content"]); \
	  end' \
	  $(SPINEL_OUT)/.emit/browse/spinel.json $(SPINEL_OUT)
	rm -rf $(SPINEL_OUT)/.emit
	# Spinel target's tree keeps the FFI `db.rb` and drops `db_cruby.rb`
	# (the gem variant is for the Ruby target). Symmetric to ruby-
	# transpile's `mv db_cruby.rb db.rb`.
	rm -f $(SPINEL_OUT)/runtime/db_cruby.rb
	touch $(SPINEL_OUT)/.stamp

.PHONY: spinel-transpile
spinel-transpile: $(SPINEL_OUT)/.stamp

.PHONY: clean-spinel
clean-spinel:
	rm -rf $(SPINEL_OUT)
