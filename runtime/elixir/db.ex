# Roundhouse Elixir DB runtime.
#
# Hand-written helpers the Elixir emitter copies verbatim into each
# generated project as `lib/roundhouse/db.ex`. Owns the per-test
# SQLite connection and hides `exqlite`'s low-level NIF API from the
# generated save/destroy/count/find functions.
#
# Stored in the test process's dictionary (`Process.put/2`) — ExUnit
# runs each test in its own process, so no cross-test bleed. When
# `async: true` tests run concurrently, each gets its own connection
# naturally.

defmodule Roundhouse.Db do
  @moduledoc false

  @doc """
  Open a fresh `:memory:` SQLite database, run the schema DDL, and
  install the connection in the test process's dict. `schema_sql`
  may contain multiple statements separated by `;\\n` — we split and
  dispatch each non-empty chunk.
  """
  @spec setup_test_db(String.t()) :: :ok
  def setup_test_db(schema_sql) do
    if prev = Process.get(:roundhouse_conn) do
      Exqlite.Sqlite3.close(prev)
    end

    {:ok, conn} = Exqlite.Sqlite3.open(":memory:")

    schema_sql
    |> String.split(";\n")
    |> Enum.each(fn stmt ->
      stmt = String.trim(stmt)
      unless stmt == "" do
        :ok = Exqlite.Sqlite3.execute(conn, stmt)
      end
    end)

    Process.put(:roundhouse_conn, conn)
    :ok
  end

  @doc "Borrow the current test's connection. Raises if unset."
  @spec conn() :: Exqlite.Sqlite3.db()
  def conn do
    Process.get(:roundhouse_conn) ||
      raise "test db not initialized; call Roundhouse.Db.setup_test_db/1 first"
  end

  @doc """
  Run a mutating statement (INSERT / UPDATE / DELETE). Returns the
  rowid of the last insert on the connection — useful for INSERTs;
  meaningless for the other operations (caller just ignores it).
  """
  @spec execute(String.t(), list()) :: integer()
  def execute(sql, params \\ []) do
    conn = conn()
    {:ok, stmt} = Exqlite.Sqlite3.prepare(conn, sql)
    :ok = Exqlite.Sqlite3.bind(stmt, params)
    :done = Exqlite.Sqlite3.step(conn, stmt)
    id = Exqlite.Sqlite3.last_insert_rowid(conn)
    :ok = Exqlite.Sqlite3.release(conn, stmt)
    id
  end

  @doc "Run a single-row SELECT; returns the row list or nil."
  @spec query_one(String.t(), list()) :: list() | nil
  def query_one(sql, params \\ []) do
    conn = conn()
    {:ok, stmt} = Exqlite.Sqlite3.prepare(conn, sql)
    :ok = Exqlite.Sqlite3.bind(stmt, params)

    result =
      case Exqlite.Sqlite3.step(conn, stmt) do
        {:row, row} -> row
        :done -> nil
      end

    :ok = Exqlite.Sqlite3.release(conn, stmt)
    result
  end

  @doc "Run a multi-row SELECT; returns a list of row lists."
  @spec query_all(String.t(), list()) :: [list()]
  def query_all(sql, params \\ []) do
    conn = conn()
    {:ok, stmt} = Exqlite.Sqlite3.prepare(conn, sql)
    :ok = Exqlite.Sqlite3.bind(stmt, params)
    rows = drain(conn, stmt, [])
    :ok = Exqlite.Sqlite3.release(conn, stmt)
    rows
  end

  defp drain(conn, stmt, acc) do
    case Exqlite.Sqlite3.step(conn, stmt) do
      {:row, row} -> drain(conn, stmt, [row | acc])
      :done -> Enum.reverse(acc)
    end
  end

  @doc "Scalar query — first column of single row."
  @spec scalar(String.t(), list()) :: any()
  def scalar(sql, params \\ []) do
    [val] = query_one(sql, params)
    val
  end

  @doc """
  Last rowid assigned by the most recent INSERT on the current
  connection. Called from fixture loaders and `.create` rewrites so
  tests can reach the autoincrement id their save just produced.
  """
  @spec last_insert_rowid() :: integer()
  def last_insert_rowid do
    Exqlite.Sqlite3.last_insert_rowid(conn())
  end
end
