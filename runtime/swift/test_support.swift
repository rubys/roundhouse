import XCTest
@testable import App

struct RhTestFailure: Error, CustomStringConvertible {
    let message: String
    init(_ message: String) { self.message = message }
    var description: String { message }
}

class RoundhouseTestCase: XCTestCase {
    var __status: Int = 200
    var __body: String = ""
    var __location: String = ""
    var __flash = Flash()
    var __session = Session()

    override func setUpWithError() throws {
        if !RoundhouseTestSetup.schemaSql.isEmpty {
            Db.setupTestDb(RoundhouseTestSetup.schemaSql)
            for loader in RoundhouseTestSetup.fixtureLoaders {
                try loader()
            }
        }
        ViewHelpers.resetSlotsBang()
        __flash = Flash()
        __session = Session()
    }

    // ── controller-test dispatch ─────────────────────────────────

    func get(_ path: String) {
        performRequest("GET", path, [:])
    }

    func post(_ path: String, _ opts: [String: Any?] = [:]) {
        performRequest("POST", path, (opts["params"] as? [String: Any?]) ?? [:])
    }

    func patch(_ path: String, _ opts: [String: Any?] = [:]) {
        performRequest("PATCH", path, (opts["params"] as? [String: Any?]) ?? [:])
    }

    func delete(_ path: String, _ opts: [String: Any?] = [:]) {
        performRequest("DELETE", path, (opts["params"] as? [String: Any?]) ?? [:])
    }

    private func performRequest(_ method: String, _ path: String, _ params: [String: Any?]) {
        ViewHelpers.resetSlotsBang()
        guard let match = Router.match(method, path, RoundhouseTestSetup.routes) else {
            XCTFail("no route for \(method) \(path)")
            return
        }
        guard let factory = RoundhouseTestSetup.controllers[match.controller] else {
            XCTFail("no controller registered for \(match.controller)")
            return
        }
        var merged: [String: Any?] = params
        for (k, v) in match.pathParams {
            merged[k] = v
        }
        let controller = factory()
        controller.params = merged
        controller.requestFormat = "html"
        controller.requestMethod = method
        controller.requestPath = path
        controller.flash = __flash
        controller.session = __session
        do {
            try controller.processAction(match.action)
        } catch is RecordNotFound {
            __status = 404
            __body = ""
            __location = ""
            return
        } catch {
            XCTFail("processAction threw: \(error)")
            return
        }
        __status = controller.status
        __body = controller.body
        __location = controller.location ?? ""
        __flash = controller.flash
    }

    // ── HTTP response assertions ─────────────────────────────────

    private static let statusRanges: [String: ClosedRange<Int>] = [
        "success": 200...299,
        "redirect": 300...399,
        "missing": 404...404,
        "not_found": 404...404,
        "error": 500...599,
        "ok": 200...200,
        "created": 201...201,
        "no_content": 204...204,
        "moved_permanently": 301...301,
        "found": 302...302,
        "see_other": 303...303,
        "bad_request": 400...400,
        "unauthorized": 401...401,
        "forbidden": 403...403,
        "unprocessable_entity": 422...422,
        "unprocessable_content": 422...422,
        "internal_server_error": 500...500,
    ]

    func assertResponse(_ expected: String) {
        guard let range = Self.statusRanges[expected] else {
            XCTFail("unknown response expectation \(expected)")
            return
        }
        if !range.contains(__status) {
            XCTFail("expected response \(expected), got status=\(__status) body=\(__body.prefix(200))")
        }
    }

    func assertRedirectedTo(_ expectedPath: String) {
        if __status < 300 || __status >= 400 {
            XCTFail("expected a redirect, got status=\(__status) location=\(__location)")
            return
        }
        if !__location.contains(expectedPath) {
            XCTFail("expected Location to contain \(expectedPath), got \(__location)")
        }
    }

    // `assert_select` substring shim: match on the opening tag or the
    // id="x" fragment derived from the selector. Rough but effective
    // for the scaffold-blog HTML shapes; cardinality kwargs are
    // best-effort no-ops (same loose semantics as the crystal/ts shims).
    func assertSelect(_ selector: String) {
        let fragment = selectorFragment(selector)
        if !__body.contains(fragment) {
            XCTFail("expected body to match selector \(selector) (looked for \(fragment))")
        }
    }

    func assertSelect(_ selector: String, _ content: String) {
        assertSelect(selector)
        if !__body.contains(content) {
            XCTFail("expected body to contain \(content) matching selector \(selector)")
        }
    }

    func assertSelect(_ selector: String, _ opts: [String: Any?]) {
        assertSelect(selector)
    }

    func assertSelect(_ selector: String, _ body: () throws -> Void) rethrows {
        assertSelect(selector)
        try body()
    }

    private func selectorFragment(_ selector: String) -> String {
        let first = selector.split(separator: " ").first.map(String.init) ?? selector
        if first.hasPrefix("#") {
            return "id=\"\(first.dropFirst())\""
        }
        if first.hasPrefix(".") {
            return "\(first.dropFirst())\""
        }
        return "<\(first)"
    }
}
