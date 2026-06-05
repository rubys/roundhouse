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
# .table/0`, then `Dispatch.call/5` — but skips Plug.Conn and
# returns a `TestResponse` directly. Assertion semantics: substring-
# match on the response body, loose but good-enough for the blog's HTML.

defmodule TestResponse do
  @moduledoc """
  Wrapper around the `{body, status, content_type, location}` tuple
  `Dispatch.call/5` returns, exposing Rails-Minitest-compatible
  assertion helpers. Bodies substring-match for `assert_select`-style
  queries.
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
    fragment = selector_fragment(selector)

    unless String.contains?(body, fragment) do
      raise ExUnit.AssertionError,
        message:
          "expected body to match selector #{inspect(selector)} (looked for #{inspect(fragment)})"
    end
  end

  def assert_select_text(%TestResponse{body: body} = resp, selector, text) do
    assert_select(resp, selector)

    unless String.contains?(body, to_string(text)) do
      raise ExUnit.AssertionError,
        message:
          "expected body to contain text #{inspect(text)} under selector #{inspect(selector)}"
    end
  end

  def assert_select_min(%TestResponse{body: body}, selector, n) do
    fragment = selector_fragment(selector)
    count = count_occurrences(body, fragment)

    unless count >= n do
      raise ExUnit.AssertionError,
        message:
          "expected at least #{n} matches for selector #{inspect(selector)}, got #{count}"
    end
  end

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

defmodule TestClient do
  @moduledoc """
  Pure in-process HTTP client — dispatches through the same v2 stack
  as `Server` (`ActionDispatch.Router.match/3` over
  `RoutesTable.table/0` → `Dispatch.call/5`). No real HTTP, no
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
        result = Dispatch.call(mr.controller, mr.action, path_params, body_params, format)
        TestResponse.from(normalize_status(result))
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
