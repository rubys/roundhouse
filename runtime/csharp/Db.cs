using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.IO;
using System.Threading;
using Microsoft.Data.Sqlite;

namespace Roundhouse;

// The sqlite primitive layer the lowered model IR dispatches against
// (`Db.Prepare` / `Db.StepPred` / `Db.ColumnInt` / `Db.ColumnText` /
// `Db.EscapeString` / `Db.EscapeInt` / `Db.Exec` / `Db.LastInsertRowid` /
// `Db.Finalize`). PascalCase to match the emitter's rendering. Prepared-
// statement handles are `long`s (the emitter renders integer literals with an
// `L` suffix). The DB path comes from `BLOG_DB` / `DATABASE_PATH` (default
// `storage/development.sqlite3`).
//
// Concurrency: reads use a BOUNDED connection pool (~cores), so the number of
// open connections — and their per-connection WAL page cache — stays bounded
// under load rather than growing one-per-Kestrel-thread (which balloons RSS).
// The pool gate also caps concurrent DB work to the pool size. Writes (rare —
// POST only) use a per-thread connection so `exec`(INSERT) and
// `last_insert_rowid()` stay on the same connection; WAL + autocommit makes a
// committed write visible to the read pool immediately.
public static class Db
{
    // Set only by `SetupTestDb`; production reads the environment.
    private static string? _testPath;

    private static string DbPath =>
        _testPath
        ?? Environment.GetEnvironmentVariable("BLOG_DB")
        ?? Environment.GetEnvironmentVariable("DATABASE_PATH")
        ?? "storage/development.sqlite3";

    // Clamp to [4, 16]: 16 concurrent readers already saturate SQLite, and an
    // unbounded ProcessorCount pool means one connection (+ its WAL page cache)
    // per core — hundreds of MB of baseline on a many-core host for no gain.
    private static readonly int PoolSize = Math.Clamp(Environment.ProcessorCount, 4, 16);
    private static readonly SemaphoreSlim Gate = new(PoolSize, PoolSize);
    private static readonly ConcurrentBag<SqliteConnection> Pool = new();
    private static readonly ConcurrentDictionary<long, (SqliteConnection conn, SqliteCommand cmd, SqliteDataReader reader)> OpenReaders = new();
    private static long _nextHandle;

    [ThreadStatic] private static SqliteConnection? _writeConn;

    private static SqliteConnection Open()
    {
        var c = new SqliteConnection($"Data Source={DbPath}");
        c.Open();
        using var pragma = c.CreateCommand();
        pragma.CommandText = "PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;";
        pragma.ExecuteNonQuery();
        return c;
    }

    private static SqliteConnection Rent()
    {
        Gate.Wait();
        return Pool.TryTake(out var c) ? c : Open();
    }

    private static void ReturnConn(SqliteConnection c)
    {
        Pool.Add(c);
        Gate.Release();
    }

    // Prepare + execute a read query, returning a handle to its cursor (and the
    // rented connection + command it holds until `finalize`).
    public static long Prepare(string sql)
    {
        var conn = Rent();
        var cmd = conn.CreateCommand();
        cmd.CommandText = sql;
        var reader = cmd.ExecuteReader();
        var handle = Interlocked.Increment(ref _nextHandle);
        OpenReaders[handle] = (conn, cmd, reader);
        return handle;
    }

    public static bool StepPred(long stmt) => OpenReaders[stmt].reader.Read();

    public static long ColumnInt(long stmt, long index)
    {
        var r = OpenReaders[stmt].reader;
        return r.IsDBNull((int)index) ? 0L : Convert.ToInt64(r.GetValue((int)index));
    }

    public static string ColumnText(long stmt, long index)
    {
        var r = OpenReaders[stmt].reader;
        return r.IsDBNull((int)index) ? "" : Convert.ToString(r.GetValue((int)index)) ?? "";
    }

    // Nullable-column reads. A column the schema declares nullable
    // holds NULL until something sets it, and NULL is not the type's
    // zero: "" in a nullable UNIQUE column collides row-to-row, and 0
    // in a nullable fk makes `where(fk: nil)` match nothing. The
    // lowerer emits these for those columns; the readers above stay as
    // they are for NOT NULL columns.
    public static long? ColumnIntOpt(long stmt, long index)
    {
        var r = OpenReaders[stmt].reader;
        return r.IsDBNull((int)index) ? null : Convert.ToInt64(r.GetValue((int)index));
    }

