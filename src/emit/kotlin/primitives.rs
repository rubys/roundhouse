//! Hand-written Kotlin runtime primitives.
//!
//! These are the target-specific bottom layer (per `project_two_layer_
//! runtime.md`): types the transpiled framework runtime calls into but
//! that have no Ruby source — they bridge to the JVM/JDBC/Javalin stack.
//! The transpiled `runtime/ruby/*.rb` files reach them by name (same
//! `roundhouse` package), so the surface each exposes is dictated by how
//! the emitter renders the corresponding Ruby calls.
//!
//! Grown one primitive at a time, mirroring the runtime-transpile order:
//! Time first (the only thing standing between `ActiveRecordBase.kt` and a
//! clean compile), then Db / ParamValue (both self-contained and added
//! here), then Server / the adapter once controllers + a `Main` entry
//! exist (Server is coupled to `ArticlesController`, so it's held back to
//! keep every emitted primitive independently compileable).
//!
//! Each primitive is ported from the hand-written Phase R reference under
//! `kotlin-reference/src/main/kotlin/runtime/`, adapted where the emitter's
//! rendering differs from the reference's hand-written call sites (notably:
//! the emitter renders `Ty::Int` literals with an `L` suffix, so `Db`'s
//! column-index params are `Long` here, not the reference's `Int`).

use std::path::PathBuf;

use crate::emit::EmittedFile;

/// `Time.now.utc.iso8601` is the sole Time API the framework runtime uses
/// (`ActiveRecord::Base#fill_timestamps`). The emitter renders that chain
/// as `Time.now().utc.iso8601` — a method call then two property reads —
/// so `now()` returns a `TimeInstant` whose `utc`/`iso8601` are `val`s.
const TIME_KT: &str = r#"// Hand-written roundhouse runtime primitive (no Ruby source).
// Minimal shim for `Time.now.utc.iso8601`, used by
// ActiveRecord::Base#fill_timestamps to stamp created_at/updated_at.

package roundhouse

import java.time.OffsetDateTime
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter
import java.time.temporal.ChronoUnit

object Time {
    fun now(): TimeInstant = TimeInstant(OffsetDateTime.now())
}

class TimeInstant(private val dt: OffsetDateTime) {
    // `Time#utc` — the same instant at a UTC offset.
    val utc: TimeInstant
        get() = TimeInstant(dt.withOffsetSameInstant(ZoneOffset.UTC))

    // `Time#iso8601` — seconds precision, `Z` for a zero offset (the `XXX`
    // pattern renders UTC as `Z`, matching Ruby): `2026-06-07T17:30:00Z`.
    val iso8601: String
        get() = dt.truncatedTo(ChronoUnit.SECONDS)
            .format(DateTimeFormatter.ofPattern("yyyy-MM-dd'T'HH:mm:ssXXX"))
}
"#;

/// The sqlite primitive layer the lowered model IR dispatches against
/// (`Db.prepare` / `Db.step` / `Db.columnInt` / `Db.columnText` /
/// `Db.finalize` / `Db.exec` / `Db.escape*`). Ported from
/// `kotlin-reference/runtime/Db.kt`, with one adaptation: the emitter
/// renders `Ty::Int` literals with an `L` suffix (`Db.columnInt(stmt, 0L)`),
/// so the column-index params are `Long` and shifted to JDBC's 1-based,
/// `Int`-typed column index internally.
const DB_KT: &str = r#"// Hand-written roundhouse runtime primitive (no Ruby source).
// The sqlite primitive layer the lowered model IR dispatches against.
// Mirrors `runtime/crystal/db.cr` and `runtime/typescript/db.ts`: an opaque
// Long stmt id indexes a table of open JDBC ResultSets, and `step` /
// `columnInt` / `columnText` read the cursor. The lowered `_adapter_*` model
// emit calls exactly this surface.
//
// CONCURRENCY: Javalin/Jetty dispatches each request onto its own thread, and
// the whole prepare → step → … → finalize sequence for a request runs
// synchronously on that one thread. So all mutable cursor state is
// THREAD-CONFINED via `ThreadLocal` — each thread lazily opens its own JDBC
// connection and keeps its own statement table, counters, and last-insert
// state. No locks, no shared maps: a JDBC `Connection` isn't safe for
// concurrent use, and a shared `HashMap` corrupts under concurrent writes.
// SQLite serves concurrent readers fine (a per-connection `busy_timeout`
// rides out the brief writer lock). This is the per-thread analog of the
// connection pool the crystal/go/rust runtimes size to wrk concurrency.
//
// xerial `sqlite-jdbc` is the locked driver. JDBC columns are 1-based, and
// the emitter passes a zero-based `Long` index (Int literals carry an `L`
// suffix), so the index is shifted and narrowed to `Int` here.

package roundhouse

