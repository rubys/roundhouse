# Roundhouse Elixir v2 DB runtime — hand-written (no Ruby source).
#
# The portable prepared-cursor surface the lowered model emit targets:
# `Db.prepare/step?/column_int/column_text/exec/escape_string/escape_int/
# last_insert_rowid/finalize`. A thin wrapper over `Exqlite.Sqlite3`,
# reusing the shared connection owned by `Roundhouse.Db` (the legacy
# runtime emitted into the same project). Mirrors `runtime/go/v2/db.go`
# and `runtime/rust/db.rs`: the lowered IR inlines values into SQL via
# `escape_string`/`escape_int` rather than binding parameters, so
# `prepare`/`exec` take complete SQL strings.
#
# `prepare` pre-fetches every row up front and returns an opaque integer
# stmt id; `step?` advances a per-id cursor; `column_*` read the current
# row. Cursor state lives in the process dictionary, so it inherits the
# same per-process (per-ExUnit-test) isolation as `Roundhouse.Db`'s
# connection.

defmodule V2.Db do
  @moduledoc false

  # Run a SELECT, materialize every row, return an opaque stmt id.
  def prepare(sql) do
    conn = Roundhouse.Db.conn()
    {:ok, stmt} = Exqlite.Sqlite3.prepare(conn, sql)
    rows = drain(conn, stmt, [])
    :ok = Exqlite.Sqlite3.release(conn, stmt)
    id = next_id()
    put_stmt(id, %{remaining: rows, current: nil})
    id
  end

  # Run a mutating statement (INSERT / UPDATE / DELETE), recording the
  # last insert rowid for `last_insert_rowid/0`.
  def exec(sql) do
    Process.put({__MODULE__, :last_rowid}, Roundhouse.Db.execute(sql))
    :ok
  end

  # Rowid produced by the most recent `exec` INSERT.
  def last_insert_rowid do
    Process.get({__MODULE__, :last_rowid}, 0)
  end

  # Advance the cursor; true if a row is now current, false at the end.
  def step?(id) do
    case stmt(id) do
      %{remaining: [row | rest]} = entry ->
        put_stmt(id, %{entry | remaining: rest, current: row})
        true

      entry when is_map(entry) ->
        put_stmt(id, %{entry | current: nil})
        false

      _ ->
        false
    end
  end

  # Integer column of the current row (NULL / missing → 0; text/float
  # best-effort coerce, matching the go/rust shims).
  def column_int(id, i) do
    case column(id, i) do
      n when is_integer(n) -> n
      n when is_float(n) -> trunc(n)
      s when is_binary(s) ->
        case Integer.parse(s) do
          {n, _} -> n
          :error -> 0
        end

      _ -> 0
    end
  end

  # Text column of the current row (NULL / missing → ""; numerics
  # stringify).
  def column_text(id, i) do
    case column(id, i) do
      s when is_binary(s) -> s
      n when is_integer(n) -> Integer.to_string(n)
      n when is_float(n) -> Float.to_string(n)
      _ -> ""
    end
  end

  # Drop a prepared statement's cursor state.
  def finalize(id) do
    put_stmts(Map.delete(stmts(), id))
    :ok
  end

  # SQL-quote a string literal (SQLite rule: single quotes doubled). No
  # other byte transforms — values are inlined, not parameter-bound.
  def escape_string(s) do
    "'" <> String.replace(s, "'", "''") <> "'"
  end

  # Render an integer for SQL inlining.
  def escape_int(n), do: Integer.to_string(n)

  # ---- cursor state (process dict, per-process like Roundhouse.Db) ----

  defp drain(conn, stmt, acc) do
    case Exqlite.Sqlite3.step(conn, stmt) do
      {:row, row} -> drain(conn, stmt, [row | acc])
      :done -> Enum.reverse(acc)
    end
  end

  defp column(id, i) do
    case stmt(id) do
      %{current: row} when is_list(row) -> Enum.at(row, i)
      _ -> nil
    end
  end

  defp stmts, do: Process.get({__MODULE__, :stmts}, %{})
  defp put_stmts(map), do: Process.put({__MODULE__, :stmts}, map)
  defp stmt(id), do: Map.get(stmts(), id)
  defp put_stmt(id, entry), do: put_stmts(Map.put(stmts(), id, entry))

  defp next_id do
    id = Process.get({__MODULE__, :next}, 0) + 1
    Process.put({__MODULE__, :next}, id)
    id
  end
end
