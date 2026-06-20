import Foundation
import Hummingbird
import HummingbirdWebSocket
import NIOCore
import NIOPosix

struct DispatchResult {
    var status: Int
    var contentType: String?
    var location: String?
    var body: String
    // Flash the action SET this request (Flash.toPersisted already swept
    // the show-once ones) — the handler writes it to the rh_flash cookie.
    // Defaulted so the controller-less paths (404/error/asset) need not
    // set it; Swift's memberwise init keeps their call sites unchanged.
    var flash: [String: String] = [:]
}

enum Server {
    static func start(
        _ dbPath: String,
        _ port: Int,
        _ routes: [Route],
        _ controllers: [String: () -> ActionControllerBase],
        _ layout: @escaping (String, String?, String?) -> String
    ) async throws {
        Db.openProductionDb(dbPath)

        let hb = Hummingbird.Router()
        for method: HTTPRequest.Method in [.get, .post, .put, .patch, .delete] {
            for path in ["/", "**"] {
                hb.on(RouterPath(path), method: method) { request, _ -> Response in
                    var form: [String: String] = [:]
                    if request.method == .post || request.method == .put || request.method == .patch {
                        var buffer = try await request.body.collect(upTo: 1 << 20)
                        if let s = buffer.readString(length: buffer.readableBytes) {
                            form = parseUrlencoded(s)
                        }
                    }
                    let method = request.method.rawValue
                    let path = request.uri.path
                    let query = parseUrlencoded(request.uri.query ?? "")
                    // Flash carried from the previous request (the redirect
                    // that set `flash[:notice] = …`) rides the rh_flash
                    // cookie; reload it for view display.
                    let incomingFlash = readFlashCookie(request.headers[.cookie])
                    let r = try await NIOThreadPool.singleton.runIfActive {
                        dispatchScoped(method, path, query, form, incomingFlash, routes, controllers, layout)
                    }
                    var headers = HTTPFields()
                    if let ct = r.contentType {
                        headers[.contentType] = ct
                    }
                    if let loc = r.location {
                        headers[.location] = loc
                    }
                    // Carry the flash the action set into the next request
                    // (or clear it once shown — toPersisted returns empty for
                    // a merely-displayed notice). Storage adapter only; the
                    // show-once sweep lives in the transpiled Flash class.
                    headers[.setCookie] = flashSetCookie(r.flash)
                    return Response(
                        status: HTTPResponse.Status(code: r.status),
                        headers: headers,
                        body: .init(byteBuffer: ByteBuffer(string: r.body))
                    )
                }
            }
        }

        // The Action Cable /cable WebSocket — a separate ws router so
        // the upgrade can echo the `actioncable-v1-json` subprotocol
        // ActionCable clients require (cf. the Kotlin raw-Jetty-servlet
        // workaround; Hummingbird's shouldUpgrade hook makes it direct).
        let wsRouter = Hummingbird.Router(context: BasicWebSocketRequestContext.self)
        wsRouter.ws("/cable") { _, _ in
            return .upgrade([.secWebSocketProtocol: "actioncable-v1-json"])
        } onUpgrade: { inbound, outbound, _ in
            await Cable.handle(inbound, outbound)
        }

        let app = Application(
            router: hb,
            server: .http1WebSocketUpgrade(webSocketRouter: wsRouter),
            configuration: .init(address: .hostname("127.0.0.1", port: port))
        )
        print("Roundhouse Swift server listening on http://127.0.0.1:\(port)")
        try await app.runService()
    }