import java.io.File
import java.sql.Connection
import java.sql.DriverManager
import java.sql.PreparedStatement
import java.sql.ResultSet

object Db {
    @Volatile private var path: String? = null

    // One connection per thread, opened lazily against the configured path.
    private val tlConn: ThreadLocal<Connection> = ThreadLocal.withInitial {
        val p = path ?: error("Db not opened")
        val c = DriverManager.getConnection("jdbc:sqlite:$p")
        c.createStatement().use { it.execute("PRAGMA busy_timeout=5000") }
        c
    }
    private val tlStatements: ThreadLocal<HashMap<Long, ResultSet>> =
        ThreadLocal.withInitial { HashMap() }
    private val tlOwners: ThreadLocal<HashMap<Long, PreparedStatement>> =
        ThreadLocal.withInitial { HashMap() }
    private val tlNextId: ThreadLocal<Long> = ThreadLocal.withInitial { 0L }
    private val tlRowid: ThreadLocal<Long> = ThreadLocal.withInitial { 0L }
    private val tlChanges: ThreadLocal<Long> = ThreadLocal.withInitial { 0L }

    private fun conn(): Connection = tlConn.get()

    fun openProductionDb(path: String) {
        File(path).parentFile?.mkdirs()
        this.path = path
        // Open this (main) thread's connection eagerly to fail fast on a bad
        // path; worker threads open theirs on first query.
        tlConn.get()
    }

    // Run one-shot DDL/INSERT/UPDATE/DELETE; capture rowid + changes.
    fun exec(sql: String) {
        conn().createStatement().use { st ->
            st.executeUpdate(sql)
            conn().createStatement().use { c ->
                c.executeQuery("SELECT last_insert_rowid(), changes()").use { rs ->
                    if (rs.next()) {
                        tlRowid.set(rs.getLong(1))
                        tlChanges.set(rs.getLong(2))
                    }
                }
            }
        }
    }

    // Prepare a SELECT, returning an opaque integer handle.
    fun prepare(sql: String): Long {
        val ps = conn().prepareStatement(sql)
        val rs = ps.executeQuery()
        val id = tlNextId.get() + 1
        tlNextId.set(id)
        tlStatements.get()[id] = rs
        tlOwners.get()[id] = ps
        return id
    }

    // Advance the cursor; false (snapshot cleared) when exhausted.
    fun step(stmtId: Long): Boolean {
        val rs = tlStatements.get()[stmtId] ?: return false
        return rs.next()
    }

    // Read an integer column at a zero-based index. NULL coerces to 0.
    fun columnInt(stmtId: Long, i: Long): Long {
        val rs = tlStatements.get()[stmtId]!!
        val v = rs.getLong((i + 1).toInt())
        return if (rs.wasNull()) 0L else v
    }

    // Read a text column at a zero-based index. NULL coerces to "".
    fun columnText(stmtId: Long, i: Long): String {
        val rs = tlStatements.get()[stmtId]!!
        return rs.getString((i + 1).toInt()) ?: ""
    }

    // Release the ResultSet + statement. Idempotent.
    fun finalize(stmtId: Long) {
        tlStatements.get().remove(stmtId)?.close()
        tlOwners.get().remove(stmtId)?.close()
    }

    fun lastInsertRowid(): Long = tlRowid.get()
    fun changes(): Long = tlChanges.get()

    // SQL-quote helpers the lowered adapter emit inlines.
    fun escapeString(s: String): String = "'" + s.replace("'", "''") + "'"
    fun escapeInt(n: Long): String = n.toString()
    fun escapeBool(b: Boolean): String = if (b) "1" else "0"
    // `IN (...)` list for a preload query: comma-joined integer ids.
    fun escapeIntList(ids: MutableList<Long>): String = ids.joinToString(", ") { it.toString() }
}
"#;

// The previous `ParamValue` sealed-union primitive was removed: the params
// layer now holds untyped values as Kotlin's top type `Any?` (nested Hash →
// `MutableMap<String, Any?>`, scalar → `String`), which is what
// `<Resource>Params.from_raw`'s lowered `is_a?(Hash)` / `is_a?(String)` checks
// (emitted as `is Map<*,*>` / `is String`) actually match against. A typed
// wrapper failed every check, silently dropping create params. See the
// `Roundhouse::ParamValue` → `Any?` mapping in `ty.rs::render_class` and
// `setParam` in `Server.kt`.

/// The Javalin HTTP listener — the per-target server primitive (cf.
/// `runtime/crystal/server.cr`, `runtime/go/v2/server.go`). Parses the
/// request, dispatches through the transpiled `Router.match` against the
/// app's routes table, instantiates the matched controller, populates its
/// request state (params/flash/session/format), runs `process_action`, and
/// formats the response (redirect, html-with-layout, or json). The routes
/// table, controller factory map, and layout function are passed in by the
/// emitted `Main.kt` (they're app-specific).
const SERVER_KT: &str = r##"// Hand-written roundhouse runtime primitive (no Ruby source).
// Javalin HTTP listener — parse request -> Router.match -> instantiate
// controller -> process_action -> format response. Mirrors
// runtime/crystal/server.cr.

