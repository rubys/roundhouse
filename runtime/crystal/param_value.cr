# Recursive type alias for request parameters.
#
# Form bodies and URL params arrive as String leaves, Hashes keyed by
# String, and Arrays. JSON bodies add Integer, Float, Bool, and Nil
# leaves while keeping the same recursive structure.
#
# `Roundhouse::ParamValue` is the cross-target type contract: each
# target's runtime defines its own recursive realization (Crystal
# alias here, TS `type ParamValue = …`, Ruby/Spinel dynamic). The
# lowerer emits target-agnostic `is_a?(Hash)` / `is_a?(String)`
# narrowing around accesses; each emit translates `is_a?` to its
# idiomatic narrowing predicate.
#
# Crystal's `alias` admits self-reference through a generic
# constructor (`Hash`/`Array` here) — the same pattern stdlib's
# `JSON::Any` uses internally.

module Roundhouse
  alias ParamValue = String | Int64 | Float64 | Bool | Nil | Hash(String, ParamValue) | Array(ParamValue)
end
