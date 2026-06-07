package roundhouse

import io.javalin.Javalin

// Roundhouse Kotlin server runtime — the primitive HTTP listener
// (Javalin, the locked choice). Mirrors `runtime/crystal/server.cr`:
// parse request → dispatch to a controller → format the body.
//
// Phase R serves only GET /articles. The full version routes through the
// transpiled `ActionDispatch::Router.match` against a routes table; here
// the one route is wired directly so the reference stays minimal while
// proving the toolchain end-to-end (Javalin + xerial JDBC + the lowered
// controller/view shape).
object Server {
    fun start(dbPath: String, port: Int) {
        Db.openProductionDb(dbPath)

        val app = Javalin.create()
        app.get("/articles") { ctx ->
            val controller = ArticlesController()
            controller.requestFormat = "html"
            controller.index()
            ctx.html(controller.body)
        }
        app.start("127.0.0.1", port)
        println("Roundhouse Kotlin reference listening on http://127.0.0.1:$port")
    }
}