package roundhouse

import io.javalin.Javalin
import io.javalin.http.Context
import io.javalin.http.Handler

object Server {
    // Compiled assets (tailwind.css, turbo.min.js, …) live under
    // static/assets/ and are served at /assets/* by `serveAsset` (called
    // from `dispatch`). Served in-handler rather than via Javalin's
    // staticFiles because the greedy "/<path>" app route shadows Javalin's
    // static handler. Absolute so it's independent of any cwd surprises.
    private val assetsRoot: java.io.File = java.io.File("static/assets").absoluteFile

    fun start(
        dbPath: String,
        port: Int,
        routes: MutableList<Route>,
        controllers: Map<String, () -> ActionControllerBase>,
        layout: (String, String?, String?) -> String,
    ) {
        Db.openProductionDb(dbPath)
        val app = Javalin.create()
        val handler = Handler { ctx -> dispatch(ctx, routes, controllers, layout) }
        for (p in listOf("/", "/<path>")) {
            app.get(p, handler)
            app.post(p, handler)
            app.put(p, handler)
            app.patch(p, handler)
            app.delete(p, handler)
        }
        app.start("127.0.0.1", port)
        println("Roundhouse Kotlin server listening on http://127.0.0.1:$port")
    }

    // Serve a file from static/assets/, content-typed by extension.
    // Path-traversal guarded (the canonical target must stay under the
    // assets root). 404 when missing, so a fresh archive with no built
    // assets still boots and the layout's asset links simply 404.
    private fun serveAsset(ctx: Context, rel: String) {
        val file = java.io.File(assetsRoot, rel).canonicalFile
        if (!file.path.startsWith(assetsRoot.canonicalFile.path) || !file.isFile) {
            ctx.status(404).result("Not Found")
            return
        }
        val contentType = when (file.extension.lowercase()) {
            "css" -> "text/css"
            "js", "mjs" -> "application/javascript"
            "json", "map" -> "application/json"
            "svg" -> "image/svg+xml"
            "png" -> "image/png"
            else -> "application/octet-stream"
        }
        ctx.contentType(contentType).result(file.readBytes())
    }

    private fun dispatch(
        ctx: Context,
        routes: MutableList<Route>,
        controllers: Map<String, () -> ActionControllerBase>,
        layout: (String, String?, String?) -> String,
    ) {
        ViewHelpers.resetSlotsBang()

        // Compiled assets (/assets/tailwind.css, …) — served before route
        // dispatch so the greedy app router doesn't 404 them.
        if (ctx.method().name == "GET" && ctx.path().startsWith("/assets/")) {
            serveAsset(ctx, ctx.path().removePrefix("/assets/"))
            return
        }

        // Rails' `_method` override (button_to delete/patch forms POST).
        var method = ctx.method().name
        if (method == "POST") {
            ctx.formParam("_method")?.let { method = it.uppercase() }
        }

        // A `.json` extension selects the JSON variant.
        var path = ctx.path()
        var format = "html"
        if (path.endsWith(".json")) {
            format = "json"
            path = path.substring(0, path.length - 5)
        }

        val match = Router.match(method, path, routes)
        val factory = match?.let { controllers[it.controller] }
        if (match == null || factory == null) {
            ctx.status(404).result("Not Found")
            return
        }

        val params: MutableMap<String, Any?> = mutableMapOf()
        for ((k, v) in match.pathParams) {
            params[k] = v
        }
        for ((k, vals) in ctx.queryParamMap()) {
            vals.firstOrNull()?.let { setParam(params, k, it) }
        }
        for ((k, vals) in ctx.formParamMap()) {
            vals.firstOrNull()?.let { setParam(params, k, it) }
        }

        val controller = factory()
        controller.params = params
        controller.requestFormat = format
        controller.requestMethod = method
        controller.requestPath = path
        controller.flash = Flash()
        controller.session = Session()
        controller.processAction(match.action)

        val code = controller.status.toInt()
        val location = controller.location
        if (location != null) {
            ctx.status(code)
            ctx.header("Location", location)
        } else if (controller.requestFormat == "json") {
            ctx.status(code).contentType("application/json").result(controller.body)
        } else {
            ctx.status(code)
                .html(layout(controller.body, controller.flash.notice, controller.flash.alert))
        }
    }

