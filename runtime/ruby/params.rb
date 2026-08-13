# Narrowing accessors for the recursive request-params tree.
#
# `Roundhouse::ParamValue` is `String | Hash[String, ParamValue] |
# Array[ParamValue]`, realized per target (Crystal alias, TS union,
# rust `serde_json::Value`, Go `any`). Until now that per-target file
# carried a TYPE AND NO OPERATIONS, so every consumer had to narrow at
# its own call site with `is_a?` — and each emitter recognized that as
# an IDIOM (a type test in `If`-condition position returning the
# narrowed value) rather than modeling the type test itself.
#
# The idiom is fine until you need the same fact somewhere else. Asking
# "was this key provided" three different ways broke three different
# targets, each measured against its toolchain suite:
#
#   `k? && v.is_a?(String)`             go2 — `is_a?(String)` has a
#     type-assertion arm only as an If CONDITION; in a boolean
#     expression it emits `v.IsAPred` against an undefined `String`.
#   `k? && !v.nil?`                     rust2 — renders `nil?` as
#     `is_none()`, which `serde_json::Value` doesn't have (`is_null`).
#   `if v.is_a?(String) then k? else false end`
#                                       go2 + csharp — their narrowing
#     arms assume the branches ARE the narrowed value.
#
# So the operations live here, once, written in the one shape every
# emitter already handles, and callers get ordinary typed calls they can
# use in any position. Same two-layer split as `Db`, except `Db` wraps
# per-target FFI and this is pure logic over the union — so it belongs
# in the transpiled runtime rather than in twelve hand-written copies.
module Params
  # The nested resource hash a controller's params arrive under
  # (`{"article" => {"title" => …}}`). Missing, or present but not a
  # hash, yields an empty hash — a caller reading fields off it then
  # sees them all as not-provided, which is what Rails does with a
  # request that omitted the resource key entirely.
  def self.sub(params, key)
    value = params.fetch(key, {})
    if value.is_a?(Hash)
      value
    else
      {}
    end
  end

  # The scalar under `key`, or `fallback` when the key is absent or
  # holds a non-scalar. Rails' `permit` drops a nested hash or array
  # supplied under a scalar key, so both collapse to the fallback.
  def self.str(sub, key, fallback)
    value = sub.fetch(key, fallback)
    if value.is_a?(String)
      value
    else
      fallback
    end
  end

  # Did the request actually carry a usable value for `key`?
  #
  # Named without a `?` on purpose: predicate mangling for a MODULE
  # function isn't uniform (rust2 makes `provided_pred`, the TS Module
  # mode emits a bare `?` and fails to parse). This is generated code,
  # so portability beats the Ruby idiom.
  #
  # MEASURED against Rails 8.1: `permit` keeps a blank `""` and
  # `Parameters#compact` drops only an explicit nil. So a blank field
  # counts as provided; an absent key, a JSON null, and a non-scalar do
  # not. That distinction is what lets `update` leave a column alone
  # instead of writing `""` over it.
  #
  # The `is_a?` stays in narrowing position (returning the value or
  # nil); the answer is then a plain nil check on a `String?` local,
  # which every target handles — unlike a nil check on the union itself.
  def self.provided(sub, key)
    return false unless sub.key?(key)
    value = sub.fetch(key, "")
    narrowed = if value.is_a?(String)
      value
    else
      nil
    end
    !narrowed.nil?
  end
end
