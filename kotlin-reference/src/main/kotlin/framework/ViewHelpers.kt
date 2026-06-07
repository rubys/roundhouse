package roundhouse

// Subset of ActionView::ViewHelpers the index/_article views call. Also
// normally transpiled from `runtime/ruby/action_view/view_helpers.rb`;
// hand-written minimal here for Phase R. dom_id is specialized to Article
// (the full version is generic over any record); turbo_stream_from and the
// csrf/method inputs are simplified — exact Rails byte-parity is a Phase 6
// (`compare`) concern, not the Phase R gate.
object ViewHelpers {
    private val contentFor = HashMap<String, String>()

    fun htmlEscape(s: String): String =
        s.replace("&", "&amp;")
            .replace("<", "&lt;")
            .replace(">", "&gt;")
            .replace("\"", "&quot;")
            .replace("'", "&#39;")

    fun domId(record: Article, prefix: String? = null): String =
        if (prefix == null) "article_${record.id}" else "${prefix}_article_${record.id}"

    fun truncate(text: String, length: Int = 30): String =
        if (text.length <= length) text else text.substring(0, length - 3) + "..."

    fun turboStreamFrom(name: String): String =
        "<turbo-cable-stream-source channel=\"Turbo::StreamsChannel\" " +
            "signed-stream-name=\"$name\"></turbo-cable-stream-source>"

    fun contentForSet(key: String, value: String) {
        contentFor[key] = value
    }

    fun methodOverrideInput(method: String): String =
        "<input type=\"hidden\" name=\"_method\" value=\"$method\" autocomplete=\"off\">"

    fun csrfTokenHiddenInput(): String =
        "<input type=\"hidden\" name=\"authenticity_token\" value=\"\" autocomplete=\"off\">"
}
