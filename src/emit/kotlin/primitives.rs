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
    private var db: Connection? = null
    private val statements = HashMap<Long, ResultSet>()
    private val owners = HashMap<Long, PreparedStatement>()
    private var nextId: Long = 0
    private var lastInsertRowid: Long = 0
    private var changes: Long = 0

    private fun conn(): Connection = db ?: error("Db not opened")

    fun openProductionDb(path: String) {
        resetStatements()
        db?.close()
        File(path).parentFile?.mkdirs()
        db = DriverManager.getConnection("jdbc:sqlite:$path")
    }

    // Run one-shot DDL/INSERT/UPDATE/DELETE; capture rowid + changes.
    fun exec(sql: String) {
        conn().createStatement().use { st ->
            st.executeUpdate(sql)
            conn().createStatement().use { c ->
                c.executeQuery("SELECT last_insert_rowid(), changes()").use { rs ->
                    if (rs.next()) {
                        lastInsertRowid = rs.getLong(1)
                        changes = rs.getLong(2)
                    }
                }
            }
        }
    }

    // Prepare a SELECT, returning an opaque integer handle.
    fun prepare(sql: String): Long {
        val ps = conn().prepareStatement(sql)
        val rs = ps.executeQuery()
        nextId += 1
        statements[nextId] = rs
        owners[nextId] = ps
        return nextId
    }

    // Advance the cursor; false (snapshot cleared) when exhausted.
    fun step(stmtId: Long): Boolean {
        val rs = statements[stmtId] ?: return false
        return rs.next()
    }

    // Read an integer column at a zero-based index. NULL coerces to 0.
    fun columnInt(stmtId: Long, i: Long): Long {
        val rs = statements[stmtId]!!
        val v = rs.getLong((i + 1).toInt())
        return if (rs.wasNull()) 0L else v
    }

    // Read a text column at a zero-based index. NULL coerces to "".
    fun columnText(stmtId: Long, i: Long): String {
        val rs = statements[stmtId]!!
        return rs.getString((i + 1).toInt()) ?: ""
    }

    // Release the ResultSet + statement. Idempotent.
    fun finalize(stmtId: Long) {
        statements.remove(stmtId)?.close()
        owners.remove(stmtId)?.close()
    }

    fun lastInsertRowid(): Long = lastInsertRowid
    fun changes(): Long = changes

    // SQL-quote helpers the lowered adapter emit inlines.
    fun escapeString(s: String): String = "'" + s.replace("'", "''") + "'"
    fun escapeInt(n: Long): String = n.toString()
    fun escapeBool(b: Boolean): String = if (b) "1" else "0"
    // `IN (...)` list for a preload query: comma-joined integer ids.
    fun escapeIntList(ids: MutableList<Long>): String = ids.joinToString(", ") { it.toString() }

    private fun resetStatements() {
        statements.values.forEach { runCatching { it.close() } }
        owners.values.forEach { runCatching { it.close() } }
        statements.clear()
        owners.clear()
        nextId = 0
        lastInsertRowid = 0
        changes = 0
    }
}
"#;

/// The recursive params value type — the closed union Ruby's untyped nested
/// params Hash lowers to. Ported verbatim from
/// `kotlin-reference/runtime/ParamValue.kt`; self-contained (no consumers
/// emitted yet, but it locks the shape the params layer targets).
const PARAM_VALUE_KT: &str = r#"// Hand-written roundhouse runtime primitive (no Ruby source).
// The recursive params value type — the Kotlin analog of
// `runtime/crystal/param_value.cr` (`String | Hash | Array`) and
// `runtime/typescript/param_value.ts`. Ruby's untyped nested params Hash
// lowers to this closed union; `<Resource>Params.from_raw` narrows via
// `is Str` / `is Dict` at access sites.

package roundhouse

sealed interface ParamValue {
    // `to_s` on a scalar param (`params[:id].to_s`) must yield the inner
    // string, not the value-class wrapper's default `Str(value=…)`.
    @JvmInline value class Str(val value: String) : ParamValue {
        override fun toString(): String = value
    }
    @JvmInline value class Dict(val value: MutableMap<String, ParamValue>) : ParamValue
    @JvmInline value class Arr(val value: MutableList<ParamValue>) : ParamValue
}
"#;

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

    private fun dispatch(
        ctx: Context,
        routes: MutableList<Route>,
        controllers: Map<String, () -> ActionControllerBase>,
        layout: (String, String?, String?) -> String,
    ) {
        ViewHelpers.resetSlotsBang()

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

        val params: MutableMap<String, ParamValue> = mutableMapOf()
        for ((k, v) in match.pathParams) {
            params[k] = ParamValue.Str(v)
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

    // `article[title]=Foo` -> a nested `Dict`; a bare key -> a scalar `Str`.
    private fun setParam(params: MutableMap<String, ParamValue>, key: String, value: String) {
        val open = key.indexOf('[')
        if (open >= 0 && key.endsWith("]")) {
            val outer = key.substring(0, open)
            val inner = key.substring(open + 1, key.length - 1)
            val existing = params[outer]
            val dict = if (existing is ParamValue.Dict) existing.value else mutableMapOf()
            dict[inner] = ParamValue.Str(value)
            params[outer] = ParamValue.Dict(dict)
        } else {
            params[key] = ParamValue.Str(value)
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
            path: PathBuf::from("src/main/kotlin/ParamValue.kt"),
            content: PARAM_VALUE_KT.to_string(),
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