    // Per-request memory scope. NIO pool threads have no run loop, so
    // on Darwin Foundation's autoreleased bridge objects (NSString in
    // replacingOccurrences, NSRegularExpression matches) never drain
    // without an explicit pool — RSS grows linearly under load. Linux
    // Foundation has no autorelease machinery; the plain call is right.
    static func dispatchScoped(
        _ rawMethod: String,
        _ rawPath: String,
        _ query: [String: String],
        _ form: [String: String],
        _ incomingFlash: [String: String],
        _ routes: [Route],
        _ controllers: [String: () -> ActionControllerBase],
        _ layout: (String, String?, String?) -> String
    ) -> DispatchResult {
        #if canImport(ObjectiveC)
        return autoreleasepool {
            dispatch(rawMethod, rawPath, query, form, incomingFlash, routes, controllers, layout)
        }
        #else
        return dispatch(rawMethod, rawPath, query, form, incomingFlash, routes, controllers, layout)
        #endif
    }

    // The whole synchronous request — runs on one pool thread.
    static func dispatch(
        _ rawMethod: String,
        _ rawPath: String,
        _ query: [String: String],
        _ form: [String: String],
        _ incomingFlash: [String: String],
        _ routes: [Route],
        _ controllers: [String: () -> ActionControllerBase],
        _ layout: (String, String?, String?) -> String
    ) -> DispatchResult {
        ViewHelpers.resetSlotsBang()

        // Compiled assets (/assets/tailwind.css, …) — before route
        // dispatch so the greedy router doesn't 404 them.
        if rawMethod == "GET" && rawPath.hasPrefix("/assets/") {
            return serveAsset(String(rawPath.dropFirst("/assets/".count)))
        }

        // Rails' `_method` override (button_to delete/patch forms POST).
        var method = rawMethod
        if method == "POST", let override = form["_method"] {
            method = override.uppercased()
        }

        // A `.json` extension selects the JSON variant.
        var path = rawPath
        var format = "html"
        if path.hasSuffix(".json") {
            format = "json"
            path = String(path.dropLast(5))
        }

        guard let match = Router.match(method, path, routes),
              let factory = controllers[match.controller]
        else {
            return DispatchResult(status: 404, contentType: "text/plain", location: nil, body: "Not Found")
        }

        var params: [String: Any?] = [:]
        for (k, v) in match.pathParams {
            params[k] = v
        }
        for (k, v) in query {
            setParam(&params, k, v)
        }
        for (k, v) in form {
            setParam(&params, k, v)
        }

        let controller = factory()
        controller.params = params
        controller.requestFormat = format
        controller.requestMethod = method
        controller.requestPath = path
        controller.flash = Flash(incomingFlash)
        controller.session = Session()
        do {
            try controller.processAction(match.action)
        } catch is RecordNotFound {
            return DispatchResult(status: 404, contentType: "text/plain", location: nil, body: "Not Found")
        } catch is RecordInvalid {
            return DispatchResult(status: 422, contentType: "text/plain", location: nil, body: "Unprocessable Entity")
        } catch {
            return DispatchResult(status: 500, contentType: "text/plain", location: nil, body: "Internal Server Error")
        }

        if let location = controller.location {
            return DispatchResult(status: controller.status, contentType: nil, location: location, body: "", flash: controller.flash.toPersisted())
        }
        if controller.requestFormat == "json" {
            return DispatchResult(status: controller.status, contentType: "application/json", location: nil, body: controller.body, flash: controller.flash.toPersisted())
        }
        return DispatchResult(
            status: controller.status,
            contentType: "text/html; charset=utf-8",
            location: nil,
            body: layout(controller.body, controller.flash.notice, controller.flash.alert),
            flash: controller.flash.toPersisted()
        )
    }

    // `article[title]=Foo` → a nested `[String: Any?]`; a bare key → a
    // scalar String. Untyped params stay Any? so `from_raw`'s
    // `is_a?(Hash)` / `is_a?(String)` narrowing matches real values.
    static func setParam(_ params: inout [String: Any?], _ key: String, _ value: String) {
        if let open = key.firstIndex(of: "["), key.hasSuffix("]") {
            let outer = String(key[key.startIndex..<open])
            let inner = String(key[key.index(after: open)..<key.index(before: key.endIndex)])
            var dict = (params[outer] as? [String: Any?]) ?? [:]
            dict[inner] = value
            params[outer] = dict
        } else {
            params[key] = value
        }
    }

