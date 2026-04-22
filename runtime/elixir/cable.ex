# Roundhouse Elixir cable runtime.
#
# Action Cable WebSocket + Turbo Streams broadcaster. Mirrors
# runtime/rust/cable.rs + runtime/python/cable.py:
# actioncable-v1-json subprotocol, per-channel subscriber map via
# Registry, partial-renderer registry, welcome/ping/subscribe/
# confirm_subscription frames.
#
# Uses `:cowboy_websocket` (already required by Plug.Cowboy) and
# the stdlib `Registry` — no additional deps.

defmodule Roundhouse.Cable do
  @moduledoc """
  Broadcast helpers + `/cable` WebSocket handler.
  """

  @registry Roundhouse.CableRegistry
  @partial_registry Roundhouse.CablePartials

  # ── Registry bootstrap ──────────────────────────────────────

  @doc """
  Start the pub/sub + partial-renderer Registries. Called from
  `Roundhouse.Server.start/2` at process boot; idempotent so tests
  can also spin it up.
  """
  def ensure_started do
    ensure_registry(@registry, :duplicate)
    ensure_registry(@partial_registry, :unique)
  end

  defp ensure_registry(name, keys) do
    case Process.whereis(name) do
      nil -> {:ok, _pid} = Registry.start_link(keys: keys, name: name)
      _pid -> :ok
    end
  end

  # ── Partial-renderer registry ───────────────────────────────

  def register_partial(type_name, fn_ref) when is_binary(type_name) and is_function(fn_ref, 1) do
    ensure_started()
    Registry.unregister(@partial_registry, type_name)
    {:ok, _} = Registry.register(@partial_registry, type_name, fn_ref)
    :ok
  end

  def render_partial(type_name, id) do
    ensure_started()

    case Registry.lookup(@partial_registry, type_name) do
      [{_pid, fn_ref} | _] -> fn_ref.(id)
      _ -> "<div>#{type_name} ##{id}</div>"
    end
  end

  # ── Turbo Streams rendering ─────────────────────────────────

  def turbo_stream_html(action, target, "") do
    ~s(<turbo-stream action="#{action}" target="#{target}"></turbo-stream>)
  end

  def turbo_stream_html(action, target, content) do
    ~s(<turbo-stream action="#{action}" target="#{target}"><template>#{content}</template></turbo-stream>)
  end

  defp dom_id_for(table, id) do
    singular = if String.ends_with?(table, "s"), do: String.slice(table, 0, String.length(table) - 1), else: table
    "#{singular}_#{id}"
  end

  # ── Broadcast helpers ───────────────────────────────────────

  def broadcast_replace_to(table, id, type_name, channel, target) do
    t = if target == "", do: dom_id_for(table, id), else: target
    html = render_partial(type_name, id)
    dispatch(channel, turbo_stream_html("replace", t, html))
  end

  def broadcast_prepend_to(table, id, type_name, channel, target) do
    t = if target == "", do: table, else: target
    html = render_partial(type_name, id)
    dispatch(channel, turbo_stream_html("prepend", t, html))
  end

  def broadcast_append_to(table, id, type_name, channel, target) do
    t = if target == "", do: table, else: target
    html = render_partial(type_name, id)
    dispatch(channel, turbo_stream_html("append", t, html))
  end

  def broadcast_remove_to(table, id, channel, target) do
    t = if target == "", do: dom_id_for(table, id), else: target
    dispatch(channel, turbo_stream_html("remove", t, ""))
  end

  # Dispatch a rendered frame to every subscriber of `channel`.
  # Subscribers live as `{channel, identifier}` keys in the pub/sub
  # Registry, with the socket pid as owner — Registry.dispatch/3
  # sends `{:cable_frame, ...}` to each pid, which the socket's
  # websocket_info/2 serializes as an actioncable message frame.
  defp dispatch(channel, html) do
    ensure_started()

    Registry.dispatch(@registry, channel, fn subs ->
      for {pid, identifier} <- subs do
        send(pid, {:cable_frame, identifier, html})
      end
    end)
  end

  # ── /cable request handler ──────────────────────────────────

  # Called from `Roundhouse.Server.dispatch/1` for `path == "/cable"`.
  # Upgrades the connection by delegating to the CowboyWs handler
  # below. The protocol-negotiation header gets set here so Turbo's
  # client sees the expected subprotocol.
  def handle(conn) do
    conn
    |> Plug.Conn.put_resp_header("sec-websocket-protocol", "actioncable-v1-json")
    |> Plug.Conn.fetch_query_params()
    |> cowboy_upgrade()
  end

  defp cowboy_upgrade(conn) do
    # WebSockAdapter is the standard way to hand off from Plug to
    # Cowboy's websocket handler since Plug 1.14. We require it via
    # plug_cowboy's transitive dep graph; users that don't have it
    # see a clear error.
    WebSockAdapter.upgrade(conn, __MODULE__.Socket, %{}, timeout: 60_000)
  end

  defmodule Socket do
    @moduledoc false
    @behaviour WebSock

    alias Roundhouse.Cable

    @impl true
    def init(_opts) do
      Cable.ensure_started()
      schedule_ping()
      send(self(), :welcome)
      {:ok, %{subs: []}}
    end

    @impl true
    def handle_in({text, [opcode: :text]}, state) do
      case Jason.decode(text) do
        {:ok, %{"command" => "subscribe", "identifier" => identifier}} when is_binary(identifier) ->
          case Cable.decode_channel(identifier) do
            nil ->
              {:ok, state}

            channel ->
              {:ok, _} =
                Registry.register(Roundhouse.CableRegistry, channel, identifier)

              state = %{state | subs: [{channel, identifier} | state.subs]}
              frame = Jason.encode!(%{"type" => "confirm_subscription", "identifier" => identifier})
              {:push, {:text, frame}, state}
          end

        _ ->
          {:ok, state}
      end
    end

    def handle_in(_other, state), do: {:ok, state}

    @impl true
    def handle_info(:welcome, state) do
      {:push, {:text, Jason.encode!(%{"type" => "welcome"})}, state}
    end

    def handle_info(:ping, state) do
      schedule_ping()

      {:push,
       {:text, Jason.encode!(%{"type" => "ping", "message" => System.system_time(:second)})},
       state}
    end

    def handle_info({:cable_frame, identifier, html}, state) do
      frame =
        Jason.encode!(%{"type" => "message", "identifier" => identifier, "message" => html})

      {:push, {:text, frame}, state}
    end

    def handle_info(_other, state), do: {:ok, state}

    @impl true
    def terminate(_reason, state) do
      for {channel, _identifier} <- state.subs do
        Registry.unregister(Roundhouse.CableRegistry, channel)
      end

      :ok
    end

    defp schedule_ping do
      Process.send_after(self(), :ping, 3_000)
    end
  end

  @doc """
  Recover the channel name from Turbo's signed_stream_name.
  Identifier is a JSON blob with
    {"channel":"Turbo::StreamsChannel",
     "signed_stream_name":"<base64>--<digest>"}
  — the base64 prefix decodes to a JSON-encoded channel name.
  """
  def decode_channel(identifier) when is_binary(identifier) do
    with {:ok, %{"signed_stream_name" => signed}} when is_binary(signed) <- Jason.decode(identifier),
         [b64 | _] <- String.split(signed, "--", parts: 2),
         {:ok, decoded} <- Base.decode64(b64),
         {:ok, channel} when is_binary(channel) <- Jason.decode(decoded) do
      channel
    else
      _ -> nil
    end
  end

  def decode_channel(_), do: nil

  # Stub kept for back-compat with earlier server wiring.
  def broadcast(channel, body), do: dispatch(channel, body)
end
