# Roundhouse Elixir server runtime.
#
# Hand-written, shipped alongside generated code (copied in by the
# Elixir emitter as `lib/roundhouse/server.ex`). Runs Plug.Cowboy
# for HTTP dispatch; `/cable` registered separately as a WebSocket
# stub (full cable parity is a later pass).
#
# Mirrors runtime/rust/server.rs + runtime/go/server.go in intent:
# `Roundhouse.Server.start/2` opens the DB, applies schema, layers
# the dispatcher on top of Router.match, wraps HTML responses in
# the emitted layout when one is configured.

defmodule Roundhouse.Server do
  require Logger

  alias Roundhouse.Db
  alias Roundhouse.Http
  alias Roundhouse.ViewHelpers
  alias Roundhouse.Cable

  @doc """
  Open the DB, apply schema, and run Plug.Cowboy until the process
  exits. Blocks.
  """
  def start(schema_sql, opts \\ []) do
    db_path = Keyword.get(opts, :db_path, "storage/development.sqlite3")
    port = Keyword.get(opts, :port, resolve_port())
    layout = Keyword.get(opts, :layout, nil)

    Db.open_production_db(db_path, schema_sql)
    Cable.ensure_started()
    :persistent_term.put({__MODULE__, :layout}, layout)

    {:ok, _} = Plug.Cowboy.http(__MODULE__.Endpoint, [], port: port, ip: {127, 0, 0, 1})
    Logger.info("Roundhouse Elixir server listening on http://127.0.0.1:#{port}")
    # Block forever — Plug.Cowboy runs under the kernel supervisor;
    # the script driver holds the process open via `receive`.
    Process.sleep(:infinity)
  end

  defp resolve_port do
    case System.get_env("PORT") do
      nil -> 3000
      s -> String.to_integer(s)
    end
  end

  @doc """
  Core dispatcher: read body, do `_method` override, look up the
  route via `Roundhouse.Http.Router.match/2`, invoke the handler,
  wrap HTML in the layout. Called from the Plug endpoint.
  """
  def dispatch(conn) do
    ViewHelpers.reset_render_state()

    method = conn.method |> String.upcase()
    path = "/" <> Enum.join(conn.path_info, "/")

    if path == "/cable" do
      Cable.handle(conn)
    else
      {conn, body_params} = read_form_body(conn)

      method =
        if method == "POST" and Map.has_key?(body_params, "_method") do
          upper = body_params["_method"] |> String.upcase()
          if upper in ["PATCH", "PUT", "DELETE"], do: upper, else: method
        else
          method
        end

      case Http.Router.match(method, path) do
        nil ->
          conn
          |> Plug.Conn.put_resp_content_type("text/plain")
          |> Plug.Conn.send_resp(404, "Not Found")

        {controller, action, path_params} ->
          params = Map.merge(stringify_keys(path_params), body_params)
          ctx = %Http.ActionContext{params: params}
          resp = apply(controller, action, [ctx])
          status = if resp.status == 0, do: 200, else: resp.status
          render(conn, status, resp, get_layout())
      end
    end
  end

  defp get_layout do
    :persistent_term.get({__MODULE__, :layout}, nil)
  end

  defp render(conn, status, resp, layout) do
    cond do
      status in 300..399 and resp.location != "" ->
        conn
        |> Plug.Conn.put_resp_header("location", resp.location)
        |> Plug.Conn.put_resp_content_type("text/html; charset=utf-8")
        |> Plug.Conn.send_resp(status, resp.body)

      layout != nil ->
        ViewHelpers.set_yield(resp.body)
        body = layout.()

        conn
        |> Plug.Conn.put_resp_content_type("text/html; charset=utf-8")
        |> Plug.Conn.send_resp(status, body)

      true ->
        conn
        |> Plug.Conn.put_resp_content_type("text/html; charset=utf-8")
        |> Plug.Conn.send_resp(status, resp.body)
    end
  end

  defp read_form_body(conn) do
    ct = Plug.Conn.get_req_header(conn, "content-type") |> List.first() || ""

    if String.starts_with?(ct, "application/x-www-form-urlencoded") do
      # Read + decode by hand rather than via Plug.Parsers — Plug
      # expands `article[title]=foo` into a nested
      # `%{"article" => %{"title" => "foo"}}` map, but the emitted
      # controllers (parallel to Rust/Python) read literal flat
      # keys like `"article[title]"`. URI.decode_query gives us
      # the flat shape directly.
      {:ok, body, conn} = Plug.Conn.read_body(conn, length: 8_000_000)
      {conn, URI.decode_query(body)}
    else
      {conn, %{}}
    end
  end

  defp stringify_keys(map) when is_map(map) do
    Map.new(map, fn {k, v} -> {to_string(k), v} end)
  end

  defmodule Endpoint do
    @moduledoc """
    Plug endpoint — defers everything to `Roundhouse.Server.dispatch/1`.
    Split into its own module so Plug.Cowboy's `plug: Endpoint`
    convention works without the dispatcher fn living in the top
    module's init/call.
    """
    @behaviour Plug

    @impl true
    def init(opts), do: opts

    @impl true
    def call(conn, _opts) do
      Roundhouse.Server.dispatch(conn)
    end
  end
end
