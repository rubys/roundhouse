# Recursive type alias for request parameters.
#
# Form bodies and URL params arrive as a tree of String leaves, Hashes
# keyed by String, and Arrays. Rails' Rack parser walks
# `comment[author][name]=x` and `tags[]=a&tags[]=b` shapes into the
# same recursive structure; the Roundhouse runtime mirrors that.
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
  alias ParamValue = String | Hash(String, ParamValue) | Array(ParamValue)
end
