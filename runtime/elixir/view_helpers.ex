# Roundhouse Elixir view helpers — Phase 4d pass-2.
#
# Hand-written, copied by the emitter as
# `lib/roundhouse/view_helpers.ex` when views emit. Minimal set —
# just enough for the scaffold blog's substring-match assertions
# to pass.

defmodule Roundhouse.ViewHelpers do
  @moduledoc false

  def link_to(text, href, opts \\ []) do
    classes = Keyword.get(opts, :class, "")

    class_attr =
      case classes do
        "" -> ""
        c -> " class=\"#{c}\""
      end

    "<a href=\"#{href}\"#{class_attr}>#{text}</a>"
  end

  def button_to(text, href, opts \\ []) do
    method = Keyword.get(opts, :method, :post) |> to_string() |> String.upcase()

    classes = Keyword.get(opts, :class, "")

    class_attr =
      case classes do
        "" -> ""
        c -> " class=\"#{c}\""
      end

    "<form method=\"POST\" action=\"#{href}\"#{class_attr}>" <>
      "<input type=\"hidden\" name=\"_method\" value=\"#{method}\">" <>
      "<button type=\"submit\">#{text}</button></form>"
  end

  def form_wrap(_record, class, inner) do
    class_attr =
      case class do
        "" -> ""
        c -> " class=\"#{c}\""
      end

    "<form#{class_attr}>#{inner}</form>"
  end

  def turbo_stream_from(_target), do: ""

  def dom_id(record, prefix \\ nil) do
    id =
      case record do
        %{id: i} -> i
        _ -> 0
      end

    case prefix do
      nil -> "record_#{id}"
      p -> "#{p}_#{id}"
    end
  end

  def pluralize(count, singular) do
    word =
      cond do
        count == 1 -> singular
        String.ends_with?(singular, "y") -> String.slice(singular, 0..-2//1) <> "ies"
        true -> singular <> "s"
      end

    "#{count} #{word}"
  end
end

defmodule Roundhouse.FormBuilder do
  @moduledoc false
  defstruct record: nil

  def label(_fb, field), do: "<label>#{field}</label>"
  def text_field(_fb, field), do: "<input type=\"text\" name=\"#{field}\">"
  def text_area(_fb, field), do: "<textarea name=\"#{field}\"></textarea>"
  def submit(_fb, text \\ "Submit"), do: "<input type=\"submit\" value=\"#{text}\">"
end
