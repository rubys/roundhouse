# Roundhouse Elixir view helpers.
#
# Hand-written, shipped alongside generated code (copied in by the
# Elixir emitter as `lib/roundhouse/view_helpers.ex`). Ports the
# same helper surface as rust/ts/python/go/crystal — link_to,
# button_to, FormBuilder, turbo_stream_from, dom_id, pluralize,
# truncate, layout-slot storage (yield + content_for).
#
# Render state lives in a global `:ets` table so the sequential
# compare-tool flow works without per-process plumbing. When we
# grow past the scaffold, swap to `Process.put`/`Process.get` for
# per-request state — Cowboy runs each request on its own process.

defmodule Roundhouse.FormBuilder do
  defstruct prefix: "", css_class: "", is_persisted: false, record: nil
end

defmodule Roundhouse.ViewHelpers do
  @table :roundhouse_render_state

  # ── Render state ────────────────────────────────────────────

  def reset_render_state do
    ensure_table()
    :ets.delete_all_objects(@table)
  end

  def set_yield(body) do
    ensure_table()
    :ets.insert(@table, {:__yield__, body})
    :ok
  end

  def get_yield do
    ensure_table()

    case :ets.lookup(@table, :__yield__) do
      [{_, body}] -> body
      _ -> ""
    end
  end

  def get_slot(name) do
    ensure_table()
    key = {:slot, to_string(name)}

    case :ets.lookup(@table, key) do
      [{_, body}] -> body
      _ -> ""
    end
  end

  def content_for_set(slot, body) do
    ensure_table()
    :ets.insert(@table, {{:slot, to_string(slot)}, body})
    :ok
  end

  def content_for_get(slot), do: get_slot(slot)

  defp ensure_table do
    case :ets.whereis(@table) do
      :undefined -> :ets.new(@table, [:named_table, :public, :set])
      _tid -> :ok
    end
  end

  # ── Layout-meta helpers ─────────────────────────────────────

  def csrf_meta_tags do
    ~s(<meta name="csrf-param" content="authenticity_token" />\n<meta name="csrf-token" content="" />)
  end

  def csp_meta_tag, do: ""

  def stylesheet_link_tag(name, opts \\ %{}) do
    href = "/assets/#{name}.css"
    ~s(<link rel="stylesheet" href="#{escape_html(href)}"#{sorted_attrs(opts)} />)
  end

  def javascript_importmap_tags(pins, main_entry \\ "application") do
    imports_json =
      pins
      |> Enum.with_index()
      |> Enum.map_join("\n", fn {{name, path}, i} ->
        sep = if i + 1 < length(pins), do: ",", else: ""
        ~s(    #{Jason.encode!(name)}: #{Jason.encode!(path)}#{sep})
      end)

    preloads =
      Enum.map_join(pins, "", fn {_name, path} ->
        "\n" <> ~s(<link rel="modulepreload" href="#{escape_html(path)}">)
      end)

    ~s(<script type="importmap" data-turbo-track="reload">{\n) <>
      ~s(  "imports": {\n) <>
      imports_json <>
      "\n" <>
      "  }\n" <>
      "}</script>" <>
      preloads <>
      "\n" <>
      ~s(<script type="module">import "#{escape_html(main_entry)}"</script>)
  end

  # ── link_to / button_to ─────────────────────────────────────

  def link_to(text, url, opts \\ %{}) do
    ~s(<a href="#{escape_html(url)}"#{sorted_attrs(opts)}>#{escape_html(text)}</a>)
  end

  def button_to(text, target, opts \\ %{}) do
    opts = ensure_map(opts)
    method = Map.get(opts, "method", "post")
    button_class = Map.get(opts, "class", "")
    form_class = Map.get(opts, "form_class", "button_to")
    method_lower = String.downcase(method)

    method_input =
      if method_lower != "post" and method_lower != "get" do
        ~s(<input type="hidden" name="_method" value="#{escape_html(method)}" />)
      else
        ""
      end

    button_attrs =
      opts
      |> Enum.sort_by(&elem(&1, 0))
      |> Enum.filter(fn {k, _} -> String.starts_with?(k, "data-") end)
      |> Enum.map_join("", fn {k, v} -> ~s( #{escape_html(k)}="#{escape_html(v)}") end)

    button_cls_attr =
      if button_class == "", do: "", else: ~s( class="#{escape_html(button_class)}")

    csrf_input = ~s(<input type="hidden" name="authenticity_token" value="">)

    ~s(<form class="#{escape_html(form_class)}" method="post" action="#{escape_html(target)}">) <>
      method_input <>
      "<button" <>
      button_cls_attr <>
      button_attrs <>
      ~s( type="submit">) <>
      escape_html(text) <>
      "</button>" <>
      csrf_input <>
      "</form>"
  end

  # ── form_with wrapper ───────────────────────────────────────

  def form_wrap(action, is_persisted, html_class, inner) do
    class_attr = if html_class == "", do: "", else: ~s( class="#{escape_html(html_class)}")
    method_input = if is_persisted, do: ~s(<input type="hidden" name="_method" value="patch">), else: ""
    csrf_input = ~s(<input type="hidden" name="authenticity_token" value="">)

    ~s(<form#{class_attr} action="#{escape_html(action)}" accept-charset="UTF-8" method="post">) <>
      method_input <>
      csrf_input <>
      inner <>
      "</form>"
  end

  # ── FormBuilder ─────────────────────────────────────────────

  def form_builder(prefix, css_class \\ "", is_persisted \\ false) do
    %Roundhouse.FormBuilder{
      prefix: to_string(prefix),
      css_class: css_class,
      is_persisted: is_persisted
    }
  end

  def fb_label(%Roundhouse.FormBuilder{prefix: prefix}, field, opts \\ %{}) do
    opts = ensure_map(opts)
    cls = Map.get(opts, "class", "")
    class_attr = if cls == "", do: "", else: ~s( class="#{escape_html(cls)}")
    text = capitalize_first(to_string(field))

    ~s(<label for="#{escape_html(id_for(prefix, field))}"#{class_attr}>#{escape_html(text)}</label>)
  end

  def fb_text_field(%Roundhouse.FormBuilder{prefix: prefix}, field, value \\ "", opts \\ %{}) do
    opts = ensure_map(opts)
    cls = Map.get(opts, "class", "")
    class_attr = if cls == "", do: "", else: ~s( class="#{escape_html(cls)}")
    value_attr = if value == "" or is_nil(value), do: "", else: ~s( value="#{escape_html(value)}")

    ~s(<input type="text" name="#{escape_html(name_for(prefix, field))}" id="#{escape_html(id_for(prefix, field))}"#{value_attr}#{class_attr} />)
  end

  def fb_textarea(%Roundhouse.FormBuilder{prefix: prefix}, field, value \\ "", opts \\ %{}) do
    opts = ensure_map(opts)
    cls = Map.get(opts, "class", "")
    class_attr = if cls == "", do: "", else: ~s( class="#{escape_html(cls)}")
    rows = Map.get(opts, "rows", "")
    rows_attr = if rows == "", do: "", else: ~s( rows="#{escape_html(rows)}")
    body = if value == "" or is_nil(value), do: "", else: escape_html(value)

    ~s(<textarea#{rows_attr}#{class_attr} name="#{escape_html(name_for(prefix, field))}" id="#{escape_html(id_for(prefix, field))}">\n#{body}</textarea>)
  end

  def fb_submit(%Roundhouse.FormBuilder{prefix: prefix, is_persisted: is_persisted}, opts \\ %{}) do
    opts = ensure_map(opts)
    cls = Map.get(opts, "class", "")
    class_attr = if cls == "", do: "", else: ~s( class="#{escape_html(cls)}")

    label =
      case Map.get(opts, "label") do
        l when is_binary(l) and l != "" -> l
        _ ->
          prefix_human = capitalize_first(prefix)
          if is_persisted, do: "Update #{prefix_human}", else: "Create #{prefix_human}"
      end

    esc = escape_html(label)
    ~s(<input type="submit" name="commit" value="#{esc}"#{class_attr} data-disable-with="#{esc}" />)
  end

  # ── Turbo / misc ────────────────────────────────────────────

  def turbo_stream_from(channel) do
    encoded = :base64.encode(Jason.encode!(channel))
    ~s(<turbo-cable-stream-source channel="Turbo::StreamsChannel" signed-stream-name="#{encoded}--unsigned"></turbo-cable-stream-source>)
  end

  def dom_id(singular, id, prefix \\ "")
  def dom_id(singular, id, "") when is_binary(singular), do: "#{singular}_#{id}"
  def dom_id(singular, id, prefix) when is_binary(singular) and is_binary(prefix), do: "#{prefix}_#{singular}_#{id}"

  def pluralize(count, word) do
    if count == 1, do: "1 #{word}", else: "#{count} #{word}s"
  end

  def truncate(text, opts \\ %{}) do
    opts = ensure_map(opts)
    length = Map.get(opts, "length", "30") |> to_int(30)
    omission = Map.get(opts, "omission", "...")

    if String.length(text) <= length do
      text
    else
      cut = max(length - String.length(omission), 0)
      String.slice(text, 0, cut) <> omission
    end
  end

  def field_has_error(errors, field) when is_list(errors) do
    Enum.any?(errors, fn e ->
      case e do
        %{field: f} -> to_string(f) == to_string(field)
        {f, _} -> to_string(f) == to_string(field)
        _ -> false
      end
    end)
  end

  def field_has_error(_, _), do: false

  def error_messages_for(_errors, _noun), do: ""

  def content_for(slot, body \\ nil)
  def content_for(slot, nil), do: content_for_get(slot)
  def content_for(slot, body), do: content_for_set(slot, body)

  # ── helpers ─────────────────────────────────────────────────

  defp sorted_attrs(opts) do
    opts = ensure_map(opts)

    opts
    |> Enum.sort_by(&elem(&1, 0))
    |> Enum.map_join("", fn {k, v} -> ~s( #{escape_html(k)}="#{escape_html(v)}") end)
  end

  defp ensure_map(opts) when is_map(opts), do: opts

  defp ensure_map(opts) when is_list(opts) do
    Map.new(opts, fn
      {k, v} -> {to_string(k), v}
      other -> {to_string(other), ""}
    end)
  end

  defp ensure_map(_), do: %{}

  defp name_for("", field), do: to_string(field)
  defp name_for(prefix, field), do: "#{prefix}[#{field}]"

  defp id_for("", field), do: to_string(field)
  defp id_for(prefix, field), do: "#{prefix}_#{field}"

  defp capitalize_first(""), do: ""

  defp capitalize_first(s) when is_binary(s) do
    {first, rest} = String.split_at(s, 1)
    String.upcase(first) <> rest
  end

  defp capitalize_first(s), do: capitalize_first(to_string(s))

  defp to_int(v, default) do
    case v do
      i when is_integer(i) ->
        i

      s when is_binary(s) ->
        case Integer.parse(s) do
          {n, _} -> n
          _ -> default
        end

      _ ->
        default
    end
  end

  defp escape_html(nil), do: ""
  defp escape_html(v) when not is_binary(v), do: escape_html(to_string(v))

  defp escape_html(s) do
    s
    |> String.replace("&", "&amp;")
    |> String.replace("<", "&lt;")
    |> String.replace(">", "&gt;")
    |> String.replace("\"", "&quot;")
    |> String.replace("'", "&#39;")
  end
end

