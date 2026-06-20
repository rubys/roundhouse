package roundhouse

import org.junit.jupiter.api.Assertions.fail

open class RoundhouseTestCase {
    var __status: Long = 200L
    var __body: String = ""
    var __location: String = ""
    var __flash = Flash()
    var __session = Session()

    @org.junit.jupiter.api.BeforeEach
    fun __rhSetUp() {
        if (RoundhouseTestSetup.schemaSql.isNotEmpty()) {
            Db.setupTestDb(RoundhouseTestSetup.schemaSql)
            for (loader in RoundhouseTestSetup.fixtureLoaders) {
                loader()
            }
        }
        ViewHelpers.resetSlotsBang()
        __flash = Flash()
        __session = Session()
    }

    // ── controller-test dispatch ─────────────────────────────────

    fun get(path: String) {
        performRequest("GET", path, mutableMapOf())
    }

    fun post(path: String, opts: MutableMap<String, Any?> = mutableMapOf()) {
        performRequest("POST", path, requestParams(opts))
    }

    fun patch(path: String, opts: MutableMap<String, Any?> = mutableMapOf()) {
        performRequest("PATCH", path, requestParams(opts))
    }

    fun delete(path: String, opts: MutableMap<String, Any?> = mutableMapOf()) {
        performRequest("DELETE", path, requestParams(opts))
    }

    @Suppress("UNCHECKED_CAST")
    private fun requestParams(opts: MutableMap<String, Any?>): MutableMap<String, Any?> {
        return (opts["params"] as? MutableMap<String, Any?>) ?: mutableMapOf()
    }

    private fun performRequest(method: String, path: String, params: MutableMap<String, Any?>) {
        ViewHelpers.resetSlotsBang()
        val match = Router.match(method, path, RoundhouseTestSetup.routes)
        if (match == null) {
            fail<Unit>("no route for $method $path")
            return
        }
        val factory = RoundhouseTestSetup.controllers[match.controller]
        if (factory == null) {
            fail<Unit>("no controller registered for ${match.controller}")
            return
        }
        val merged: MutableMap<String, Any?> = mutableMapOf()
        merged.putAll(params)
        for ((k, v) in match.pathParams) {
            merged[k] = v
        }
        val controller = factory()
        controller.params = merged
        controller.requestFormat = "html"
        controller.requestMethod = method
        controller.requestPath = path
        controller.flash = __flash
        controller.session = __session
        try {
            controller.processAction(match.action)
        } catch (e: RecordNotFound) {
            __status = 404L
            __body = ""
            __location = ""
            return
        }
        __status = controller.status
        __body = controller.body
        __location = controller.location ?: ""
        __flash = controller.flash
    }

    // ── HTTP response assertions ─────────────────────────────────

    private val statusRanges: Map<String, LongRange> = mapOf(
        "success" to 200L..299L,
        "redirect" to 300L..399L,
        "missing" to 404L..404L,
        "not_found" to 404L..404L,
        "error" to 500L..599L,
        "ok" to 200L..200L,
        "created" to 201L..201L,
        "no_content" to 204L..204L,
        "moved_permanently" to 301L..301L,
        "found" to 302L..302L,
        "see_other" to 303L..303L,
        "bad_request" to 400L..400L,
        "unauthorized" to 401L..401L,
        "forbidden" to 403L..403L,
        "unprocessable_entity" to 422L..422L,
        "unprocessable_content" to 422L..422L,
        "internal_server_error" to 500L..500L,
    )

    fun assertResponse(expected: String) {
        val range = statusRanges[expected]
        if (range == null) {
            fail<Unit>("unknown response expectation $expected")
            return
        }
        if (__status !in range) {
            fail<Unit>("expected response $expected, got status=$__status body=${__body.take(200)}")
        }
    }

    fun assertRedirectedTo(expectedPath: String) {
        if (__status < 300L || __status >= 400L) {
            fail<Unit>("expected a redirect, got status=$__status location=$__location")
            return
        }
        if (!__location.contains(expectedPath)) {
            fail<Unit>("expected Location to contain $expectedPath, got $__location")
        }
    }

    // `assertSelect` over the Dom primitive surface (defined below).
    // Presence check: the selector matches at least one node. The stub
    // Dom is a substring matcher, so this stays rough-but-effective for
    // the scaffold-blog HTML shapes; cardinality kwargs are best-effort
    // no-ops. A real engine tightens it without changing these sites.
    fun assertSelect(selector: String) {
        if (Dom.select(Dom.parse(__body), selector).isEmpty()) {
            fail<Unit>("expected body to match selector $selector")
        }
    }

    fun assertSelect(selector: String, content: String) {
        val nodes = Dom.select(Dom.parse(__body), selector)
        if (nodes.isEmpty()) {
            fail<Unit>("expected body to match selector $selector")
        }
        if (nodes.none { Dom.text(it).contains(content) }) {
            fail<Unit>("expected text $content under selector $selector")
        }
    }

    fun assertSelect(selector: String, opts: MutableMap<String, Any?>) {
        assertSelect(selector)
    }

    fun assertSelect(selector: String, body: () -> Unit) {
        assertSelect(selector)
        body()
    }
}

// ── Dom primitive surface (the assertSelect substrate) ─────────────
//
// The HTML-query contract assertSelect lowers to, shared in shape with
// the Ruby/TS/Python/Rust/Elixir twins (cross-target contract in
// runtime/spinel/test/test_helper.rbs). Stub: the substring matcher
// dressed as a Dom — select fabricates one synthetic node (the whole
// document) per fragment occurrence and text returns it verbatim, so
// presence / minimum / content checks degrade to exactly the pre-
// contract behavior. The upgrade path is to swap these three functions
// for a jsoup-backed engine — real nodes, real CSS selectors —
// touching only this object; the RoundhouseTestCase call sites stay
// put.
object Dom {
    // Parse an HTML document. Stub: the document *is* its html string.
    fun parse(html: String): String = html

    // Nodes matching `selector` within `root` (a document or node).
    // Stub: one synthetic node (the root's html) per substring-fragment
    // occurrence.
    fun select(root: String, selector: String): List<String> {
        val fragment = fragmentFor(selector)
        val nodes = mutableListOf<String>()
        var from = 0
        while (true) {
            val i = root.indexOf(fragment, from)
            if (i < 0) break
            nodes.add(root)
            from = i + fragment.length
        }
        return nodes
    }

    // Concatenated descendant text of a node. Stub: the node verbatim.
    fun text(node: String): String = node

    // Loose selector → substring fragment (the stub's rule, replaced by
    // a real CSS engine on upgrade): "#id" → id="id", ".cls" → cls",
    // "tag" → <tag. Compound selectors take the first chunk.
    private fun fragmentFor(selector: String): String {
        val first = selector.split(" ").firstOrNull() ?: selector
        if (first.startsWith("#")) return "id=\"" + first.drop(1) + "\""
        if (first.startsWith(".")) return first.drop(1) + "\""
        return "<" + first
    }
}
