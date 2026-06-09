import Hummingbird
import NIOCore
import NIOPosix

// Roundhouse Swift server runtime — the primitive HTTP listener
// (Hummingbird 2.x, the locked choice). Mirrors
// `kotlin-reference/runtime/Server.kt`: parse request → dispatch to a
// controller → format the body.
//
// THE BRIDGE (plan decision 1, load-bearing): Hummingbird 2 handlers are
// async and can hop executors, which would break the thread-confined Db +
// slot state. So the handler does NO async work itself — it wraps the
// ENTIRE synchronous dispatch (controller → Db → view render) in a single
// `NIOThreadPool.runIfActive` closure, so the whole request runs on one
// stable pool thread. This is the Jetty-thread model, restored explicitly.
//
// Phase R serves only GET /articles. The full version routes through the
// transpiled `ActionDispatch::Router.match` against a routes table; here
// the one route is wired directly so the reference stays minimal while
// proving the toolchain end-to-end (Hummingbird + CSQLite + the lowered
// controller/view shape).
enum Server {
    static func start(dbPath: String, port: Int) async throws {
        Db.openProductionDb(dbPath)

        let router = Router()
        router.get("/articles") { _, _ -> Response in
            let body = try await NIOThreadPool.singleton.runIfActive {
                let controller = ArticlesController()
                controller.requestFormat = "html"
                controller.index()
                return controller.body
            }
            return Response(
                status: .ok,
                headers: [.contentType: "text/html; charset=utf-8"],
                body: .init(byteBuffer: ByteBuffer(string: body))
            )
        }

        let app = Application(
            router: router,
            configuration: .init(address: .hostname("127.0.0.1", port: port))
        )
        print("Roundhouse Swift reference listening on http://127.0.0.1:\(port)")
        try await app.runService()
    }
}
