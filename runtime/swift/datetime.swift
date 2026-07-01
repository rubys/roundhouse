// Roundhouse Swift datetime runtime — the native-`Date` seam for
// temporal (Date/DateTime/Time) columns.
//
// Storage stays portable ISO-8601 TEXT: a temporal column hydrates into a
// `String` backing property (`Db.columnText`), exactly like every other
// target. The model's synthesized reader parses that text into a native
// Foundation `Date` via `Roundhouse.RhDateTime.parse` (see
// `src/emit/swift/expr.rs`, which maps the `ActiveSupport.parse_db_time`
// intrinsic here). JSON serialization then formats a `Date` back to Rails'
// canonical `...Z` millisecond form via the `JsonBuilder.encodeDatetime(Date?)`
// overload below (the existing `encodeDatetime(String?)` keeps the
// pre-formatted-text path).

import Foundation

enum Roundhouse {
    enum RhDateTime {
        // Parse a stored ISO-8601 value into a native UTC `Date`. Nil-safe:
        // nil / empty → nil. Handles the two forms roundhouse ever stores:
        //
        //   * DB-dump / seed form — "2026-05-15 21:14:56.300213" (space
        //     separator, zone-less, microsecond precision, implicitly UTC).
        //   * RFC3339 form — "2026-05-15T21:14:56Z" (what `fill_timestamps`
        //     writes via `Time.now().utc.iso8601`, and API-supplied values).
        //
        // A value that parses under neither returns nil rather than
        // trapping — a malformed stored timestamp shouldn't take down a
        // read path.
        static func parse(_ s: String?) -> Date? {
            guard let s = s, !s.isEmpty else { return nil }
            // DB-dump form: a space at index 10 separates date from time.
            if s.count > 10 {
                let sep = s.index(s.startIndex, offsetBy: 10)
                if s[sep] == " " {
                    let base = String(s.prefix(19)) // "2026-05-15 21:14:56"
                    let fmt = DateFormatter()
                    fmt.locale = Locale(identifier: "en_US_POSIX")
                    fmt.timeZone = TimeZone(identifier: "UTC")
                    fmt.dateFormat = "yyyy-MM-dd HH:mm:ss"
                    guard let whole = fmt.date(from: base) else { return nil }
                    // Fractional seconds after the dot, added as a
                    // TimeInterval (robust vs DateFormatter fractional
                    // quirks): ".300213" → +0.300213s.
                    if s.count > 20,
                       s[s.index(s.startIndex, offsetBy: 19)] == "." {
                        let digits = s.dropFirst(20).prefix(while: { $0.isNumber })
                        if let frac = Double("0." + digits) {
                            return whole.addingTimeInterval(frac)
                        }
                    }
                    return whole
                }
            }
            // RFC3339 / ISO8601 form (optionally fractional).
            let iso = ISO8601DateFormatter()
            iso.timeZone = TimeZone(identifier: "UTC")
            iso.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            if let d = iso.date(from: s) { return d }
            iso.formatOptions = [.withInternetDateTime]
            return iso.date(from: s)
        }
    }
}

extension JsonBuilder {
    // `Date` overload of `encodeDatetime`. The transpiled `String?` version
    // (JsonBuilder.swift, from the shared runtime) handles pre-formatted
    // text; this one formats a native `Date` — what a temporal column's
    // reader yields — to Rails' canonical JSON shape: UTC, millisecond
    // precision, `Z` suffix (e.g. "2026-05-15T21:14:56.300Z"). The compare
    // harness canonicalizes Rails' microsecond precision down to
    // milliseconds, so this matches byte-for-byte.
    static func encodeDatetime(_ t: Date?) -> String {
        guard let t = t else { return "null" }
        let fmt = ISO8601DateFormatter()
        fmt.timeZone = TimeZone(identifier: "UTC")
        fmt.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return "\"\(fmt.string(from: t))\""
    }
}
