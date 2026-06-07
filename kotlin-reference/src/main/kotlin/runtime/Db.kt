package roundhouse

import java.io.File
import java.sql.Connection
import java.sql.DriverManager
import java.sql.PreparedStatement
import java.sql.ResultSet

// Roundhouse Kotlin DB runtime — the sqlite primitive layer the lowered
// model IR dispatches against. Mirrors `runtime/crystal/db.cr` and
// `runtime/typescript/db.ts`: an opaque Long stmt id indexes a table of
// open JDBC ResultSets, and `step` / `columnInt` / `columnText` read the
// cursor. The lowered `Article.fromStmt` emit calls exactly this surface
// (`Db.prepare`, `Db.step`, `Db.columnInt`, `Db.columnText`, `Db.finalize`).
//
// This is a HAND-WRITTEN per-target primitive (Phase R reference). xerial
// `sqlite-jdbc` is the locked driver. JDBC columns are 1-based, so the
// zero-based index the emit passes is shifted by one here.
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
    fun columnInt(stmtId: Long, i: Int): Long {
        val rs = statements[stmtId]!!
        val v = rs.getLong(i + 1)
        return if (rs.wasNull()) 0L else v
    }

    // Read a text column at a zero-based index. NULL coerces to "".
    fun columnText(stmtId: Long, i: Int): String {
        val rs = statements[stmtId]!!
        return rs.getString(i + 1) ?: ""
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
