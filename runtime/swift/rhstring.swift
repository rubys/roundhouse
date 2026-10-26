import Foundation

enum RhString {
    // Ruby's String#[](index): a one-character String, nil past the end
    static func at(_ s: String, _ index: Int) -> String? {
        let chars = Array(s)
        let i = index < 0 ? chars.count + index : index
        return i < 0 || i >= chars.count ? nil : String(chars[i])
    }

    // Ruby's Array#[](start, length): a negative start counts from the end, nil past the end, clamped otherwise
    static func listSlice<T>(_ list: [T], _ start: Int, _ length: Int) -> [T]? {
        let b = start < 0 ? start + list.count : start
        if b < 0 || b > list.count || length < 0 { return nil }
        return Array(list[b..<(b + min(length, list.count - b))])
    }

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

    // Ruby `String#match?(re)` / `Regexp#match?(str)` — true when the
    // pattern finds a match. NSRegularExpression has no instance
    // predicate that takes just a String, so the emit bridges both
    // orientations through this helper.
    static func matchPred(_ s: String, _ pattern: NSRegularExpression) -> Bool {
        let ns = s as NSString
        return pattern.firstMatch(
            in: s,
            options: [],
            range: NSRange(location: 0, length: ns.length)
        ) != nil
    }
}
