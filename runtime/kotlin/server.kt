// Hand-written roundhouse runtime primitive (no Ruby source).
// Javalin HTTP listener — parse request -> Router.match -> instantiate
// controller -> process_action -> format response. Mirrors
// runtime/crystal/server.cr.

package roundhouse

import io.javalin.Javalin
import io.javalin.http.Context
import io.javalin.http.Cookie
import io.javalin.http.Handler

object Server {
    // Flash cookie name. Flash is cookie-backed and per-session (per
    // browser), so parallel clients never share a flash slot — the
    // "show exactly once" lifecycle lives in the transpiled Flash class
    // (Flash.toPersisted keeps only what the action set), and dispatch is
    // just the storage adapter. Mirrors go (server.go) / rust (http.rs).
    private const val FLASH_COOKIE = "rh_flash"
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
        // Mount the Action Cable /cable WebSocket on Jetty's servlet context.
        // Done at config time (not via app.ws) so the upgrade can negotiate
        // the actioncable-v1-json subprotocol — see Cable / CableServlet.
        val app = Javalin.create { config ->
            config.jetty.modifyServletContextHandler { handler -> Cable.mount(handler) }
        }
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

    // Decode the rh_flash cookie into the String-keyed map the Flash
    // constructor reloads from. Absent/empty -> empty map (first request
    // in a session carries no flash). Only the closed notice/alert key
    // set is surfaced. Values are percent-encoded (URLEncoder) so notice
    // text survives the `key=value&…` structure + cookie-octet rules.
    private fun readFlashCookie(ctx: Context): MutableMap<String, String> {
        val out: MutableMap<String, String> = mutableMapOf()
        val raw = ctx.cookie(FLASH_COOKIE) ?: return out
        for (pair in raw.split("&")) {
            val idx = pair.indexOf('=')
            if (idx <= 0) continue
            val k = pair.substring(0, idx)
            if (k != "notice" && k != "alert") continue
            val v = java.net.URLDecoder.decode(pair.substring(idx + 1), "UTF-8")
            if (v.isNotEmpty()) out[k] = v
        }
        return out
    }

    // Persist the entries the action set (Flash.toPersisted already swept
    // the show-once ones). Empty -> clear the cookie so a shown notice
    // doesn't stick. HttpOnly + Path=/ to match go/rust.
    private fun writeFlashCookie(ctx: Context, persisted: Map<String, String>) {
        if (persisted.isEmpty()) {
            ctx.removeCookie(FLASH_COOKIE, "/")
            return
        }
        val parts = mutableListOf<String>()
        for (k in listOf("notice", "alert")) {
            persisted[k]?.let { parts.add("$k=" + java.net.URLEncoder.encode(it, "UTF-8")) }
        }
        ctx.cookie(Cookie(name = FLASH_COOKIE, value = parts.joinToString("&"), path = "/", isHttpOnly = true))
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
        // Reload the flash carried from the previous request (the redirect
        // that set `flash[:notice] = …`) so views render it; the
        // constructor snapshots it as *_was so toPersisted can drop it
        // after one display.
        controller.flash = Flash(readFlashCookie(ctx))
        controller.session = Session()
        controller.processAction(match.action)

        // Carry the flash the action SET into the next request (or clear it
        // once shown). toPersisted keeps only entries changed this request,
        // so a merely-displayed notice drops out — shows exactly once.
        writeFlashCookie(ctx, controller.flash.toPersisted())

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
