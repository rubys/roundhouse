# JSON — resolved by each tree's own stack. The scaffold boot.rb's
# `require_relative "runtime/json_impl"` lands here uniformly; this file
# just claims the real implementation:
#
# - spinel: the compiler's bundled `json` spin package
#   (packages/json — typed native binding to lib/sp_json.c;
#   `native_func :parse, [:string], :any` etc.), activated by the
#   plain `require`.
# - CRuby / JRuby: the stdlib json.
#
# NOT NAMED `json.rb`, and that is the whole point. A shim that
# DELEGATES to a library must not share the library's name: this file
# is reached by `require_relative`, which registers its absolute path
# in `$LOADED_FEATURES` before the body runs, so if it were `json.rb`
# and `runtime/` were on `$LOAD_PATH`, the `require "json"` below would
# resolve back to THIS file, find it already loading, return false, and
# the real JSON would never load. Every later `JSON.generate` then dies
# with `uninitialized constant JSON` — and `Base64.strict_encode64(
# JSON.generate(stream))` in `turbo_stream_from` is the first casualty.
#
# That is not hypothetical: the emitted Rakefile puts `runtime/` on the
# load path (`t.libs << "runtime"`), and `project.rs`'s BUNDLED table
# WRITES `require "json"` into emitted app and test files that name the
# constant. Both paths need the bare name to reach the real library.
# `runtime/base64.rb` and `runtime/erb.rb` may keep their library names
# because they DEFINE those modules rather than delegating.
#
# This file used to be a hand-rolled `JSON.generate` shim (String
# input only) from before spinel bundled the package — lobsters'
# extras (keybase/github/twitter/diff_bot) call `JSON.parse`, which
# the shim deliberately omitted, and the stdlib/native versions are
# supersets of everything the shim provided (turbo_stream_from's
# `JSON.generate("articles")` → `"\"articles\""` on both).
require "json"
