enum Broadcasts {
    static func append(_ args: [String: Any?]) { record("append", args) }
    static func prepend(_ args: [String: Any?]) { record("prepend", args) }
    static func replace(_ args: [String: Any?]) { record("replace", args) }
    static func remove(_ args: [String: Any?]) { record("remove", args) }

    private static func record(_ action: String, _ opts: [String: Any?]) {
        guard let stream = opts["stream"] as? String else { return }
        let target = (opts["target"] as? String) ?? ""
        let html = (opts["html"] as? String) ?? ""
        Cable.dispatch(stream, Cable.turboStreamHtml(action, target, html))
    }
}
