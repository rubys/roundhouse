# Bridge: framework Ruby lives at runtime/ruby/ in the workspace
# (one source of truth, transpiled per target). For the standalone
# spinel-blog fixture, this file routes the standard
# `require_relative "runtime/active_record"` to the canonical path
# so test_helper.rb / main.rb shapes stay target-shape.
#
# In the demo build (`make spinel-transpile`), runtime/ruby/ contents
# are copied over fixtures/spinel-blog/runtime/ — replacing this
# bridge with the real file — so the emitted demo loads its
# framework from a flat `runtime/` tree (the eventual Spinel-target
# layout).
require_relative "../../../runtime/ruby/active_record"
