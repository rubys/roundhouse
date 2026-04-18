# Roundhouse Elixir runtime.
#
# Hand-written Elixir shipped alongside each generated app. The Elixir
# emitter copies this file verbatim into the generated project as
# `lib/roundhouse.ex`. Mirrors runtime/{rust,crystal,go,typescript}.*
# — same per-target posture: minimal surface, each new lowering adds
# exactly what it needs.

defmodule Roundhouse.ValidationError do
  @moduledoc """
  A single validation failure produced by a model's generated `validate`
  function. Carries the attribute name and a human-readable message;
  `full_message/1` composes them into a Rails-compatible display string
  (`"Title can't be blank"`).
  """

  defstruct [:field, :message]

  @type t :: %__MODULE__{field: String.t(), message: String.t()}

  @doc "Constructor matching what the emitted model code calls."
  @spec new(String.t(), String.t()) :: t()
  def new(field, message) when is_binary(field) and is_binary(message) do
    %__MODULE__{field: field, message: message}
  end

  @doc """
  Rails-compatible display form: capitalize the field name, replace
  underscores with spaces, prepend to the message.

      iex> Roundhouse.ValidationError.full_message(%Roundhouse.ValidationError{field: "post_id", message: "can't be blank"})
      "Post id can't be blank"
  """
  @spec full_message(t()) :: String.t()
  def full_message(%__MODULE__{field: field, message: message}) do
    label =
      field
      |> String.replace("_", " ")
      |> String.capitalize()

    "#{label} #{message}"
  end
end
