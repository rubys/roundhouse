// Roundhouse TypeScript datetime runtime — the native-`Date` seam for
// temporal (Date/DateTime/Time) columns.
//
// Storage stays portable ISO-8601 TEXT: a temporal column hydrates into a
// `string` backing field (`Db.column_text`), exactly like every other
// target. The model's synthesized reader parses that text into a native
// `Date` via `RhDateTime.parse` (see `src/emit/typescript/expr.rs`, which
// maps the `ActiveSupport.parse_db_time` intrinsic here, and the temporal
// branch in `src/emit/typescript.rs` that emits the `get <col>(): Date |
// null` getter). JSON serialization then formats a `Date` back to Rails'
// canonical `...Z` millisecond form via the `Date` branch added to
// `JsonBuilder.encode_datetime` (the string path stays for pre-formatted
// text).

export class RhDateTime {
  // Parse a stored ISO-8601 value into a native UTC `Date`. Nil-safe:
  // null / undefined / empty → null. Handles the two forms roundhouse
  // ever stores:
  //
  //   * DB-dump / seed form — "2026-05-15 21:14:56.300213" (space
  //     separator, zone-less, microsecond precision, implicitly UTC).
  //   * RFC3339 form — "2026-05-15T21:14:56Z" (what `fill_timestamps`
  //     writes via `Time.now().utc.iso8601`, and API-supplied values).
  //
  // A value that parses under neither returns null rather than throwing —
  // a malformed stored timestamp shouldn't take down a read path. A
  // zone-less value is read as UTC (a `Z` is appended before parsing) so
  // serialization emits a `Z` offset. `Date`'s ISO parser truncates
  // sub-millisecond fractional digits, matching Rails' canonicalization.
  static parse(s: string | null | undefined): Date | null {
    if (s == null || s === "") {
      return null;
    }
    let candidate = s;
    if (candidate.length > 10 && candidate[10] === " ") {
      // DB-dump form: swap the space separator for `T` and mark UTC.
      candidate = candidate.slice(0, 10) + "T" + candidate.slice(11) + "Z";
    } else if (
      candidate.includes("T") &&
      !/([zZ]|[+-]\d{2}:?\d{2})$/.test(candidate)
    ) {
      // Bare ISO datetime with no zone → treat as UTC.
      candidate = candidate + "Z";
    }
    const d = new Date(candidate);
    return isNaN(d.getTime()) ? null : d;
  }

  // Write-side sibling of `parse` — the `ActiveSupport.db_now` intrinsic.
  // Current UTC time in Rails' exact sqlite storage form:
  // "YYYY-MM-DD HH:MM:SS.ffffff" — space separator, zero-padded 6-digit
  // fractional seconds (microseconds), no zone marker (implicitly UTC,
  // byte-matching what Rails' sqlite3 adapter writes, e.g.
  // "2026-07-02 21:33:40.675251"). `fill_timestamps` stamps with it so a
  // column's TEXT values stay homogeneous — and lexicographically
  // ordered — when a roundhouse-emitted app shares a database with a
  // real Rails app. JS `Date` has millisecond resolution, so the last
  // three digits are always "000"; the shape (exactly six fractional
  // digits) is what matters.
  static dbNow(): string {
    const d = new Date();
    const pad = (n: number, w: number) => String(n).padStart(w, "0");
    return (
      pad(d.getUTCFullYear(), 4) +
      "-" +
      pad(d.getUTCMonth() + 1, 2) +
      "-" +
      pad(d.getUTCDate(), 2) +
      " " +
      pad(d.getUTCHours(), 2) +
      ":" +
      pad(d.getUTCMinutes(), 2) +
      ":" +
      pad(d.getUTCSeconds(), 2) +
      "." +
      pad(d.getUTCMilliseconds(), 3) +
      "000"
    );
  }
}
