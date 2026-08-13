# Narrowing accessors over the recursive request-params tree.
#
# `Roundhouse::ParamValue` was a per-target TYPE WITH NO OPERATIONS, so
# every consumer narrowed at its own call site with `is_a?`, and each
# emitter recognized that as an IDIOM rather than modeling the test.
# The operations live beside the type, hand-written per target the way
# `Db` is: these bodies inspect this target's representation, which is
# what makes them a primitive rather than framework logic.
#
# Semantics fixed by a Rails 8.1 oracle: `permit` keeps a blank "" and
# `Parameters#compact` drops only an explicit nil. So blank counts as
# provided; absent, JSON null, and non-scalar do not.

defmodule Params do
  @moduledoc false

  def sub(params, key) do
    case Map.get(params, key) do
      value when is_map(value) -> value
      _ -> %{}
    end
  end

  def str(sub, key, fallback) do
    case Map.get(sub, key) do
      value when is_binary(value) -> value
      _ -> fallback
    end
  end

  def provided(sub, key) do
    is_binary(Map.get(sub, key))
  end
end
