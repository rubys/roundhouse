import CSQLite
import Foundation
import NIOPosix

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

    static func openProductionDb(_ path: String) {
        dbPath = path
    }

    // Test mode: fresh in-memory DB + schema, replacing this thread's
    // connection. Called from RoundhouseTestCase.setUpWithError before
    // every test (XCTest runs tests serially on one thread, so the
    // thread-confined connection IS the test's connection).
    static func setupTestDb(_ schema: String) {
        dbPath = ":memory:"
        tlConn.currentValue = nil
        if !schema.isEmpty {
            exec(schema)
        }
    }

    static func exec(_ sql: String) {
        let c = conn()
        guard sqlite3_exec(c.handle, sql, nil, nil, nil) == SQLITE_OK else {
            fatalError("sqlite exec failed: \(String(cString: sqlite3_errmsg(c.handle)))")
        }
        c.lastInsertRowid = Int(sqlite3_last_insert_rowid(c.handle))
        c.changes = Int(sqlite3_changes(c.handle))
    }

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

    static func step(_ stmtId: Int) -> Bool {
        guard let stmt = conn().statements[stmtId] else { return false }
        return sqlite3_step(stmt) == SQLITE_ROW
    }

    static func columnInt(_ stmtId: Int, _ i: Int) -> Int {
        let stmt = conn().statements[stmtId]!
        return Int(sqlite3_column_int64(stmt, Int32(i)))
    }

    static func columnText(_ stmtId: Int, _ i: Int) -> String {
        let stmt = conn().statements[stmtId]!
        guard let text = sqlite3_column_text(stmt, Int32(i)) else { return "" }
        return String(cString: text)
    }

    static func finalize(_ stmtId: Int) {
        if let stmt = conn().statements.removeValue(forKey: stmtId) {
            sqlite3_finalize(stmt)
        }
    }

    static func lastInsertRowid() -> Int { conn().lastInsertRowid }
    static func changes() -> Int { conn().changes }

    static func escapeString(_ s: String) -> String {
        "'" + s.replacingOccurrences(of: "'", with: "''") + "'"
    }
    static func escapeInt(_ n: Int) -> String { String(n) }
    static func escapeBool(_ b: Bool) -> String { b ? "1" : "0" }
    static func escapeIntList(_ ids: [Int]) -> String {
        ids.map(String.init).joined(separator: ", ")
    }
}
