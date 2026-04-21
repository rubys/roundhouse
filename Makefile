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
