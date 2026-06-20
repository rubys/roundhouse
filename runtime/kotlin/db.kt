// Hand-written roundhouse runtime primitive (no Ruby source).
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

    // Per-test in-memory database: drop this thread's connection (a fresh
    // jdbc:sqlite::memory: connection IS a fresh database) and replay the
    // schema DDL. Tests dispatch synchronously on the JUnit thread, so the
    // thread-local scope matches the test scope.
    fun setupTestDb(schema: String) {
        if (path != null) {
            runCatching { tlConn.get().close() }
        }
        path = ":memory:"
        tlStatements.remove()
        tlOwners.remove()
        tlConn.remove()
        if (schema.isNotEmpty()) {
            for (stmt in schema.split(";\n")) {
                if (stmt.isNotBlank()) {
                    exec(stmt)
                }
            }
        }
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
