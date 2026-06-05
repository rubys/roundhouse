# Roundhouse Elixir server runtime.
#
# Hand-written, shipped alongside generated code (copied in by the
# Elixir emitter as `lib/server.ex`). Runs Plug.Cowboy and dispatches
# through the app stack — `ActionDispatch.Router.match/3` over
# `RoutesTable.table/0`, then `Dispatch.call/5` into the per-controller
# `process_action`.
#
# The DB connection + schema are owned by the shared `Roundhouse.Db` /
# `Roundhouse.SchemaSQL` target runtime (which `Db` reuses via
# `Roundhouse.Db.conn()`), so `start/2` opens the DB through it.

defmodule Server do
  require Logger

  alias Roundhouse.Db

  @doc """
  Open the DB, apply schema, and run Plug.Cowboy until the process
  exits. Blocks.
  """
  def start(schema_sql, opts \\ []) do
    db_path = Keyword.get(opts, :db_path, "storage/development.sqlite3")
    port = Keyword.get(opts, :port, resolve_port())

    Db.open_production_db(db_path, schema_sql)

    {:ok, _} = Plug.Cowboy.http(__MODULE__.Endpoint, [], port: port, ip: {127, 0, 0, 1})
    Logger.info("Roundhouse Elixir (v2) server listening on http://127.0.0.1:#{port}")
    Process.sleep(:infinity)
  end

  defp resolve_port do
    case System.get_env("PORT") do
      nil -> 3000
      s -> String.to_integer(s)
    end
  end

  @doc """
  Core dispatcher: read body, apply the `_method` override, strip a
  `.json` suffix (→ `:json` format), look up the route via the v2
  router over `RoutesTable.table/0`, run the action through
  `Dispatch.call/5`, and ship the response.
  """
  def dispatch(conn) do
    raw_method = conn.method |> String.upcase()
    raw_path = "/" <> Enum.join(conn.path_info, "/")

    {conn, body_params} = read_form_body(conn)

    method =
      if raw_method == "POST" and Map.has_key?(body_params, "_method") do
        upper = body_params["_method"] |> String.upcase()
        if upper in ["PATCH", "PUT", "DELETE"], do: upper, else: raw_method
      else
        raw_method
      end

    {path, format} =
      if String.ends_with?(raw_path, ".json") do
        {String.replace_suffix(raw_path, ".json", ""), :json}
      else
        {raw_path, :html}
      end

    case ActionDispatch.Router.match(method, path, RoutesTable.table()) do
      nil ->
        conn
        |> Plug.Conn.put_resp_content_type("text/plain")
        |> Plug.Conn.send_resp(404, "Not Found")

      mr ->
        path_params = stringify_keys(mr.path_params)
        {body, status, content_type, location} =
          Dispatch.call(mr.controller, mr.action, path_params, body_params, format)

        status = if status == 0, do: 200, else: status
        send_response(conn, status, body, content_type, location)
    end
  end

  defp send_response(conn, status, body, content_type, location) do
    conn =
      if is_binary(location) and location != "" do
        Plug.Conn.put_resp_header(conn, "location", location)
      else
        conn
      end

    ct = if is_binary(content_type) and content_type != "", do: content_type, else: "text/html; charset=utf-8"

    conn
    |> Plug.Conn.put_resp_content_type(ct)
    |> Plug.Conn.send_resp(status, body)
  end

  # Read + decode the form body by hand (not via Plug.Parsers), then
  # NEST bracket-notation keys: `article[title]=x` → `%{"article" =>
  # %{"title" => "x"}}`. The v2 controllers read nested params (via
  # `<Resource>Params.from_raw`, which does `params["article"]["title"]`),
  # the go2/Rust convention — distinct from v1's flat-key shape.
  defp read_form_body(conn) do
    ct = Plug.Conn.get_req_header(conn, "content-type") |> List.first() || ""

    if String.starts_with?(ct, "application/x-www-form-urlencoded") do
      {:ok, body, conn} = Plug.Conn.read_body(conn, length: 8_000_000)
      {conn, nest_params(URI.decode_query(body))}
    else
      {conn, %{}}
    end
  end

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

  defp stringify_keys(map) when is_map(map) do
    Map.new(map, fn {k, v} -> {to_string(k), v} end)
  end

  defmodule Endpoint do
    @moduledoc """
    Plug endpoint — defers everything to `Server.dispatch/1`.
    """
    @behaviour Plug

    @impl true
    def init(opts), do: opts

    @impl true
    def call(conn, _opts) do
      Server.dispatch(conn)
    end
  end
end
