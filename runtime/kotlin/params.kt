// Narrowing accessors over the recursive request-params tree.
//
// `Roundhouse::ParamValue` was a per-target TYPE WITH NO OPERATIONS, so
// every consumer narrowed at its own call site with `is_a?`, and each
// emitter recognized that as an IDIOM rather than modeling the test.
// The operations live beside the type, hand-written per target the way
// `Db` is: these bodies inspect this target's representation, which is
// what makes them a primitive rather than framework logic.
//
// Semantics fixed by a Rails 8.1 oracle: `permit` keeps a blank "" and
// `Parameters#compact` drops only an explicit nil. So blank counts as
// provided; absent, JSON null, and non-scalar do not.

package roundhouse

object Params {
    @JvmStatic
    fun sub(params: MutableMap<String, Any?>, key: String): MutableMap<String, Any?> {
        val value = params[key]
        @Suppress("UNCHECKED_CAST")
        return if (value is MutableMap<*, *>) value as MutableMap<String, Any?> else mutableMapOf()
    }

    @JvmStatic
    fun str(sub: MutableMap<String, Any?>, key: String, fallback: String): String {
        val value = sub[key]
        return if (value is String) value else fallback
    }

    @JvmStatic
    fun provided(sub: MutableMap<String, Any?>, key: String): Boolean = sub[key] is String

    // The scalar readers the params lowering emits, and its record of
    // multiparameter keys. Those keys (`title(1)`) are modeled on the
    // Ruby family only (runtime/ruby/params.rb); this target ignores
    // them, as it always has, so these pass through.
    @JvmStatic
    fun scalarProvided(sub: MutableMap<String, Any?>, key: String): Boolean = provided(sub, key)

    @JvmStatic
    fun scalar(sub: MutableMap<String, Any?>, key: String): String = str(sub, key, "")

    @JvmStatic
    fun noFields(): MutableList<String> = mutableListOf()

    @JvmStatic
    fun multiparameterFields(sub: MutableMap<String, Any?>, keys: MutableList<String>): MutableList<String> = mutableListOf()

    @JvmStatic
    fun multiparameterUnsupported(sub: MutableMap<String, Any?>, keys: MutableList<String>): Boolean = false

    @JvmStatic
    fun anyOf(list: MutableList<String>, names: MutableList<String>): Boolean = names.any { it in list }
}
