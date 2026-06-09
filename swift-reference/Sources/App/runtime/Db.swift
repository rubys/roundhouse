import CSQLite
import Foundation
import NIOPosix

// Roundhouse Swift DB runtime — the sqlite primitive layer the lowered
// model IR dispatches against. Mirrors `kotlin-reference/runtime/Db.kt`
// (and `runtime/crystal/db.cr`): an opaque Int stmt id indexes a table of
// prepared statements, and `step` / `columnInt` / `columnText` read the
// cursor. The lowered `Article.fromStmt` emit calls exactly this surface
// (`Db.prepare`, `Db.step`, `Db.columnInt`, `Db.columnText`, `Db.finalize`).
//
// This is a HAND-WRITTEN per-target primitive (Phase R reference). The
// system SQLite3 C API (via the CSQLite systemLibrary target) is the
// locked driver — no JDBC indirection, so unlike Db.kt there is no
// ResultSet table or 1-based column shift; the C API is already
// cursor-shaped and zero-based.
//
// THREAD-CONFINED from the start (plan decision 4, the Kotlin 7k→54k
// lesson): each NIOThreadPool thread lazily opens its own connection and
// keeps its own statement table via ThreadSpecificVariable. A request's
// whole prepare→step→finalize runs on one pool thread (see Server.swift),
// so no locks.

final class DbConnection {
    let handle: OpaquePointer
    var statements: [Int: OpaquePointer] = [:]
    var nextId: Int = 0
    var lastInsertRowid: Int = 0
    var changes: Int = 0

    init(path: String) {
        var h: OpaquePointer? = nil
        guard sqlite3_open(path, &h) == SQLITE_OK, let opened = h else {
            fatalError("cannot open database at \(path)")
        }
        handle = opened
        sqlite3_busy_timeout(handle, 5000)
    }

    deinit {
        for (_, stmt) in statements { sqlite3_finalize(stmt) }
        sqlite3_close(handle)
    }
}

enum Db {
    private static var dbPath = "storage/development.sqlite3"
    private static let tlConn = ThreadSpecificVariable<DbConnection>()

    private static func conn() -> DbConnection {
        if let c = tlConn.currentValue { return c }
        let c = DbConnection(path: dbPath)
        tlConn.currentValue = c
        return c
    }

    // Record the path; each pool thread opens its own connection lazily.
    static func openProductionDb(_ path: String) {
        dbPath = path
    }

    // Run one-shot DDL/INSERT/UPDATE/DELETE; capture rowid + changes.
    static func exec(_ sql: String) {
        let c = conn()
        guard sqlite3_exec(c.handle, sql, nil, nil, nil) == SQLITE_OK else {
            fatalError("sqlite exec failed: \(String(cString: sqlite3_errmsg(c.handle)))")
        }
        c.lastInsertRowid = Int(sqlite3_last_insert_rowid(c.handle))
        c.changes = Int(sqlite3_changes(c.handle))
    }

    // Prepare a SELECT, returning an opaque integer handle.
    static func prepare(_ sql: String) -> Int {
        let c = conn()
        var stmt: OpaquePointer? = nil
        guard sqlite3_prepare_v2(c.handle, sql, -1, &stmt, nil) == SQLITE_OK, let prepared = stmt else {
            fatalError("sqlite prepare failed: \(String(cString: sqlite3_errmsg(c.handle)))")
        }
        c.nextId += 1
        c.statements[c.nextId] = prepared
        return c.nextId
    }

    // Advance the cursor; false when exhausted.
    static func step(_ stmtId: Int) -> Bool {
        guard let stmt = conn().statements[stmtId] else { return false }
        return sqlite3_step(stmt) == SQLITE_ROW
    }

    // Read an integer column at a zero-based index. NULL coerces to 0.
    static func columnInt(_ stmtId: Int, _ i: Int) -> Int {
        let stmt = conn().statements[stmtId]!
        return Int(sqlite3_column_int64(stmt, Int32(i)))
    }

    // Read a text column at a zero-based index. NULL coerces to "".
    static func columnText(_ stmtId: Int, _ i: Int) -> String {
        let stmt = conn().statements[stmtId]!
        guard let text = sqlite3_column_text(stmt, Int32(i)) else { return "" }
        return String(cString: text)
    }

    // Release the statement. Idempotent.
    static func finalize(_ stmtId: Int) {
        if let stmt = conn().statements.removeValue(forKey: stmtId) {
            sqlite3_finalize(stmt)
        }
    }

    static func lastInsertRowid() -> Int { conn().lastInsertRowid }
    static func changes() -> Int { conn().changes }

    // SQL-quote helpers the lowered adapter emit inlines.
    static func escapeString(_ s: String) -> String {
        "'" + s.replacingOccurrences(of: "'", with: "''") + "'"
    }
    static func escapeInt(_ n: Int) -> String { String(n) }
    static func escapeBool(_ b: Bool) -> String { b ? "1" : "0" }
}
