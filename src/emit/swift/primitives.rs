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
    // Ruby `to_s` semantics for untyped values: nil → "", optionals
    // unwrap (recursively — `Any` can box nested optionals, which
    // String(describing:)/interpolation would render as "Optional(…)").
    static func s(_ x: Any?) -> String {
        guard let x = x else { return "" }
        let m = Mirror(reflecting: x)
        if m.displayStyle == .optional {
            guard let child = m.children.first else { return "" }
            return s(child.value)
        }
        return "\(x)"
    }

    // Ruby `str.gsub(literal, replacement)` — pure Swift, NO Foundation
    // bridging (corelibs-foundation's replacingOccurrences is the
    // dominant render cost on Linux: NSString UTF-16 bridging + generic
    // search + an allocation per call even when nothing matches).
    // Fast path: no occurrence → the ORIGINAL string back, zero
    // allocation — the common case for html_escape's five passes over
    // ordinary text. Single-ASCII-byte needles (every escape pass, tr)
    // pre-check via a raw UTF-8 byte scan; an ASCII byte can't
    // false-match inside a multibyte sequence.
    static func replace(_ s: String, _ from: String, _ to: String) -> String {
        guard !from.isEmpty else { return s }
        let fromUtf8 = Array(from.utf8)
        if fromUtf8.count == 1 {
            let needle = fromUtf8[0]
            if !s.utf8.contains(needle) { return s }
        }
        guard let first = s.firstRange(of: from) else { return s }
        var out = String(s[s.startIndex..<first.lowerBound])
        out.reserveCapacity(s.utf8.count + to.utf8.count)
        out += to
        var cursor = first.upperBound
        while let r = s[cursor...].firstRange(of: from) {
            out += s[cursor..<r.lowerBound]
            out += to
            cursor = r.upperBound
        }
        out += s[cursor...]
        return out
    }

    // Ruby `str.gsub(regex, map)`: each match is replaced by its map
    // entry (identity when absent). Fast path: when NO map key occurs in
    // the string, any regex match could only be identity-replaced, so
    // the result is the original — skip the (corelibs-slow)
    // NSRegularExpression machinery entirely. The JSON-escape path hits
    // this for every ordinary string.
    static func gsubMap(_ s: String, _ pattern: NSRegularExpression, _ map: [String: String]) -> String {
        if !map.keys.contains(where: { s.contains($0) }) { return s }
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
        "'" + RhString.replace(s, "'", "''") + "'"
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

// The HTTP listener — Hummingbird 2, the locked choice (plan decision
// 1). THE BRIDGE: Hummingbird handlers are async and hop executors,
// which would break the thread-confined Db/slot state — so the handler
// collects the body asynchronously, then runs the ENTIRE synchronous
// dispatch (router match → controller → Db → render) in ONE
// `NIOThreadPool.runIfActive` closure on a stable pool thread.
// `processAction` is the throws boundary: RecordNotFound → 404,
// RecordInvalid → 422 (the Phase 5 throws-propagation contract).
// NOTE: `Hummingbird.Router` is qualified — the transpiled
// ActionDispatch router is this module's `Router`.
const SERVER_SWIFT: &str = r#"import Foundation
import Hummingbird
import NIOCore
import NIOPosix

struct DispatchResult {
    var status: Int
    var contentType: String?
    var location: String?
    var body: String
}

enum Server {
    static func start(
        _ dbPath: String,
        _ port: Int,
        _ routes: [Route],
        _ controllers: [String: () -> ActionControllerBase],
        _ layout: @escaping (String, String?, String?) -> String
    ) async throws {
        Db.openProductionDb(dbPath)

        let hb = Hummingbird.Router()
        for method: HTTPRequest.Method in [.get, .post, .put, .patch, .delete] {
            for path in ["/", "**"] {
                hb.on(RouterPath(path), method: method) { request, _ -> Response in
                    var form: [String: String] = [:]
                    if request.method == .post || request.method == .put || request.method == .patch {
                        var buffer = try await request.body.collect(upTo: 1 << 20)
                        if let s = buffer.readString(length: buffer.readableBytes) {
                            form = parseUrlencoded(s)
                        }
                    }
                    let method = request.method.rawValue
                    let path = request.uri.path
                    let query = parseUrlencoded(request.uri.query ?? "")
                    let r = try await NIOThreadPool.singleton.runIfActive {
                        dispatchScoped(method, path, query, form, routes, controllers, layout)
                    }
                    var headers = HTTPFields()
                    if let ct = r.contentType {
                        headers[.contentType] = ct
                    }
                    if let loc = r.location {
                        headers[.location] = loc
                    }
                    return Response(
                        status: HTTPResponse.Status(code: r.status),
                        headers: headers,
                        body: .init(byteBuffer: ByteBuffer(string: r.body))
                    )
                }
            }
        }

        let app = Application(
            router: hb,
            configuration: .init(address: .hostname("127.0.0.1", port: port))
        )
        print("Roundhouse Swift server listening on http://127.0.0.1:\(port)")
        try await app.runService()
    }

    // Per-request memory scope. NIO pool threads have no run loop, so
    // on Darwin Foundation's autoreleased bridge objects (NSString in
    // replacingOccurrences, NSRegularExpression matches) never drain
    // without an explicit pool — RSS grows linearly under load. Linux
    // Foundation has no autorelease machinery; the plain call is right.
    static func dispatchScoped(
        _ rawMethod: String,
        _ rawPath: String,
        _ query: [String: String],
        _ form: [String: String],
        _ routes: [Route],
        _ controllers: [String: () -> ActionControllerBase],
        _ layout: (String, String?, String?) -> String
    ) -> DispatchResult {
        #if canImport(ObjectiveC)
        return autoreleasepool {
            dispatch(rawMethod, rawPath, query, form, routes, controllers, layout)
        }
        #else
        return dispatch(rawMethod, rawPath, query, form, routes, controllers, layout)
        #endif
    }

    // The whole synchronous request — runs on one pool thread.
    static func dispatch(
        _ rawMethod: String,
        _ rawPath: String,
        _ query: [String: String],
        _ form: [String: String],
        _ routes: [Route],
        _ controllers: [String: () -> ActionControllerBase],
        _ layout: (String, String?, String?) -> String
    ) -> DispatchResult {
        ViewHelpers.resetSlotsBang()

        // Compiled assets (/assets/tailwind.css, …) — before route
        // dispatch so the greedy router doesn't 404 them.
        if rawMethod == "GET" && rawPath.hasPrefix("/assets/") {
            return serveAsset(String(rawPath.dropFirst("/assets/".count)))
        }

        // Rails' `_method` override (button_to delete/patch forms POST).
        var method = rawMethod
        if method == "POST", let override = form["_method"] {
            method = override.uppercased()
        }

        // A `.json` extension selects the JSON variant.
        var path = rawPath
        var format = "html"
        if path.hasSuffix(".json") {
            format = "json"
            path = String(path.dropLast(5))
        }

        guard let match = Router.match(method, path, routes),
              let factory = controllers[match.controller]
        else {
            return DispatchResult(status: 404, contentType: "text/plain", location: nil, body: "Not Found")
        }

        var params: [String: Any?] = [:]
        for (k, v) in match.pathParams {
            params[k] = v
        }
        for (k, v) in query {
            setParam(&params, k, v)
        }
        for (k, v) in form {
            setParam(&params, k, v)
        }

        let controller = factory()
        controller.params = params
        controller.requestFormat = format
        controller.requestMethod = method
        controller.requestPath = path
        controller.flash = Flash()
        controller.session = Session()
        do {
            try controller.processAction(match.action)
        } catch is RecordNotFound {
            return DispatchResult(status: 404, contentType: "text/plain", location: nil, body: "Not Found")
        } catch is RecordInvalid {
            return DispatchResult(status: 422, contentType: "text/plain", location: nil, body: "Unprocessable Entity")
        } catch {
            return DispatchResult(status: 500, contentType: "text/plain", location: nil, body: "Internal Server Error")
        }

        if let location = controller.location {
            return DispatchResult(status: controller.status, contentType: nil, location: location, body: "")
        }
        if controller.requestFormat == "json" {
            return DispatchResult(status: controller.status, contentType: "application/json", location: nil, body: controller.body)
        }
        return DispatchResult(
            status: controller.status,
            contentType: "text/html; charset=utf-8",
            location: nil,
            body: layout(controller.body, controller.flash.notice, controller.flash.alert)
        )
    }

    // `article[title]=Foo` → a nested `[String: Any?]`; a bare key → a
    // scalar String. Untyped params stay Any? so `from_raw`'s
    // `is_a?(Hash)` / `is_a?(String)` narrowing matches real values.
    static func setParam(_ params: inout [String: Any?], _ key: String, _ value: String) {
        if let open = key.firstIndex(of: "["), key.hasSuffix("]") {
            let outer = String(key[key.startIndex..<open])
            let inner = String(key[key.index(after: open)..<key.index(before: key.endIndex)])
            var dict = (params[outer] as? [String: Any?]) ?? [:]
            dict[inner] = value
            params[outer] = dict
        } else {
            params[key] = value
        }
    }

    static func parseUrlencoded(_ s: String) -> [String: String] {
        var out: [String: String] = [:]
        for pair in s.split(separator: "&") {
            let parts = pair.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
            guard let rawKey = parts.first else { continue }
            let rawValue = parts.count > 1 ? parts[1] : ""
            let decode = { (x: Substring) -> String in
                x.replacingOccurrences(of: "+", with: " ").removingPercentEncoding
                    ?? String(x)
            }
            out[decode(rawKey)] = decode(rawValue)
        }
        return out
    }

    // Serve a file from static/assets/, content-typed by extension.
    // Path-traversal guarded; 404 when missing so an archive with no
    // built assets still boots.
    static func serveAsset(_ rel: String) -> DispatchResult {
        let root = URL(fileURLWithPath: "static/assets").standardizedFileURL
        let file = root.appendingPathComponent(rel).standardizedFileURL
        guard file.path.hasPrefix(root.path),
              let data = try? Data(contentsOf: file),
              let body = String(data: data, encoding: .utf8)
        else {
            return DispatchResult(status: 404, contentType: "text/plain", location: nil, body: "Not Found")
        }
        let contentType: String
        switch file.pathExtension.lowercased() {
        case "css": contentType = "text/css"
        case "js", "mjs": contentType = "application/javascript"
        case "json", "map": contentType = "application/json"
        case "svg": contentType = "image/svg+xml"
        default: contentType = "application/octet-stream"
        }
        return DispatchResult(status: 200, contentType: contentType, location: nil, body: body)
    }
}
"#;

// Thread-confined mutable slot — the Swift analog of the Kotlin
// OBJECT_TL_FIELDS ThreadLocal conversion (the fix that ended Kotlin's
// cross-request state bleed). Module-level mutable `@ivar` state
// (ViewHelpers' content_for slots) emits as a computed static property
// backed by one of these: each NIOThreadPool thread sees its own value,
// and since a request's whole dispatch runs on one pool thread
// (Server.swift's runIfActive bridge), per-thread IS per-request.
// ThreadSpecificVariable requires a class value, hence the Box.
const RHTHREADLOCAL_SWIFT: &str = r#"import NIOPosix

final class RhThreadLocal<T> {
    private final class Box {
        var value: T
        init(_ value: T) { self.value = value }
    }

    private let tsv = ThreadSpecificVariable<Box>()
    private let makeDefault: () -> T

    init(_ makeDefault: @escaping () -> T) {
        self.makeDefault = makeDefault
    }

    var value: T {
        get {
            if let box = tsv.currentValue { return box.value }
            let box = Box(makeDefault())
            tsv.currentValue = box
            return box.value
        }
        set {
            if let box = tsv.currentValue {
                box.value = newValue
            } else {
                tsv.currentValue = Box(newValue)
            }
        }
    }
}
"#;

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
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/Server.swift"),
            content: SERVER_SWIFT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/RhThreadLocal.swift"),
            content: RHTHREADLOCAL_SWIFT.to_string(),
        },
    ]
}