    static func parseUrlencoded(_ s: String) -> [String: String] {
        var out: [String: String] = [:]
        for pair in s.split(separator: "&") {
            let parts = pair.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
            guard let rawKey = parts.first else { continue }
            let rawValue = parts.count > 1 ? parts[1] : ""
            let decode = { (x: Substring) -> String in
                x.replacingOccurrences(of: "+", with: " ").removingPercentEncoding
                    ?? String(x)
            }
            out[decode(rawKey)] = decode(rawValue)
        }
        return out
    }

    // ── flash: cookie-backed, per-session storage adapter ──────────
    // Flash is cookie-backed and per-session (per browser), so parallel
    // clients never share a flash slot; the "show exactly once" lifecycle
    // lives in the transpiled Flash class (toPersisted keeps only what the
    // action set). Mirrors go (server.go) / kotlin (Server.dispatch).

    // Unreserved set — anything else in a flash value is percent-encoded so
    // the `key=value&…` structure and cookie-octet rules survive arbitrary
    // notice text.
    static let flashValueAllowed = CharacterSet(
        charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.~"
    )

    // Decode the rh_flash cookie out of the request's Cookie header into the
    // map Flash(_:) reloads from. Absent/empty → empty (first request in a
    // session carries no flash). Only the closed notice/alert key set.
    static func readFlashCookie(_ cookieHeader: String?) -> [String: String] {
        var out: [String: String] = [:]
        guard let raw = cookieHeader else { return out }
        for jar in raw.split(separator: ";") {
            let trimmed = jar.trimmingCharacters(in: .whitespaces)
            guard trimmed.hasPrefix("rh_flash=") else { continue }
            let val = String(trimmed.dropFirst("rh_flash=".count))
            for kv in val.split(separator: "&") {
                let parts = kv.split(separator: "=", maxSplits: 1)
                guard parts.count == 2 else { continue }
                let k = String(parts[0])
                guard k == "notice" || k == "alert" else { continue }
                let v = String(parts[1]).removingPercentEncoding ?? String(parts[1])
                if !v.isEmpty { out[k] = v }
            }
        }
        return out
    }

    // Build the rh_flash Set-Cookie value. Empty → a clearing cookie
    // (Max-Age=0) so a notice shown once doesn't stick. HttpOnly + Path=/
    // to match go/kotlin.
    static func flashSetCookie(_ persisted: [String: String]) -> String {
        if persisted.isEmpty {
            return "rh_flash=; Path=/; Max-Age=0; HttpOnly"
        }
        var parts: [String] = []
        for k in ["notice", "alert"] {
            if let v = persisted[k] {
                let enc = v.addingPercentEncoding(withAllowedCharacters: flashValueAllowed) ?? v
                parts.append("\(k)=\(enc)")
            }
        }
        return "rh_flash=\(parts.joined(separator: "&")); Path=/; HttpOnly"
    }

    // Serve a file from static/assets/, content-typed by extension.
    // Path-traversal guarded; 404 when missing so an archive with no
    // built assets still boots.
    static func serveAsset(_ rel: String) -> DispatchResult {
        let root = URL(fileURLWithPath: "static/assets").standardizedFileURL
        let file = root.appendingPathComponent(rel).standardizedFileURL
        guard file.path.hasPrefix(root.path),
              let data = try? Data(contentsOf: file),
              let body = String(data: data, encoding: .utf8)
        else {
            return DispatchResult(status: 404, contentType: "text/plain", location: nil, body: "Not Found")
        }
        let contentType: String
        switch file.pathExtension.lowercased() {
        case "css": contentType = "text/css"
        case "js", "mjs": contentType = "application/javascript"
        case "json", "map": contentType = "application/json"
        case "svg": contentType = "image/svg+xml"
        default: contentType = "application/octet-stream"
        }
        return DispatchResult(status: 200, contentType: contentType, location: nil, body: body)
    }
}
