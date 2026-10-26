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

  # The scalar readers the params lowering emits, and its record of
  # multiparameter keys. Those keys (`title(1)`) are modeled on the Ruby
  # family only (runtime/ruby/params.rb); this target ignores them, as
  # it always has, so these pass through.
  def scalar_provided(sub, key), do: provided(sub, key)

  def scalar(sub, key), do: str(sub, key, "")

  def no_fields, do: []

  def multiparameter_fields(_sub, _keys), do: []

  def multiparameter_unsupported(_sub, _keys), do: false

  def any_of(list, names), do: Enum.any?(names, &(&1 in list))
end
