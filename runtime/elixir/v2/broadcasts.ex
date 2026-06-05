# Roundhouse Elixir v2 Turbo Streams broadcasts shim — hand-written.
#
# The model lowerer's broadcasts expansion (see `src/lower/
# model_to_library/broadcasts.rs`) produces calls like
# `Broadcasts.prepend(%{stream: "x", target: "y", html: "..."})` from
# inside model after_*_commit callbacks. elixir2 emits each as
# `Broadcasts.<action>(%{…})`, so this module provides one function
# per Ruby `def self.<action>`.
#
# The log is the test-visible contract (framework Ruby's
# `runtime/spinel/test/models/article_broadcasts_test.rb` asserts on
# it); transport (WebSocket fan-out) is the live-server contract, not
# wired here — mirrors the go2/rust2 stub stance. Hand-written like the
# sibling per-target shims (runtime/{go,rust,crystal}/broadcasts.*)
# because the canonical `runtime/spinel/broadcasts.rb` relies on
# module-level constant-Array mutation that doesn't translate to
# Elixir's immutable / process model.
#
# State is held in a lazily-started, named `Agent` so the log survives
# across the calls in one request and can be asserted/reset by tests.

defmodule Broadcasts do
  @moduledoc false

  def append(attrs), do: record(:append, attrs)
  def prepend(attrs), do: record(:prepend, attrs)
  def replace(attrs), do: record(:replace, attrs)
  def remove(attrs), do: record(:remove, attrs)

  @doc "The recorded broadcasts, oldest first."
  def log do
    Agent.get(ensure_started(), &Enum.reverse/1)
  end

  @doc "Clear the recorded broadcasts (test setup)."
  def reset_log! do
    Agent.update(ensure_started(), fn _ -> [] end)
  end

  defp record(action, attrs) do
    entry = Map.put(attrs, :action, action)
    Agent.update(ensure_started(), fn log -> [entry | log] end)
    nil
  end

  # The log Agent is started on first use (no supervision tree needed
  # for the test-visible stub). `start` (not `start_link`) so a crashing
  # request process doesn't take the log down with it.
  defp ensure_started do
    case Process.whereis(__MODULE__) do
      nil ->
        case Agent.start(fn -> [] end, name: __MODULE__) do
          {:ok, pid} -> pid
          {:error, {:already_started, pid}} -> pid
        end

      pid ->
        pid
    end
  end
end
