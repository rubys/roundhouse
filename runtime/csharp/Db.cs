using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Threading;
using Microsoft.Data.Sqlite;

namespace Roundhouse;

// The sqlite primitive layer the lowered model IR dispatches against
// (`Db.prepare` / `Db.step` / `Db.columnInt` / `Db.columnText` /
// `Db.escapeString` / `Db.escapeInt` / `Db.exec` / `Db.lastInsertRowid` /
// `Db.finalize`). camelCase to match the emitter's rendering. Prepared-
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
    private static string DbPath =>
        Environment.GetEnvironmentVariable("BLOG_DB")
        ?? Environment.GetEnvironmentVariable("DATABASE_PATH")
        ?? "storage/development.sqlite3";

    private static readonly int PoolSize = Math.Max(4, Environment.ProcessorCount);
    private static readonly SemaphoreSlim Gate = new(PoolSize, PoolSize);
    private static readonly ConcurrentBag<SqliteConnection> Pool = new();
    private static readonly ConcurrentDictionary<long, (SqliteConnection conn, SqliteDataReader reader)> OpenReaders = new();
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
    // rented connection it holds until `finalize`).
    public static long prepare(string sql)
    {
        var conn = Rent();
        var cmd = conn.CreateCommand();
        cmd.CommandText = sql;
        var reader = cmd.ExecuteReader();
        var handle = Interlocked.Increment(ref _nextHandle);
        OpenReaders[handle] = (conn, reader);
        return handle;
    }

    public static bool step(long stmt) => OpenReaders[stmt].reader.Read();

    public static long columnInt(long stmt, long index)
    {
        var r = OpenReaders[stmt].reader;
        return r.IsDBNull((int)index) ? 0L : Convert.ToInt64(r.GetValue((int)index));
    }

    public static string columnText(long stmt, long index)
    {
        var r = OpenReaders[stmt].reader;
        return r.IsDBNull((int)index) ? "" : Convert.ToString(r.GetValue((int)index)) ?? "";
    }

    public static void finalize(long stmt)
    {
        if (OpenReaders.TryRemove(stmt, out var e))
        {
            e.reader.Dispose();
            ReturnConn(e.conn);
        }
    }

    private static SqliteConnection WriteConn() => _writeConn ??= Open();

    public static void exec(string sql)
    {
        var cmd = WriteConn().CreateCommand();
        cmd.CommandText = sql;
        cmd.ExecuteNonQuery();
    }

    public static long lastInsertRowid()
    {
        var cmd = WriteConn().CreateCommand();
        cmd.CommandText = "SELECT last_insert_rowid()";
        return Convert.ToInt64(cmd.ExecuteScalar());
    }

    // SQL-literal escaping for the inline-VALUES INSERT/UPDATE the lowered
    // `_adapter*` methods build.
    public static string escapeString(string? value) =>
        "'" + (value ?? "").Replace("'", "''") + "'";
    public static string escapeInt(long value) => value.ToString();

    // Comma-joined ids for an `IN (...)` clause (the association preload).
    // Empty → `NULL` so `IN (NULL)` stays valid SQL and matches nothing.
    public static string escapeIntList(List<long> ids) =>
        ids.Count == 0 ? "NULL" : string.Join(", ", ids);
}
