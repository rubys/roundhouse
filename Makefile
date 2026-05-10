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
	# Target-specific tests (broadcasts/cgi_io/in_memory_adapter +
	# integration/views/models/tools subdirs). emit_spinel layers
	# test/{test_helper,models/{article,comment}_test,controllers/*,
	# fixtures/*} on top via the JSON explode below.
	mkdir -p $(RUBY_OUT)/test
	cp -r runtime/spinel/test/. $(RUBY_OUT)/test/
	# Runtime: framework Ruby (runtime/ruby/) + per-target
	# primitives (runtime/spinel/*.rb). Both land flat under
	# $(RUBY_OUT)/runtime/. runtime/ruby/test/ is roundhouse-side
	# test fixturing — not emitted here.
	mkdir -p $(RUBY_OUT)/runtime
	cp -r runtime/ruby/active_record runtime/ruby/active_support \
	      runtime/ruby/action_view runtime/ruby/action_controller \
	      runtime/ruby/action_dispatch \
	      runtime/ruby/active_record.rb runtime/ruby/action_view.rb \
	      runtime/ruby/action_controller.rb runtime/ruby/action_dispatch.rb \
	      runtime/ruby/inflector.rb $(RUBY_OUT)/runtime/
	cp runtime/spinel/*.rb $(RUBY_OUT)/runtime/
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
