using System;
using System.Collections.Generic;
using Microsoft.Data.Sqlite;

namespace Roundhouse;

// The sqlite primitive layer the lowered model IR dispatches against
// (`Db.prepare` / `Db.step` / `Db.columnInt` / `Db.columnText` /
// `Db.escapeString` / `Db.escapeInt` / `Db.exec` / `Db.lastInsertRowid` /
// `Db.finalize`). camelCase to match the emitter's rendering.
//
// Prepared-statement handles are `long`s (the emitter renders integer
// literals with an `L` suffix, so the column-index args are `long`) mapped to
// live `SqliteDataReader`s. A single shared connection backs the process; the
// DB path comes from `BLOG_DB` / `DATABASE_PATH` (Rails-traditional default
// `storage/development.sqlite3`).
public static class Db
{
    private static SqliteConnection? _conn;
    private static readonly Dictionary<long, SqliteDataReader> _readers = new();
    private static long _nextHandle;

    private static SqliteConnection Conn
    {
        get
        {
            if (_conn == null)
            {
                var path = Environment.GetEnvironmentVariable("BLOG_DB")
                    ?? Environment.GetEnvironmentVariable("DATABASE_PATH")
                    ?? "storage/development.sqlite3";
                _conn = new SqliteConnection($"Data Source={path}");
                _conn.Open();
            }
            return _conn;
        }
    }

    // Prepare + execute a query, returning a handle to its result cursor.
    public static long prepare(string sql)
    {
        var cmd = Conn.CreateCommand();
        cmd.CommandText = sql;
        var reader = cmd.ExecuteReader();
        var handle = ++_nextHandle;
        _readers[handle] = reader;
        return handle;
    }

    // Advance the cursor; false when exhausted.
    public static bool step(long stmt) => _readers[stmt].Read();

    public static long columnInt(long stmt, long index)
    {
        var r = _readers[stmt];
        return r.IsDBNull((int)index) ? 0L : Convert.ToInt64(r.GetValue((int)index));
    }

    public static string columnText(long stmt, long index)
    {
        var r = _readers[stmt];
        return r.IsDBNull((int)index) ? "" : Convert.ToString(r.GetValue((int)index)) ?? "";
    }

    public static void finalize(long stmt)
    {
        if (_readers.Remove(stmt, out var reader))
        {
            reader.Dispose();
        }
    }

    // Run a statement with no result set (INSERT/UPDATE/DELETE/DDL).
    public static void exec(string sql)
    {
        var cmd = Conn.CreateCommand();
        cmd.CommandText = sql;
        cmd.ExecuteNonQuery();
    }

    public static long lastInsertRowid()
    {
        var cmd = Conn.CreateCommand();
        cmd.CommandText = "SELECT last_insert_rowid()";
        return Convert.ToInt64(cmd.ExecuteScalar());
    }

    // SQL-literal escaping for the inline-VALUES INSERT/UPDATE the lowered
    // `_adapter*` methods build.
    public static string escapeString(string? value) =>
        "'" + (value ?? "").Replace("'", "''") + "'";
    public static string escapeInt(long value) => value.ToString();
}
