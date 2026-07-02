using System;
using System.Globalization;

namespace Roundhouse;

// Roundhouse C# datetime runtime — the native-`DateTimeOffset` seam for
// temporal (Date/DateTime/Time) columns.
//
// Storage stays portable ISO-8601 TEXT: a temporal column hydrates into a
// `string` backing field (`Db.ColumnText`), exactly like every other target.
// The model's synthesized reader parses that text into a native
// `DateTimeOffset` via `RhDateTime.Parse` (see `src/emit/csharp/library.rs`
// for the getter, and `src/emit/csharp/expr.rs`, which maps the
// `ActiveSupport.parse_db_time` intrinsic here). JSON serialization then
// formats a `DateTimeOffset` back to Rails' canonical `...Z` millisecond form
// via the `JsonBuilder.EncodeDatetime(DateTimeOffset?)` overload below.
public static class RhDateTime
{
    // Parse a stored ISO-8601 value into a UTC `DateTimeOffset`. Nil-safe:
    // null / empty -> null. Handles the two forms roundhouse ever stores:
    //
    //   * DB-dump / seed form — "2026-05-15 21:14:56.300213" (space
    //     separator, zone-less, microsecond precision, implicitly UTC).
    //   * RFC3339 form — "2026-05-15T21:14:56Z" (what `fill_timestamps`
    //     writes, and API-supplied values).
    //
    // A value that parses under neither returns null rather than throwing — a
    // malformed stored timestamp shouldn't take down a read path. A zone-less
    // value is read as UTC (AssumeUniversal), and the result is normalized to
    // UTC (AdjustToUniversal) so serialization emits a `Z` offset.
    public static DateTimeOffset? Parse(string? s)
    {
        if (string.IsNullOrEmpty(s))
        {
            return null;
        }
        const DateTimeStyles styles =
            DateTimeStyles.AssumeUniversal | DateTimeStyles.AdjustToUniversal;
        // DB-dump form: a space at index 10 separates date and time. Swap it
        // for a `T` so the invariant ISO parser accepts it.
        string candidate = (s.Length > 10 && s[10] == ' ') ? s.Replace(' ', 'T') : s;
        if (DateTimeOffset.TryParse(candidate, CultureInfo.InvariantCulture, styles, out var parsed))
        {
            return parsed;
        }
        return null;
    }

    // Write-side sibling of `Parse` (the `ActiveSupport.db_now` intrinsic
    // target): the current UTC time in Rails' exact sqlite storage form —
    // "YYYY-MM-DD HH:MM:SS.ffffff" (space separator, zero-padded 6-digit
    // fractional seconds, no zone marker), e.g. "2026-07-02 21:33:40.675251".
    // `fill_timestamps` stamps with it so a column's TEXT values stay
    // homogeneous — and lexicographically ordered — when a roundhouse-emitted
    // app shares a database with a real Rails app.
    public static string DbNow()
    {
        return DateTime.UtcNow.ToString("yyyy-MM-dd HH:mm:ss.ffffff", CultureInfo.InvariantCulture);
    }
}

// `DateTimeOffset` overload of `EncodeDatetime`, added to the transpiled
// `JsonBuilder` static class (emitted `partial` for exactly this reason). The
// transpiled `string?` version (from `runtime/ruby/json_builder.rb`) handles
// pre-formatted text; this one formats a native `DateTimeOffset` — what a
// temporal column's reader yields — to Rails' canonical JSON shape: UTC,
// millisecond precision, `Z` suffix (e.g. "2026-05-15T21:14:56.300Z"). The
// compare harness canonicalizes Rails' microsecond precision down to
// milliseconds, so this matches byte-for-byte.
public static partial class JsonBuilder
{
    public static string EncodeDatetime(DateTimeOffset? t)
    {
        if (t == null)
        {
            return "null";
        }
        var utc = t.Value.ToUniversalTime();
        return "\"" + utc.ToString("yyyy-MM-ddTHH:mm:ss.fffZ", CultureInfo.InvariantCulture) + "\"";
    }
}
