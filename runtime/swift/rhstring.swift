import Foundation

enum RhString {
    // Ruby `to_s` semantics for untyped values: nil → "", optionals
    // unwrap (recursively — `Any` can box nested optionals, which
    // String(describing:)/interpolation would render as "Optional(…)").
    static func s(_ x: Any?) -> String {
        guard let x = x else { return "" }
        let m = Mirror(reflecting: x)
        if m.displayStyle == .optional {
            guard let child = m.children.first else { return "" }
            return s(child.value)
        }
        return "\(x)"
    }

    // Ruby `str.gsub(regex, map)`: each match is replaced by its map
    // entry (identity when absent).
    static func gsubMap(_ s: String, _ pattern: NSRegularExpression, _ map: [String: String]) -> String {
        let ns = s as NSString
        var result = ""
        var last = 0
        for m in pattern.matches(in: s, range: NSRange(location: 0, length: ns.length)) {
            result += ns.substring(with: NSRange(location: last, length: m.range.location - last))
            let matched = ns.substring(with: m.range)
            result += map[matched] ?? matched
            last = m.range.location + m.range.length
        }
        result += ns.substring(from: last)
        return result
    }

    // Ruby `str.gsub(regex, replacement)`.
    static func gsub(_ s: String, _ pattern: NSRegularExpression, _ replacement: String) -> String {
        let ns = s as NSString
        return pattern.stringByReplacingMatches(
            in: s,
            range: NSRange(location: 0, length: ns.length),
            withTemplate: replacement
        )
    }
}
