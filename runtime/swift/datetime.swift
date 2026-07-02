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
//
// PERFORMANCE: no per-call formatter construction. On corelibs-Foundation
// (Linux) building a `DateFormatter`/`ISO8601DateFormatter` costs on the
// order of milliseconds, which is catastrophic on hot read/serialize paths.
// The two `ISO8601DateFormatter` variants are cached as statics (the class
// is documented thread-safe); the fixed-layout conversions ("yyyy-MM-dd
// HH:mm:ss" parse, `dbNow`, JSON encode) avoid Foundation formatters
// entirely — `DateFormatter` thread-safety on corelibs-Foundation is not
// trustworthy — and use pure integer civil-calendar arithmetic (Howard
// Hinnant's days-from-civil / civil-from-days), which is deterministic,
// locale-free, and fastest.

import Foundation

enum Roundhouse {
    enum RhDateTime {
        // Cached ISO8601 formatters for the RFC3339 fallback branch of
        // `parse`. `ISO8601DateFormatter` is documented thread-safe, so a
        // shared static per option-variant is fine.
        static let isoFractional: ISO8601DateFormatter = {
            let f = ISO8601DateFormatter()
            f.timeZone = TimeZone(identifier: "UTC")
            f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            return f
        }()
        static let isoWhole: ISO8601DateFormatter = {
            let f = ISO8601DateFormatter()
            f.timeZone = TimeZone(identifier: "UTC")
            f.formatOptions = [.withInternetDateTime]
            return f
        }()

        // days-from-civil (Howard Hinnant): proleptic-Gregorian
        // (year, month, day) → days since the Unix epoch (1970-01-01).
        fileprivate static func daysFromCivil(_ y: Int, _ m: Int, _ d: Int) -> Int {
            let yy = y - (m <= 2 ? 1 : 0)
            let era = (yy >= 0 ? yy : yy - 399) / 400
            let yoe = yy - era * 400                                    // [0, 399]
            let doy = (153 * (m + (m > 2 ? -3 : 9)) + 2) / 5 + d - 1    // [0, 365]
            let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy             // [0, 146096]
            return era * 146097 + doe - 719468
        }

        // civil-from-days (Howard Hinnant): days since the Unix epoch →
        // proleptic-Gregorian (year, month, day).
        fileprivate static func civilFromDays(_ z: Int) -> (Int, Int, Int) {
            let zz = z + 719468
            let era = (zz >= 0 ? zz : zz - 146096) / 146097
            let doe = zz - era * 146097                                 // [0, 146096]
            let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365 // [0, 399]
            let y = yoe + era * 400
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100)           // [0, 365]
            let mp = (5 * doy + 2) / 153                                // [0, 11]
            let d = doy - (153 * mp + 2) / 5 + 1                        // [1, 31]
            let m = mp + (mp < 10 ? 3 : -9)                             // [1, 12]
            return (y + (m <= 2 ? 1 : 0), m, d)
        }

