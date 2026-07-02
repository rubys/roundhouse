// Roundhouse Rust datetime runtime — the native-`chrono::DateTime<Utc>`
// seam for temporal (Date/DateTime/Time) columns.
//
// Storage stays portable ISO-8601 TEXT: a temporal column hydrates into
// a `String` ivar (`column_text`), exactly like every other target. The
// model's synthesized reader parses that text into a native
// `chrono::DateTime<Utc>` via `parse_db_time` (see the
// `ActiveSupport.parse_db_time` mapping in
// `src/emit/rust2/expr/send/mod.rs`). JSON serialization then formats a
// `DateTime<Utc>` back to Rails' canonical `...Z` millisecond form.
//
// Rust has no ad-hoc overloading, so `JsonBuilder::encode_datetime` (the
// call site in emitted JSON views) is generic over the `EncodeDatetime`
// trait, implemented for BOTH `Option<String>` (the stored-text form,
// preserving the shared runtime's micro→milli reformat) and
// `Option<chrono::DateTime<Utc>>` (the native reader form). This keeps
// every existing call site compiling without change.

use chrono::{DateTime, NaiveDateTime, Utc};

/// Parse a stored ISO-8601 value into a native UTC `DateTime`. Nil-safe:
/// an empty stored value (SQL NULL hydrates as `""`, never a Rust
/// `Option::None` at the ivar) → `None`. Handles the two forms
/// roundhouse ever stores:
///
///   * DB-dump / seed form — `"2026-05-15 21:14:56.300213"` (space
///     separator, zone-less, microsecond precision, implicitly UTC).
///   * RFC3339 form — `"2026-05-15T21:14:56Z"` (what `fill_timestamps`
///     writes, and API-supplied values).
///
/// A value that parses under neither returns `None` rather than
/// panicking — a malformed stored timestamp shouldn't take down a read
/// path.
pub fn parse_db_time(s: &str) -> Option<DateTime<Utc>> {
    if s.is_empty() {
        return None;
    }
    // DB-dump / seed form: space separator, zone-less UTC. The `%.f`
    // fraction is optional, so this covers both `...:56.300213` and a
    // bare `...:56`; the explicit no-fraction format below is a belt-
    // and-suspenders fallback for chrono versions that treat `%.f` as
    // requiring the dot.
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
        return Some(ndt.and_utc());
    }
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(ndt.and_utc());
    }
    // RFC3339 form (T separator, explicit offset / Z).
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    None
}

/// Write-side sibling of `parse_db_time` — the `ActiveSupport.db_now`
/// intrinsic (see the mapping in `src/emit/rust2/expr/send/mod.rs`).
/// Returns the current UTC time in Rails' exact sqlite storage form:
///
///   "YYYY-MM-DD HH:MM:SS.ffffff"
///
/// — space separator, zero-padded 6-digit fractional seconds
/// (microseconds), no zone marker. E.g. `"2026-07-02 21:33:40.675251"`.
/// `fill_timestamps` stamps with this so a column's TEXT values stay
/// homogeneous — and lexicographically ordered — when a
/// roundhouse-emitted app shares a database with a real Rails app.
/// chrono's `%.6f` renders the fraction *including* the leading dot.
pub fn db_now() -> String {
    Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string()
}

/// Trait backing the generic `JsonBuilder::encode_datetime`. Formats a
/// temporal value to Rails' canonical JSON shape (UTC, millisecond
/// precision, `Z` suffix, quoted) or the literal `null`.
pub trait EncodeDatetime {
    fn rh_encode_datetime(self) -> String;
}

/// Native reader form: a temporal column's getter yields
/// `Option<DateTime<Utc>>`. Format to `"2026-05-15T21:14:56.300Z"`.
impl EncodeDatetime for Option<DateTime<Utc>> {
    fn rh_encode_datetime(self) -> String {
        match self {
            None => "null".to_string(),
            Some(dt) => format!("\"{}\"", dt.to_utc().format("%Y-%m-%dT%H:%M:%S%.3fZ")),
        }
    }
}

/// Stored-text form: pre-formatted ISO-8601 text (e.g. a value passed
/// straight through from the DB or a hand-built JSON node). Mirrors the
/// shared `runtime/ruby/json_builder.rb` `encode_datetime` logic —
/// slice out date + `HH:MM:SS`, pad/truncate the fraction to exactly 3
/// digits, and re-emit as `"<date>T<time>.<ms>Z"`. A too-short value is
/// escaped and quoted verbatim.
impl EncodeDatetime for Option<String> {
    fn rh_encode_datetime(self) -> String {
        let Some(s) = self else {
            return "null".to_string();
        };
        if s.len() < 19 {
            return format!("\"{}\"", crate::json_builder::JsonBuilder::encode_string(&s));
        }
        let date = &s[0..10];
        let time = &s[11..19];
        let mut ms = "000".to_string();
        if s.len() > 20 && &s[19..20] == "." {
            let frac = &s[20..];
            let padded = format!("{frac}000");
            ms = padded[0..3].to_string();
        }
        format!("\"{date}T{time}.{ms}Z\"")
    }
}