    // `article[title]=Foo` -> a nested `MutableMap<String, Any?>`; a bare
    // key -> a scalar `String`. Untyped params are held as `Any?` (the
    // Kotlin top type) so `<Resource>Params.from_raw`'s `is_a?(Hash)` /
    // `is_a?(String)` narrowing — emitted as `is Map<*,*>` / `is String` —
    // matches against real Map/String values rather than a wrapper that
    // would fail every check.
    private fun setParam(params: MutableMap<String, Any?>, key: String, value: String) {
        val open = key.indexOf('[')
        if (open >= 0 && key.endsWith("]")) {
            val outer = key.substring(0, open)
            val inner = key.substring(open + 1, key.length - 1)
            val existing = params[outer]
            @Suppress("UNCHECKED_CAST")
            val dict = if (existing is MutableMap<*, *>) existing as MutableMap<String, Any?> else mutableMapOf()
            dict[inner] = value
            params[outer] = dict
        } else {
            params[key] = value
        }
    }
}
"##;

/// The adapter contract `ActiveRecord::Base`'s class-level CRUD defaults
/// (`_adapter_all` / `_adapter_find_by_id` / `where` / `find_by` / …)
/// dispatch against — the Kotlin analog of the per-target adapter primitive
/// every other backend ships (crystal `db.cr`, go `adapter_interface.go`,
/// rust `adapter_interface.rs`, ts `juntos.ts`). Surface mirrors
/// `runtime/ruby/active_record/base.rbs`'s `AdapterInterface`.
///
/// The legacy *functional* adapter path is DROPPED for Kotlin: there is no
/// Db-backed implementation and `ActiveRecord.adapter` is never assigned.
/// All real CRUD goes Db-direct through the Level-3 per-model overrides
/// (each model's companion re-emits `_adapter_*` calling `Db` itself —
/// Kotlin companions aren't inherited, so Base's defaults are never
/// reached). This interface exists purely as the compile-time contract for
/// those (dead, for real-blog) Base defaults; the only callers without a
/// per-model override are `where`/`find_by`, which real-blog never invokes
/// and which therefore throw `UninitializedPropertyAccessException` if hit
/// — the correct "this path is unsupported" behavior.
const ADAPTER_INTERFACE_KT: &str = r#"// Hand-written roundhouse runtime primitive (no Ruby source).
// The adapter contract ActiveRecord::Base's class-level CRUD defaults
// type-check against. Surface mirrors active_record/base.rbs's
// AdapterInterface. The legacy functional adapter path is dropped for
// Kotlin — no implementation is provided and `ActiveRecord.adapter` is
// never wired; all real CRUD is Db-direct via the Level-3 per-model
// `_adapter_*` overrides. This interface only lets Base's (unreached)
// defaults compile.

package roundhouse

interface AdapterInterface {
    fun all(tableName: String): MutableList<MutableMap<String, Any?>>
    fun find(tableName: String, id: Long): MutableMap<String, Any?>?
    fun where(tableName: String, conditions: MutableMap<String, Any?>): MutableList<MutableMap<String, Any?>>
    fun count(tableName: String): Long
    fun exists(tableName: String, id: Long): Boolean
    fun truncate(tableName: String)
}
"#;

/// Turbo Streams broadcast sink — the object the model `after_*_commit`
/// callbacks dispatch to (`Broadcasts.append`/`prepend`/`replace`/`remove`,
/// each taking a kwargs bag lowered to a `MutableMap<String, Any?>`). A
/// backend-only Kotlin target doesn't hold the websocket fan-out a full
/// Action Cable would, so these are no-ops (the lowered model still
/// *computes* the stream/target/html, it just isn't pushed anywhere) —
/// the analog of go2/rust2's Broadcasts shim. Wiring a real cable transport
/// is a later concern.
const BROADCASTS_KT: &str = r#"// Hand-written roundhouse runtime primitive (no Ruby source).
// Turbo Streams broadcast sink. A backend-only target has no Action Cable
// fan-out, so the model after_*_commit callbacks' broadcasts are no-ops
// here (mirrors go2/rust2's Broadcasts shim).

package roundhouse

object Broadcasts {
    fun append(opts: MutableMap<String, Any?>) {}
    fun prepend(opts: MutableMap<String, Any?>) {}
    fun replace(opts: MutableMap<String, Any?>) {}
    fun remove(opts: MutableMap<String, Any?>) {}
}
"#;

/// The hand-written runtime primitives, emitted under `src/main/kotlin/`.
pub fn primitives() -> Vec<EmittedFile> {
    vec![
        EmittedFile {
            path: PathBuf::from("src/main/kotlin/Time.kt"),
            content: TIME_KT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("src/main/kotlin/Db.kt"),
            content: DB_KT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("src/main/kotlin/AdapterInterface.kt"),
            content: ADAPTER_INTERFACE_KT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("src/main/kotlin/Broadcasts.kt"),
            content: BROADCASTS_KT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("src/main/kotlin/Server.kt"),
            content: SERVER_KT.to_string(),
        },
    ]
}
