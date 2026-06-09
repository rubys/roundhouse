//! Hand-written Swift primitives — the bottom layer the transpiled
//! framework runtime and lowered app code call into. The analog of
//! `src/emit/kotlin/primitives.rs` (and of `swift-reference/`'s
//! `runtime/` directory, which is the verified template). Grown one
//! primitive at a time as the transpiled runtime needs them.

use std::path::PathBuf;

use crate::emit::EmittedFile;

// String helpers with no clean inline-emit idiom. `gsubMap` is the
// regex-replace-with-lookup-table JsonBuilder's escaping uses;
// `gsub` is the plain regex template replace.
const RHSTRING_SWIFT: &str = r#"import Foundation

enum RhString {
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
"#;

/// The hand-written primitive files, emitted under `Sources/App/runtime/`.
pub fn primitives() -> Vec<EmittedFile> {
    vec![EmittedFile {
        path: PathBuf::from("Sources/App/runtime/RhString.swift"),
        content: RHSTRING_SWIFT.to_string(),
    }]
}
