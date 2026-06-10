//! Hand-written Swift primitives — the bottom layer the transpiled
//! framework runtime and lowered app code call into. The analog of
//! `src/emit/kotlin/primitives.rs` (and of `swift-reference/`'s
//! `runtime/` directory, which is the verified template). Grown one
//! primitive at a time as the transpiled runtime needs them.

use std::path::PathBuf;

use crate::emit::EmittedFile;

// String helpers with no clean inline-emit idiom. `gsubMap` is the
// regex-replace-with-lookup-table JsonBuilder's escaping uses;
// `gsub` is the plain regex template replace.
const RHSTRING_SWIFT: &str = r#"import Foundation

enum RhString {
    // Ruby `str.gsub(regex, map)`: each match is replaced by its map
    // entry (identity when absent).
    static func gsubMap(_ s: String, _ pattern: NSRegularExpression, _ map: [String: String]) -> String {
        let ns = s as NSString
        var result = ""
        var last = 0
        for m in pattern.matches(in: s, range: NSRange(location: 0, length: ns.length)) {
            result += ns.substring(with: NSRange(location: last, length: m.range.location - last))
            let matched = ns.substring(with: m.range)
            result += map[matched] ?? matched
            last = m.range.location + m.range.length
        }
        result += ns.substring(from: last)
        return result
    }

    // Ruby `str.gsub(regex, replacement)`.
    static func gsub(_ s: String, _ pattern: NSRegularExpression, _ replacement: String) -> String {
        let ns = s as NSString
        return pattern.stringByReplacingMatches(
            in: s,
            range: NSRange(location: 0, length: ns.length),
            withTemplate: replacement
        )
    }
}
"#;

// The sqlite layer the lowered `_adapter_*` model emit calls — ported
// from `swift-reference/Sources/App/runtime/Db.swift` (the verified
// Phase R shape): system SQLite3 C API via CSQLite, THREAD-CONFINED via
// ThreadSpecificVariable (each pool thread opens its own connection +
// statement table; a request's whole prepare→step→finalize runs on one
// thread — the Kotlin 7k→54k lesson, applied proactively).
const DB_SWIFT: &str = r#"import CSQLite
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
"#;

// `Time.now.utc.iso8601` (the timestamp path in Base#save) — emitted as
// `Time.now().utc.iso8601`, so: a method + two property reads. Ported
// from the Kotlin Time.kt design: truncate to seconds, `Z` offset
// rendering to match Ruby. Hand-rolled formatter (the plan's Linux
// Foundation caveat — no ISO8601DateFormatter divergence risk).
const TIME_SWIFT: &str = r#"import Foundation

enum Time {
    static func now() -> TimeInstant { TimeInstant(Date()) }
}

struct TimeInstant {
    let date: Date

    init(_ date: Date) { self.date = date }

    var utc: TimeInstant { self }

    var iso8601: String {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "UTC")!
        let c = cal.dateComponents([.year, .month, .day, .hour, .minute, .second], from: date)
        return String(
            format: "%04d-%02d-%02dT%02d:%02d:%02dZ",
            c.year!, c.month!, c.day!, c.hour!, c.minute!, c.second!
        )
    }
}
"#;

// Action Cable broadcast sink — backend-only target has no cable
// transport; the lowered after_*_commit callbacks call these as no-ops
// (same as Kotlin's Broadcasts.kt).
const BROADCASTS_SWIFT: &str = r#"enum Broadcasts {
    static func append(_ args: [String: Any?]) {}
    static func prepend(_ args: [String: Any?]) {}
    static func replace(_ args: [String: Any?]) {}
    static func remove(_ args: [String: Any?]) {}
}
"#;

// The compile-time contract for `ActiveRecord.adapter` (base.rbs
// AdapterInterface). NO implementation ships — the adapter slot is never
// assigned (the Kotlin "drop the functional adapter" decision): all real
// CRUD is Db-direct via the per-model `_adapter_*` overrides; Base's
// where/find_by are the only callers and real-blog never invokes them
// (an unwrapped-nil crash there is the correct "unsupported" signal).
const ADAPTER_INTERFACE_SWIFT: &str = r#"protocol AdapterInterface {
    func all(_ tableName: String) -> [[String: Any?]]
    func find(_ tableName: String, _ id: Int) -> [String: Any?]?
    func `where`(_ tableName: String, _ conditions: [String: Any?]) -> [[String: Any?]]
    func count(_ tableName: String) -> Int
    func exists(_ tableName: String, _ id: Int) -> Bool
    func insert(_ tableName: String, _ attributes: [String: Any?]) -> Int
    func update(_ tableName: String, _ id: Int, _ attributes: [String: Any?])
    func delete(_ tableName: String, _ id: Int)
    func truncate(_ tableName: String)
}
"#;

// NOTE: no ParamValue primitive. The enum-union shape (locked in
// swift-reference) doesn't survive the runtime's untyped `is_a?(Hash)`
// narrowing — the Kotlin arc hit this exact failure and resolved it by
// mapping ParamValue → the top type (see ty.rs); params are nested
// `[String: Any?]` maps end-to-end.

/// The hand-written primitive files, emitted under `Sources/App/runtime/`.
pub fn primitives() -> Vec<EmittedFile> {
    vec![
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/RhString.swift"),
            content: RHSTRING_SWIFT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/Db.swift"),
            content: DB_SWIFT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/Time.swift"),
            content: TIME_SWIFT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/Broadcasts.swift"),
            content: BROADCASTS_SWIFT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/AdapterInterface.swift"),
            content: ADAPTER_INTERFACE_SWIFT.to_string(),
        },
    ]
}
