// Narrowing accessors over the recursive request-params tree.
//
// `Roundhouse::ParamValue` was a per-target TYPE WITH NO OPERATIONS, so
// every consumer narrowed at its own call site with `is_a?`, and each
// emitter recognized that as an IDIOM rather than modeling the test.
// The operations live beside the type, hand-written per target the way
// `Db` is: these bodies inspect this target's representation, which is
// what makes them a primitive rather than framework logic.
//
// Semantics fixed by a Rails 8.1 oracle: `permit` keeps a blank "" and
// `Parameters#compact` drops only an explicit nil. So blank counts as
// provided; absent, JSON null, and non-scalar do not.

enum Params {
    static func sub(_ params: [String: Any?], _ key: String) -> [String: Any?] {
        if let value = params[key] ?? nil, let nested = value as? [String: Any?] {
            return nested
        }
        return [:]
    }

    static func str(_ sub: [String: Any?], _ key: String, _ fallback: String) -> String {
        if let value = sub[key] ?? nil, let s = value as? String {
            return s
        }
        return fallback
    }

    static func provided(_ sub: [String: Any?], _ key: String) -> Bool {
        if let value = sub[key] ?? nil, value is String {
            return true
        }
        return false
    }
}
