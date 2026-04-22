# Roundhouse Elixir cable runtime — scaffolding only.
#
# Parity with runtime/rust/cable.rs + runtime/python/cable.py is
# scoped to a later pass. For now `handle/1` returns 426 Upgrade
# Required so a curl probe sees a definitive status; a real browser
# interprets this as "cable unavailable" and moves on. Navigation
# and form-submit flows (what the compare tool exercises) don't
# depend on cable.

defmodule Roundhouse.Cable do
  @doc """
  /cable endpoint stub. Returns 426 until the full WebSocket port
  lands. Generated models with `broadcasts_to` will eventually
  call through `broadcast/2` here.
  """
  def handle(conn) do
    conn
    |> Plug.Conn.put_resp_content_type("text/plain")
    |> Plug.Conn.send_resp(426, "WebSocket upgrade not wired yet")
  end

  @doc """
  Stub broadcaster — drops the payload. Later pass fans out to
  subscribers via `Phoenix.PubSub` or equivalent.
  """
  def broadcast(_channel, _body), do: :ok
end
