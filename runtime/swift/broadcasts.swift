enum Broadcasts {
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
    static func turbo_stream_fragment(_ action: String, _ target: String, _ html: String, _ attributes: String = "") -> String {
        if action == "remove" {
            return "<turbo-stream\(attributes) action=\"remove\" target=\"\(target)\"></turbo-stream>"
        }
        return "<turbo-stream\(attributes) action=\"\(action)\" target=\"\(target)\"><template>\(html)</template></turbo-stream>"
    }

    static func append(_ args: [String: Any?]) { record("append", args) }
    static func prepend(_ args: [String: Any?]) { record("prepend", args) }
    static func replace(_ args: [String: Any?]) { record("replace", args) }
    static func remove(_ args: [String: Any?]) { record("remove", args) }

    private static func record(_ action: String, _ opts: [String: Any?]) {
        guard let stream = opts["stream"] as? String else { return }
        let target = (opts["target"] as? String) ?? ""
        let html = (opts["html"] as? String) ?? ""
        // Rendered attribute text (` maintain_scroll="true"`) the
        // broadcast lowering composed. Read here so an app that writes
        // `attributes:` is not silently stripped of it.
        let attributes = (opts["attributes"] as? String) ?? ""
        Cable.dispatch(stream, Cable.turboStreamHtml(action, target, html, attributes))
    }
}
