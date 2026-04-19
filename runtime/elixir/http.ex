# Roundhouse Elixir HTTP runtime — Phase 4c compile-only stubs.
#
# Hand-written, shipped alongside generated code (copied in by the
# Elixir emitter as `lib/roundhouse/http.ex`). Provides just enough
# surface that emitted controller actions compile: `params`,
# `render`, `redirect_to`, `head`, and a `respond_to` DSL stub.
#
# Mirrors the Rust/Crystal/Go twins in intent, but Elixir's dynamic
# typing means there's no `Response` struct to return — every stub
# returns `:ok`. The emitter flattens `respond_to do |fmt| fmt.html
# { body } end` to just `body` directly, so `respond_to/1` below is
# kept only so that hand-written code outside the emitter still
# compiles. Phase 4c tests stay `@tag :skip`, so nothing executes
# during `mix test`; the purpose is to make `mix compile` succeed.

defmodule Roundhouse.Http do
  @moduledoc """
  Phase 4c compile-only HTTP stubs. Every function returns `:ok`.
  Real behavior lands in a later Phase 4 stage once the call-site
  shape stabilises.
  """

  @doc "Placeholder for bare `params` in a controller body."
  def params, do: %{}

  @doc "Stub `render :template` / `render template, opts`."
  def render(_template), do: :ok
  def render(_template, _opts), do: :ok

  @doc "Stub `redirect_to target` / `redirect_to target, opts`."
  def redirect_to(_target), do: :ok
  def redirect_to(_target, _opts), do: :ok

  @doc "Stub `head :status`."
  def head(_status), do: :ok

  @doc """
  `respond_to do |format| ... end` in emitted code is flattened by
  the Phase-4c emitter — the HTML branch body lands inline at the
  respond_to call site and the JSON branch becomes a `# TODO: JSON
  branch` comment. This stub exists only so hand-written Elixir that
  calls `Roundhouse.Http.respond_to/1` still compiles.
  """
  def respond_to(fun) when is_function(fun, 1), do: fun.(%{})
end
