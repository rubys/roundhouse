# Narrowing accessors over the recursive request-params tree.
#
# `Roundhouse::ParamValue` was a per-target TYPE WITH NO OPERATIONS, so
# every consumer narrowed at its own call site with `is_a?`, and each
# emitter recognized that as an IDIOM (a type test in `If`-condition
# position whose branches ARE the narrowed value) rather than modeling
# the test. Move the same fact to any other position and it falls off a
# cliff — measured, three spellings each broke a different target.
#
# So the operations live beside the type, hand-written per target the
# way `Db` is: these bodies inspect this target's representation, which
# is what makes them a primitive rather than framework logic.
#
# Semantics fixed by a Rails 8.1 oracle: `permit` keeps a blank "" and
# `Parameters#compact` drops only an explicit nil. So blank counts as
# provided; absent, JSON null, and non-scalar do not.

module Params
  def self.sub(params : Hash(String, Roundhouse::ParamValue), key : String) : Hash(String, Roundhouse::ParamValue)
    value = params[key]?
    value.is_a?(Hash(String, Roundhouse::ParamValue)) ? value : Hash(String, Roundhouse::ParamValue).new
  end

  def self.str(sub : Hash(String, Roundhouse::ParamValue), key : String, fallback : String) : String
    value = sub[key]?
    value.is_a?(String) ? value : fallback
  end

  def self.provided(sub : Hash(String, Roundhouse::ParamValue), key : String) : Bool
    sub[key]?.is_a?(String)
  end
end
