// Hand-written roundhouse runtime primitive (no Ruby source).
// Turbo Streams broadcast sink. The model after_*_commit callbacks pass a
// {stream, target, html} bag; compose the <turbo-stream> wrapper and fan it
// out to /cable subscribers via Cable. Mirrors go/rust/crystal's Broadcasts.

package roundhouse

object Broadcasts {
    // Compose the `<turbo-stream>` element for a `turbo_stream.<action>`
    // call in a `.turbo_stream.erb` template. Positional and String-typed on
    // purpose: it is the ONE shape the view lowerer emits on every target
    // (the older keyword/Symbol `render_fragment` spellings had drifted
    // apart between targets).
    //
    // `attributes` carries its own leading space and is written BEFORE
    // `action`/`target`, where turbo-rails' `tag.turbo_stream(template,
    // **attributes, action:, target:)` puts it. Optional, so the
    // three-argument call the view lowerer emits is unchanged.
    @JvmStatic
    @JvmOverloads
    fun turbo_stream_fragment(action: String, target: String, html: String, attributes: String = ""): String =
        if (action == "remove") {
            "<turbo-stream$attributes action=\"remove\" target=\"$target\"></turbo-stream>"
        } else {
            "<turbo-stream$attributes action=\"$action\" target=\"$target\"><template>$html</template></turbo-stream>"
        }

    fun append(opts: MutableMap<String, Any?>) = record("append", opts)
    fun prepend(opts: MutableMap<String, Any?>) = record("prepend", opts)
    fun replace(opts: MutableMap<String, Any?>) = record("replace", opts)
    fun remove(opts: MutableMap<String, Any?>) = record("remove", opts)

    private fun record(action: String, opts: MutableMap<String, Any?>) {
        val stream = opts["stream"] as? String ?: return
        val target = opts["target"] as? String ?: ""
        val html = opts["html"] as? String ?: ""
        // Rendered attribute text (` maintain_scroll="true"`) the
        // broadcast lowering composed. Read here so an app that writes
        // `attributes:` is not silently stripped of it.
        val attributes = opts["attributes"] as? String ?: ""
        Cable.dispatch(stream, Cable.turboStreamHtml(action, target, html, attributes))
    }
}
