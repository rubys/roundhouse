// Hand-written roundhouse runtime primitive (no Ruby source).
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
    fun existsPred(tableName: String, id: Long): Boolean
    fun truncate(tableName: String)
}
