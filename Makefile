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

# ── Spinel demo: transpile real-blog to a runnable spinel-shape app ──
#
# Produces $(SPINEL_OUT) by overlaying lowered output from real-blog
# on top of a verbatim copy of fixtures/spinel-blog/. The runtime,
# dev server, Gemfile, Makefile, main.rb, app/views.rb, and
# tailwind.css come from the spinel-blog scaffold; the lowered emit
# replaces the hand-written app/{models,controllers,views} and
# config/{schema,routes}.rb. The result is runnable: `make spinel-dev`
# delegates to the inner Makefile's `dev` target → server boots on :3000.
#
# This is the same scaffold-overlay-emit pattern that
# `tests/spinel_toolchain.rs` uses for the toolchain CI job; the
# difference is which target the inner Makefile is asked to run.

SPINEL_OUT ?= build/transpiled-blog

$(SPINEL_OUT)/.stamp: fixtures/real-blog fixtures/spinel-blog
	rm -rf $(SPINEL_OUT)
	mkdir -p $(SPINEL_OUT)
	cp -r fixtures/spinel-blog/. $(SPINEL_OUT)/
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
	touch $(SPINEL_OUT)/.stamp

.PHONY: spinel-transpile
spinel-transpile: $(SPINEL_OUT)/.stamp

.PHONY: spinel-dev spinel-test spinel-run
spinel-dev spinel-test spinel-run: spinel-transpile
	cd $(SPINEL_OUT) && $(MAKE) $(patsubst spinel-%,%,$@)

.PHONY: clean-spinel
clean-spinel:
	rm -rf $(SPINEL_OUT)