        // (year, month, day, hour, min, sec) — UTC — → `Date`. The pure-
        // arithmetic replacement for the old per-call `DateFormatter` in
        // `parse`'s DB-dump branch.
        fileprivate static func dateFromCivil(
            _ y: Int, _ mo: Int, _ d: Int, _ h: Int, _ mi: Int, _ s: Int
        ) -> Date {
            let days = daysFromCivil(y, mo, d)
            return Date(timeIntervalSince1970: Double(days * 86_400 + h * 3_600 + mi * 60 + s))
        }

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
                    // Fixed layout "yyyy-MM-dd HH:mm:ss" — decode with
                    // integer arithmetic (no Foundation formatter; see the
                    // header comment). Malformed → nil, never trap.
                    guard s.count >= 19 else { return nil }
                    let chars = Array(s)
                    guard chars[4] == "-", chars[7] == "-",
                          chars[13] == ":", chars[16] == ":" else { return nil }
                    func num(_ r: Range<Int>) -> Int? { Int(String(chars[r])) }
                    guard let y = num(0..<4), let mo = num(5..<7), let d = num(8..<10),
                          let h = num(11..<13), let mi = num(14..<16), let sec = num(17..<19),
                          (1...12).contains(mo), (1...31).contains(d),
                          (0...23).contains(h), (0...59).contains(mi), (0...59).contains(sec)
                    else { return nil }
                    let whole = dateFromCivil(y, mo, d, h, mi, sec)
                    // Fractional seconds after the dot, added as a
                    // TimeInterval: ".300213" → +0.300s.
                    //
                    // TRUNCATE to milliseconds (3 digits) here, matching how
                    // Rails/`Time#iso8601(3)` and the compare harness reduce
                    // sub-second precision. Foundation `Date` is a `Double`,
                    // and the `encodeDatetime(Date)` serializer below rounds
                    // to the nearest millisecond — so if we kept the full
                    // microseconds, ".456789" would round UP to ".457" while
                    // Rails truncates to ".456", a 1 ms mismatch on ~half of
                    // all timestamps. Truncating the string first makes the
                    // parsed `Date` hold a clean ms value that round-trips.
                    if s.count > 20, chars[19] == "." {
                        let digits = s.dropFirst(20).prefix(while: { $0.isNumber }).prefix(3)
                        if let frac = Double("0." + digits) {
                            return whole.addingTimeInterval(frac)
                        }
                    }
                    return whole
                }
            }
            // RFC3339 / ISO8601 form (optionally fractional).
            if let d = isoFractional.date(from: s) { return d }
            return isoWhole.date(from: s)
        }

        // Temporal writer intrinsic target: `ActiveSupport.db_now` →
        // current UTC time in Rails' exact sqlite storage form —
        // "YYYY-MM-DD HH:MM:SS.ffffff": space separator, zero-padded
        // 6-digit fractional seconds (microseconds), no zone marker (e.g.
        // "2026-07-02 21:33:40.675251"). `fill_timestamps` stamps with it
        // so a column's TEXT values stay homogeneous — and
        // lexicographically ordered — when a roundhouse-emitted app shares
        // a database with a real Rails app. (`Date` is a `Double` with
        // ~0.2µs resolution at the current epoch, so 6-digit formatting is
        // meaningful; sub-µs exactness doesn't matter since the value is
        // "now".)
        static func dbNow() -> String {
            let micros = Int((Date().timeIntervalSince1970 * 1_000_000).rounded())
            var days = micros / 86_400_000_000
            var rem = micros % 86_400_000_000
            if rem < 0 { rem += 86_400_000_000; days -= 1 }
            let (y, mo, d) = civilFromDays(days)
            return String(
                format: "%04d-%02d-%02d %02d:%02d:%02d.%06d",
                y, mo, d,
                rem / 3_600_000_000, (rem / 60_000_000) % 60,
                (rem / 1_000_000) % 60, rem % 1_000_000
            )
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
    // milliseconds, so this matches byte-for-byte. Formats via the same
    // civil-calendar arithmetic as `RhDateTime` (no per-call formatter;
    // rounds to the nearest millisecond, exactly like the
    // `ISO8601DateFormatter` it replaces).
    static func encodeDatetime(_ t: Date?) -> String {
        guard let t = t else { return "null" }
        let ms = Int((t.timeIntervalSince1970 * 1_000).rounded())
        var days = ms / 86_400_000
        var rem = ms % 86_400_000
        if rem < 0 { rem += 86_400_000; days -= 1 }
        let (y, mo, d) = Roundhouse.RhDateTime.civilFromDays(days)
        let s = String(
            format: "%04d-%02d-%02dT%02d:%02d:%02d.%03dZ",
            y, mo, d,
            rem / 3_600_000, (rem / 60_000) % 60, (rem / 1_000) % 60, rem % 1_000
        )
        return "\"\(s)\""
    }
}
