# Hand-written — ActiveRecord::Base is not yet on ELIXIR_RUNTIME
# (while→recursion / instance mutation still growing), but models already
# call `ActiveRecord.Reflection.AssociationReflection.new/2` from the
# lowered `reflect_on_association` surface (#818 / #832). Ship the thin
# struct the call site needs until Base itself transpiles.
defmodule ActiveRecord.Reflection.AssociationReflection do
  defstruct [:name, :counter_cache_column]

  def new(name, counter_cache_column) do
    %__MODULE__{name: name, counter_cache_column: counter_cache_column}
  end

  def name(%__MODULE__{name: name}), do: name
  def counter_cache_column(%__MODULE__{counter_cache_column: c}), do: c
  def klass(_ref), do: nil
  def macro(_ref), do: nil
end
