using System;

namespace Roundhouse;

// The sqlite primitive layer the lowered model IR dispatches against
// (`Db.prepare` / `Db.step` / `Db.columnInt` / `Db.columnText` /
// `Db.escapeString` / `Db.escapeInt` / `Db.exec` / `Db.lastInsertRowid` /
// `Db.finalize`). camelCase to match the emitter's rendering; statement
// handles are `long` (the emitter renders integer literals with an `L`
// suffix, so the column-index args are `long`).
//
// **Phase 2 stub** — these compile and behave as a no-op in-memory shim so
// the model layer builds and links. Phase 3 replaces this with the real
// `Microsoft.Data.Sqlite` (ADO.NET) adapter.
public static class Db
{
    public static long prepare(string sql) => 0L;
    public static bool step(long stmt) => false;
    public static long columnInt(long stmt, long index) => 0L;
    public static string columnText(long stmt, long index) => "";
    public static void finalize(long stmt) { }
    public static void exec(string sql) { }
    public static long lastInsertRowid() => 0L;

    // SQL-literal escaping for the inline-VALUES INSERT/UPDATE the lowered
    // `_adapter*` methods build.
    public static string escapeString(string? value) =>
        "'" + (value ?? "").Replace("'", "''") + "'";
    public static string escapeInt(long value) => value.ToString();
}
