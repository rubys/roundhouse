import Foundation

// Subset of ActionView::ViewHelpers the index/_article views call. Also
// normally transpiled from `runtime/ruby/action_view/view_helpers.rb`;
// hand-written minimal here for Phase R. dom_id is specialized to Article
// (the full version is generic over any record); turbo_stream_from and the
// csrf/method inputs are simplified — exact Rails byte-parity is the
// `compare` gate's concern, not the Phase R gate.
//
// `contentFor` is shared mutable module state; the emitter-phase design
// makes such slots per-thread (ThreadSpecificVariable, the Kotlin
// ThreadLocal-slots fix) — kept plain here to match the Kotlin Phase R
// reference, and it is write-only on the GET /articles path.
enum ViewHelpers {
    private static var contentFor: [String: String] = [:]

    static func htmlEscape(_ s: String) -> String {
        s.replacingOccurrences(of: "&", with: "&amp;")
            .replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: ">", with: "&gt;")
            .replacingOccurrences(of: "\"", with: "&quot;")
            .replacingOccurrences(of: "'", with: "&#39;")
    }

    static func domId(_ record: Article, _ prefix: String? = nil) -> String {
        if let prefix = prefix {
            return "\(prefix)_article_\(record.id)"
        }
        return "article_\(record.id)"
    }

    static func truncate(_ text: String, _ length: Int = 30) -> String {
        if text.count <= length { return text }
        return String(text.prefix(length - 3)) + "..."
    }

    static func turboStreamFrom(_ name: String) -> String {
        "<turbo-cable-stream-source channel=\"Turbo::StreamsChannel\" "
            + "signed-stream-name=\"\(name)\"></turbo-cable-stream-source>"
    }

    static func contentForSet(_ key: String, _ value: String) {
        contentFor[key] = value
    }

    static func methodOverrideInput(_ method: String) -> String {
        "<input type=\"hidden\" name=\"_method\" value=\"\(method)\" autocomplete=\"off\">"
    }

    static func csrfTokenHiddenInput() -> String {
        "<input type=\"hidden\" name=\"authenticity_token\" value=\"\" autocomplete=\"off\">"
    }
}
