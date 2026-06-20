// Hand-written roundhouse runtime primitive (no Ruby source).
// Turbo Streams broadcast sink. The model after_*_commit callbacks pass a
// {stream, target, html} bag; compose the <turbo-stream> wrapper and fan it
// out to /cable subscribers via Cable. Mirrors go/rust/crystal's Broadcasts.

package roundhouse

object Broadcasts {
    fun append(opts: MutableMap<String, Any?>) = record("append", opts)
    fun prepend(opts: MutableMap<String, Any?>) = record("prepend", opts)
    fun replace(opts: MutableMap<String, Any?>) = record("replace", opts)
    fun remove(opts: MutableMap<String, Any?>) = record("remove", opts)

    private fun record(action: String, opts: MutableMap<String, Any?>) {
        val stream = opts["stream"] as? String ?: return
        val target = opts["target"] as? String ?: ""
        val html = opts["html"] as? String ?: ""
        Cable.dispatch(stream, Cable.turboStreamHtml(action, target, html))
    }
}