    public static double? ColumnFloatOpt(long stmt, long index)
    {
        var r = OpenReaders[stmt].reader;
        return r.IsDBNull((int)index) ? null : Convert.ToDouble(r.GetValue((int)index));
    }

    public static string? ColumnTextOpt(long stmt, long index)
    {
        var r = OpenReaders[stmt].reader;
        return r.IsDBNull((int)index) ? null : Convert.ToString(r.GetValue((int)index));
    }

    public static bool? ColumnBoolOpt(long stmt, long index)
    {
        var v = ColumnIntOpt(stmt, index);
        return v is null ? null : v != 0L;
    }

    // Dispose BOTH the reader and the command — the command owns the native
    // sqlite3_stmt, which `reader.Dispose()` only resets, not frees. Leaking
    // the command lets prepared statements pile up in native memory faster
    // than the GC finalizes the wrappers → unbounded RSS growth under load
    // that no managed GC heap limit can cap.
    public static void Finalize(long stmt)
    {
        if (OpenReaders.TryRemove(stmt, out var e))
        {
            e.reader.Dispose();
            e.cmd.Dispose();
            ReturnConn(e.conn);
        }
    }

    private static SqliteConnection WriteConn() => _writeConn ??= Open();

    // Per-test database: point every future connection at a fresh file and
    // replay the schema DDL. The C# analog of kotlin's `Db.setupTestDb` /
    // swift's per-test `:memory:`.
    //
    // A FILE, not `:memory:`, because this Db pools connections and each
    // connection to `:memory:` is its OWN database — the write would land
    // somewhere the read pool can't see. One path per process, deleted and
    // recreated per test, keeps every connection looking at the same bytes.
    //
    // Existing connections point at the previous file, so they're dropped:
    // pooled readers are disposed (the next `Rent` opens a fresh one — the
    // Gate counts concurrent rentals, not pool contents, so draining it does
    // not desync), and this thread's write connection is closed. Callers run
    // on the test thread with parallelization disabled (see the test
    // project's AssemblyInfo), so per-thread state is per-test state.
    public static void SetupTestDb(string schema)
    {
        foreach (var handle in OpenReaders.Keys)
        {
            Finalize(handle);
        }
        while (Pool.TryTake(out var pooled))
        {
            pooled.Dispose();
        }
        _writeConn?.Dispose();
        _writeConn = null;
        SqliteConnection.ClearAllPools();

        _testPath ??= Path.Combine(
            Path.GetTempPath(), $"roundhouse-test-{Environment.ProcessId}.sqlite3");
        foreach (var suffix in new[] { "", "-wal", "-shm" })
        {
            var f = _testPath + suffix;
            if (File.Exists(f)) File.Delete(f);
        }

        if (schema.Length == 0) return;
        foreach (var stmt in schema.Split(";\n"))
        {
            if (stmt.Trim().Length > 0) Exec(stmt);
        }
    }

    public static void Exec(string sql)
    {
        using var cmd = WriteConn().CreateCommand();
        cmd.CommandText = sql;
        cmd.ExecuteNonQuery();
    }

    public static long LastInsertRowid()
    {
        using var cmd = WriteConn().CreateCommand();
        cmd.CommandText = "SELECT last_insert_rowid()";
        return Convert.ToInt64(cmd.ExecuteScalar());
    }

    // SQL-literal escaping for the inline-VALUES INSERT/UPDATE the lowered
    // `_adapter*` methods build.
    public static string EscapeString(string? value) =>
        "'" + (value ?? "").Replace("'", "''") + "'";
    public static string EscapeInt(long value) => value.ToString();

    // Nullable-column writes: null renders the SQL keyword NULL rather
    // than `''` / `0`, so a nullable column round-trips as null.
    public static string EscapeStringOpt(string? value) =>
        value is null ? "NULL" : EscapeString(value);
    public static string EscapeIntOpt(long? value) =>
        value is null ? "NULL" : value.Value.ToString();
    public static string EscapeFloatOpt(double? value) =>
        value is null ? "NULL" : value.Value.ToString();
    public static string EscapeBoolOpt(bool? value) =>
        value is null ? "NULL" : (value.Value ? "1" : "0");

    // Comma-joined ids for an `IN (...)` clause (the association preload).
    // Empty → `NULL` so `IN (NULL)` stays valid SQL and matches nothing.
    public static string EscapeIntList(List<long> ids) =>
        ids.Count == 0 ? "NULL" : string.Join(", ", ids);
}
