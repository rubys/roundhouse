# Roundhouse Elixir HTTP runtime — Phase 4d pass-2 shape.
#
# Hand-written, shipped alongside generated code (copied in by the
# Elixir emitter as `lib/roundhouse/http.ex`). Provides the
# controller-facing types + the Router match table; the test-support
# module calls Router.match/2 to dispatch.
#
# Mirrors runtime/python/http.py (the reference dynamic twin) in
# intent and shape. ActionResponse/ActionContext are structs; Router
# state lives in an ETS table so generated `app/routes.ex` can
# register handlers at module-load time while tests dispatch
# through `Router.match/2` in a shared table.

defmodule Roundhouse.Http.ActionResponse do
  @moduledoc """
  Every generated controller action returns one of these. Fields
  are optional so actions pick only what they need:
    body: HTML string for GET actions
    status: HTTP status code (default 200)
    location: redirect target URL (for 3xx responses)
  """
  defstruct body: "", status: 200, location: ""
end

defmodule Roundhouse.Http.ActionContext do
  @moduledoc """
  Request context passed to every action. `params` merges path
  params (from the URL pattern) with form body fields.
  """
  defstruct params: %{}
end

defmodule Roundhouse.Http.Router do
  @moduledoc """
  Process-wide route table. Generated `app/routes.ex` calls
  `Router.resources/3` / `Router.root/3` / `Router.get/4` etc. at
  module-load time; `TestClient` in `test_support.ex` dispatches
  through `Router.match/2`.

  Backed by an ETS table (`:roundhouse_routes`) so writes during
  route registration and reads during test dispatch both see a
  shared table. The first call to any mutator ensures the table
  exists — idempotent across reruns.
  """

  @table :roundhouse_routes

  def reset do
    ensure_table()
    :ets.delete_all_objects(@table)
    :ok
  end

  def root(controller, action) do
    register("GET", "/", controller, action)
  end

  def root(path, controller, action) do
    register("GET", path, controller, action)
  end

  def resources(name, controller, opts \\ []) do
    only = Keyword.get(opts, :only)
    except = Keyword.get(opts, :except, [])
    nested = Keyword.get(opts, :nested, [])
    add_resource_routes(name, controller, only, except, nil)

    if nested != [] do
      parent_singular = singularize(name)

      Enum.each(nested, fn n ->
        n_name = Keyword.fetch!(n, :name)
        n_controller = Keyword.fetch!(n, :controller)
        n_only = Keyword.get(n, :only)
        n_except = Keyword.get(n, :except, [])
        add_resource_routes(n_name, n_controller, n_only, n_except, {parent_singular, name})
      end)
    end
  end

  def get(path, controller, action), do: register("GET", path, controller, action)
  def post(path, controller, action), do: register("POST", path, controller, action)
  def put(path, controller, action), do: register("PUT", path, controller, action)
  def patch(path, controller, action), do: register("PATCH", path, controller, action)
  def delete(path, controller, action), do: register("DELETE", path, controller, action)

  def match(method, path) do
    ensure_table()

    :ets.tab2list(@table)
    |> Enum.filter(&route_entry?/1)
    |> Enum.sort_by(fn {seq, _, _, _, _} -> seq end)
    |> Enum.find_value(fn {_seq, route_method, route_path, controller, action} ->
      if route_method == method do
        case try_match_path(route_path, path) do
          nil -> nil
          params -> {controller, action, params}
        end
      else
        nil
      end
    end)
  end

  defp route_entry?({_, _, _, _, _}), do: true
  defp route_entry?(_), do: false

  defp add_resource_routes(name, controller, only, except, scope) do
    standard = [
      {"index", "GET", ""},
      {"new", "GET", "/new"},
      {"create", "POST", ""},
      {"show", "GET", "/:id"},
      {"edit", "GET", "/:id/edit"},
      {"update", "PATCH", "/:id"},
      {"destroy", "DELETE", "/:id"}
    ]

    Enum.each(standard, fn {action, method, suffix} ->
      allow = if only == nil or only == [], do: true, else: action in Enum.map(only, &to_string/1)
      denied = action in Enum.map(except, &to_string/1)

      if allow and not denied do
        base =
          case scope do
            nil -> "/#{name}"
            {parent_singular, parent_plural} -> "/#{parent_plural}/:#{parent_singular}_id/#{name}"
          end

        register(method, base <> suffix, controller, String.to_atom(action))
      end
    end)
  end

  defp register(method, path, controller, action) do
    ensure_table()
    seq = :ets.update_counter(@table, :__seq__, 1, {:__seq__, 0})
    :ets.insert(@table, {seq, method, path, controller, action})
    :ok
  end

  defp ensure_table do
    case :ets.whereis(@table) do
      :undefined ->
        :ets.new(@table, [:public, :named_table, :set, {:read_concurrency, true}])

      _ ->
        :ok
    end
  end

  defp try_match_path(pattern, path) do
    pat_parts = for p <- String.split(pattern, "/"), p != "", do: p
    path_parts = for p <- String.split(path, "/"), p != "", do: p

    if length(pat_parts) == length(path_parts) do
      Enum.zip(pat_parts, path_parts)
      |> Enum.reduce_while(%{}, fn {p, v}, acc ->
        cond do
          String.starts_with?(p, ":") ->
            {:cont, Map.put(acc, String.slice(p, 1..-1//1), v)}

          p == v ->
            {:cont, acc}

          true ->
            {:halt, nil}
        end
      end)
    else
      nil
    end
  end

  defp singularize(plural) do
    cond do
      String.ends_with?(plural, "ies") -> String.slice(plural, 0..-4//1) <> "y"
      String.ends_with?(plural, "ses") -> String.slice(plural, 0..-3//1)
      String.ends_with?(plural, "s") -> String.slice(plural, 0..-2//1)
      true -> plural
    end
  end
end

defmodule Roundhouse.Http do
  @moduledoc """
  Controller-facing shortcuts used by hand-written Elixir that
  predates the pass-2 template actions. The emitter itself no
  longer generates calls to `render`, `redirect_to`, etc. — action
  bodies return `ActionResponse` structs directly.
  """

  alias Roundhouse.Http.ActionResponse

  @doc "Placeholder for bare `params` in a controller body."
  def params, do: %{}

  def render(_template), do: %ActionResponse{}
  def render(_template, _opts), do: %ActionResponse{}

  def redirect_to(_target), do: %ActionResponse{}
  def redirect_to(_target, _opts), do: %ActionResponse{}

  def head(_status), do: %ActionResponse{}

  def respond_to(fun) when is_function(fun, 1), do: fun.(%{})
end
