# Roundhouse Elixir test-support runtime.
#
# Hand-written, shipped alongside generated code (copied in by the
# Elixir emitter as `test/support/test_support.ex`). Controller tests
# call into `TestClient` for pure in-process HTTP dispatch (no real
# server, no socket setup) and wrap responses in `TestResponse` for
# Rails-compatible assertions.
#
# `TestClient.dispatch/3` mirrors `Server.dispatch/1`'s routing
# path — `ActionDispatch.Router.match/3` over `RoutesTable
# .table/0`, then `Dispatch.call/6` — but skips Plug.Conn and
# returns a `TestResponse` directly. Assertion semantics: substring-
# match on the response body, loose but good-enough for the blog's HTML.

defmodule Dom do
  @moduledoc """
  Dom primitive surface — the HTML-query contract `assert_select`
  lowers to (shared in shape with the Ruby/TS/Python/Rust twins; see
  the cross-target contract in runtime/spinel/test/test_helper.rbs).

  Stub: the substring matcher dressed as a Dom. `select/2` returns one
  synthetic node — the whole document — per fragment occurrence, and
  `text/1` returns it verbatim, so presence / minimum / text checks
  degrade to exactly the pre-contract behavior. A later phase swaps
  these three functions for a `Floki`-backed engine — real nodes, real
  CSS selectors — touching only this module; the `TestResponse` call
  sites and every other target stay put.
  """

  # Parse an HTML document. Stub: the document *is* its html string.
  def parse(html), do: html || ""

  # Nodes matching `selector` within `root` (a document or node). Stub:
  # one synthetic node (the root's html) per substring-fragment
  # occurrence.
  def select(root, selector) do
    List.duplicate(root, count_occurrences(root, selector_fragment(selector)))
  end

  # Concatenated descendant text of a node. Stub: the node verbatim.
  def text(node), do: node

  defp count_occurrences(body, fragment) do
    body
    |> String.split(fragment)
    |> length()
    |> Kernel.-(1)
  end

  defp selector_fragment(selector) do
    first = selector |> String.split(~r/\s+/) |> List.first() || ""

    cond do
      String.starts_with?(first, "#") -> "id=\"" <> String.slice(first, 1..-1//1) <> "\""
      String.starts_with?(first, ".") -> String.slice(first, 1..-1//1) <> "\""
      true -> "<" <> first
    end
  end
end

defmodule TestResponse do
  @moduledoc """
  Wrapper around the `{body, status, content_type, location}` tuple
  `Dispatch.call/6` returns, exposing Rails-Minitest-compatible
  assertion helpers. Bodies are queried via the `Dom` surface above.
  """

  defstruct body: "", status: 200, location: ""

  def from({body, status, _content_type, location}) do
    %TestResponse{body: body || "", status: status || 200, location: location || ""}
  end

  def assert_ok(%TestResponse{status: 200}), do: :ok

  def assert_ok(%TestResponse{status: s}) do
    raise ExUnit.AssertionError, message: "expected 200 OK, got #{s}"
  end

  def assert_unprocessable(%TestResponse{status: 422}), do: :ok

  def assert_unprocessable(%TestResponse{status: s}) do
    raise ExUnit.AssertionError, message: "expected 422 Unprocessable Entity, got #{s}"
  end

  def assert_status(%TestResponse{status: s}, s), do: :ok

  def assert_status(%TestResponse{status: actual}, expected) do
    raise ExUnit.AssertionError, message: "expected status #{expected}, got #{actual}"
  end

  def assert_redirected_to(%TestResponse{status: s, location: loc}, path) do
    unless s >= 300 and s < 400 do
      raise ExUnit.AssertionError, message: "expected a redirection, got #{s}"
    end

    unless String.contains?(loc || "", path) do
      raise ExUnit.AssertionError,
        message: "expected Location to contain #{inspect(path)}, got #{inspect(loc)}"
    end
  end

  def assert_select(%TestResponse{body: body}, selector) do
    if Dom.select(Dom.parse(body), selector) == [] do
      raise ExUnit.AssertionError,
        message: "expected body to match selector #{inspect(selector)}"
    end
  end

  def assert_select_text(%TestResponse{body: body} = resp, selector, text) do
    assert_select(resp, selector)
    nodes = Dom.select(Dom.parse(body), selector)

    unless Enum.any?(nodes, fn n -> String.contains?(Dom.text(n), to_string(text)) end) do
      raise ExUnit.AssertionError,
        message:
          "expected text #{inspect(text)} under selector #{inspect(selector)}"
    end
  end

  def assert_select_min(%TestResponse{body: body}, selector, n) do
    count = length(Dom.select(Dom.parse(body), selector))

    unless count >= n do
      raise ExUnit.AssertionError,
        message:
          "expected at least #{n} matches for selector #{inspect(selector)}, got #{count}"
    end
  end
end

defmodule TestClient do
  @moduledoc """
  Pure in-process HTTP client — dispatches through the same v2 stack
  as `Server` (`ActionDispatch.Router.match/3` over
  `RoutesTable.table/0` → `Dispatch.call/6`). No real HTTP, no
  socket setup. Flat bracket-notation body keys (`"article[title]"`)
  are nested to the shape the v2 controllers read, matching the
  server's `read_form_body` path.
  """

  def get(path), do: dispatch("GET", path, %{})
  def post(path, body \\ %{}), do: dispatch("POST", path, body)
  def patch(path, body \\ %{}), do: dispatch("PATCH", path, body)
  def put(path, body \\ %{}), do: dispatch("PUT", path, body)
  def delete(path), do: dispatch("DELETE", path, %{})

  defp dispatch(method, raw_path, body) do
    {path, format} =
      if String.ends_with?(raw_path, ".json") do
        {String.replace_suffix(raw_path, ".json", ""), :json}
      else
        {raw_path, :html}
      end

    case ActionDispatch.Router.match(method, path, RoutesTable.table()) do
      nil ->
        raise ExUnit.AssertionError, message: "no route for #{method} #{path}"

      mr ->
        path_params = stringify_keys(mr.path_params)
        body_params = nest_params(stringify_keys(body))
        # Dispatch.call/6 returns a 5-tuple now (the trailing element is the
        # flash to persist to the rh_flash cookie); tests carry no incoming
        # flash (`%{}`) and assert on the response, not the cookie, so drop
        # the carried flash and keep the legacy 4-tuple downstream.
        {body, status, ct, loc, _flash} =
          Dispatch.call(mr.controller, mr.action, path_params, body_params, format, %{})

        TestResponse.from(normalize_status({body, status, ct, loc}))
    end
  end

  # A 0 status from the dispatch tuple means "unset" → 200, matching
  # Server.dispatch/1.
  defp normalize_status({body, 0, ct, loc}), do: {body, 200, ct, loc}
  defp normalize_status(other), do: other

  defp stringify_keys(map) when is_map(map) do
    Map.new(map, fn {k, v} -> {to_string(k), v} end)
  end

  # Flat `"article[title]" => v` → nested `%{"article" => %{"title" => v}}`,
  # the shape the v2 controllers read (`<Resource>Params.from_raw`).
  # Mirrors Server.nest_params/1.
  defp nest_params(flat) do
    Enum.reduce(flat, %{}, fn {k, v}, acc ->
      case Regex.run(~r/^([^\[]+)\[([^\]]+)\]$/, k) do
        [_, outer, inner] ->
          sub = Map.get(acc, outer, %{})
          Map.put(acc, outer, Map.put(sub, inner, v))

        _ ->
          Map.put(acc, k, v)
      end
    end)
  end
end
