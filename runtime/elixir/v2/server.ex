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
    Cable.start_registry()

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
  def dispatch(%{path_info: ["cable"]} = conn) do
    # Action Cable WebSocket. Echo the `actioncable-v1-json` subprotocol the
    # `@rails/actioncable` client requires (it closes the socket otherwise),
    # then hand the connection to the Cowboy WebSocket handler via
    # `upgrade_adapter` — stays inside the Plug pipeline, no custom Cowboy
    # dispatch. CableHandler runs the actioncable-v1-json flow from there.
    conn = Plug.Conn.put_resp_header(conn, "sec-websocket-protocol", "actioncable-v1-json")
    WebSockAdapter.upgrade(conn, CableHandler, %{channels: []}, [])
  end

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
    Plug endpoint. Serves compiled assets (tailwind.css, turbo.min.js, the
    importmap JS) from `static/assets/` at `/assets/*` — the URLs the emitted
    layout's `stylesheet_link_tag` / importmap reference — then defers
    everything else to `Server.dispatch/1`. `Plug.Static` passes through
    (doesn't halt) on a miss, so a non-asset path falls to the dispatcher.
    """
    use Plug.Builder

    plug Plug.Static, at: "/assets", from: "static/assets", gzip: false
    plug :dispatch

    def dispatch(conn, _opts), do: Server.dispatch(conn)
  end
end

# ── Action Cable WebSocket + Turbo Streams broadcaster ───────────────
#
# Per-target transport primitive (cf. runtime/{go/v2,crystal,rust}/cable +
# the ts CableServer). Subscribers are held in an Elixir `Registry` keyed by
# channel; each /cable connection is its own process that registers under the
# decoded stream name and receives `{:cable_msg, …}` on every broadcast. Same
# wire format (actioncable-v1-json) and per-channel fan-out as the other
# targets. `Broadcasts.record` calls `Cable.dispatch`.
defmodule Cable do
  @registry Cable.Registry

  @doc "Start the subscriber registry (idempotent; called from Server.start)."
  def start_registry do
    case Registry.start_link(keys: :duplicate, name: @registry) do
      {:ok, _} -> :ok
      {:error, {:already_started, _}} -> :ok
    end
  end

  @doc "Register the calling (WebSocket) process as a subscriber of `channel`."
  def subscribe(channel, identifier) do
    Registry.register(@registry, channel, identifier)
  end

  @doc """
  Fan `html` out to every subscriber of `channel`, wrapped in the Action
  Cable message envelope (the subscribe `identifier` is echoed so Turbo
  routes the frame to the right stream-source). Registry auto-drops a
  subscriber when its process dies, so no explicit unsubscribe is needed.
  """
  def dispatch(channel, html) do
    # No-op when the registry isn't started (test runs / CLI invocations that
    # never boot the server but still exercise model callbacks via Broadcasts).
    if Process.whereis(@registry) do
      Registry.dispatch(@registry, channel, fn entries ->
        for {pid, identifier} <- entries, do: send(pid, {:cable_msg, identifier, html})
      end)
    end

    :ok
  end

  def turbo_stream_html(action, target, content) do
    if content == "" do
      ~s(<turbo-stream action="#{action}" target="#{target}"></turbo-stream>)
    else
      ~s(<turbo-stream action="#{action}" target="#{target}"><template>#{content}</template></turbo-stream>)
    end
  end

  @doc """
  Recover the channel name from Turbo's signed_stream_name. The identifier is
  `{"channel":"Turbo::StreamsChannel","signed_stream_name":"<b64>--<digest>"}`;
  the base64 prefix decodes to a JSON-encoded stream name (the same string a
  broadcast's `stream` carries). Returns nil on malformed input.
  """
  def decode_channel(identifier) do
    with {:ok, %{"signed_stream_name" => signed}} <- Jason.decode(identifier),
         [b64 | _] <- String.split(signed, "--"),
         {:ok, decoded} <- Base.decode64(b64),
         {:ok, channel} when is_binary(channel) <- Jason.decode(decoded) do
      channel
    else
      _ -> nil
    end
  end
end

# WebSock handler for /cable, reached via `Plug.Conn.upgrade_adapter(conn,
# :websocket, {CableHandler, state, opts})`. Plug.Cowboy bridges the WebSock
# spec to Cowboy via the bundled websock_adapter. Sends the welcome frame,
# pings every 3s (ActionCable treats a ~6s gap as a dead connection), confirms
# subscribe commands, and pushes broadcasts forwarded by Cable.dispatch as
# `{:cable_msg, …}` messages to this process.
defmodule CableHandler do
  @behaviour WebSock

  @impl true
  def init(state) do
    # WebSock `init/1` can't push frames, so queue a self-message to send the
    # Action Cable welcome (the client waits for it before subscribing) and
    # kick off the ping heartbeat.
    send(self(), :welcome)
    {:ok, state}
  end

  @impl true
  def handle_in({msg, [opcode: :text]}, state) do
    with {:ok, %{"command" => "subscribe", "identifier" => identifier}} <- Jason.decode(msg),
         channel when is_binary(channel) <- Cable.decode_channel(identifier) do
      Cable.subscribe(channel, identifier)
      confirm = Jason.encode!(%{type: "confirm_subscription", identifier: identifier})
      {:push, {:text, confirm}, state}
    else
      _ -> {:ok, state}
    end
  end

  def handle_in(_frame, state), do: {:ok, state}

  @impl true
  def handle_info(:welcome, state) do
    Process.send_after(self(), :ping, 3000)
    {:push, {:text, Jason.encode!(%{type: "welcome"})}, state}
  end

  def handle_info(:ping, state) do
    Process.send_after(self(), :ping, 3000)
    {:push, {:text, Jason.encode!(%{type: "ping", message: System.system_time(:second)})}, state}
  end

  def handle_info({:cable_msg, identifier, html}, state) do
    {:push, {:text, Jason.encode!(%{type: "message", identifier: identifier, message: html})}, state}
  end

  def handle_info(_info, state), do: {:ok, state}

  @impl true
  def terminate(_reason, _state), do: :ok
end
