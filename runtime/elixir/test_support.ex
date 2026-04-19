# Roundhouse Elixir test-support runtime.
#
# Hand-written, shipped alongside generated code (copied in by the
# Elixir emitter as `lib/roundhouse/test_support.ex`). Controller
# tests call into `TestClient` for pure in-process HTTP dispatch
# (no real server, no socket setup) and wrap responses in
# `TestResponse` for Rails-compatible assertions.
#
# Mirrors runtime/python/test_support.py in intent, shape, and
# assertion semantics — substring-match on the response body,
# loose but good-enough for the scaffold blog's HTML.

defmodule Roundhouse.TestResponse do
  @moduledoc """
  Wrapper around `ActionResponse` exposing Rails-Minitest-
  compatible assertion helpers. Method names mirror the Ruby
  source; bodies substring-match for `assert_select`-style queries.
  """

  defstruct body: "", status: 200, location: ""

  alias Roundhouse.TestResponse

  def from(%Roundhouse.Http.ActionResponse{body: b, status: s, location: l}) do
    %TestResponse{body: b || "", status: s || 200, location: l || ""}
  end

  def from(_other), do: %TestResponse{}

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

defmodule Roundhouse.TestClient do
  @moduledoc """
  Pure in-process HTTP client — dispatches through
  `Roundhouse.Http.Router.match/2` directly. No real HTTP, no
  socket setup.
  """

  alias Roundhouse.Http.ActionContext
  alias Roundhouse.Http.Router
  alias Roundhouse.TestResponse

  def get(path), do: dispatch("GET", path, %{})
  def post(path, body \\ %{}), do: dispatch("POST", path, body)
  def patch(path, body \\ %{}), do: dispatch("PATCH", path, body)
  def put(path, body \\ %{}), do: dispatch("PUT", path, body)
  def delete(path), do: dispatch("DELETE", path, %{})

  defp dispatch(method, path, body) do
    case Router.match(method, path) do
      nil ->
        raise ExUnit.AssertionError, message: "no route for #{method} #{path}"

      {controller, action, path_params} ->
        params = Map.merge(path_params, body)
        context = %ActionContext{params: params}
        result = apply(controller, action, [context])
        TestResponse.from(result)
    end
  end
end
