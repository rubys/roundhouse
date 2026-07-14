# JSON — resolved by each tree's own stack. The scaffold main.rb's
# `require_relative "runtime/json"` lands here uniformly; this file
# just claims the real implementation:
#
# - spinel: the compiler's bundled `json` spin package
#   (packages/json — typed native binding to lib/sp_json.c;
#   `native_func :parse, [:string], :any` etc.), activated by the
#   plain `require`.
# - CRuby / JRuby: the stdlib json.
#
# This file used to be a hand-rolled `JSON.generate` shim (String
# input only) from before spinel bundled the package — lobsters'
# extras (keybase/github/twitter/diff_bot) call `JSON.parse`, which
# the shim deliberately omitted, and the stdlib/native versions are
# supersets of everything the shim provided (turbo_stream_from's
# `JSON.generate("articles")` → `"\"articles\""` on both).
require "json"
