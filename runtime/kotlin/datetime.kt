// Hand-written roundhouse runtime primitive (no Ruby source) — the
// native-`Time` seam for temporal (Date/DateTime/Time) columns.
//
// Storage stays portable ISO-8601 TEXT: a temporal column hydrates into a
// `String` backing field (`<col>Raw`), exactly like every other column
// (`Db.columnText`). The model's synthesized reader parses that text into a
// native `java.time.OffsetDateTime` via `RhDateTime.parse` (see the temporal
// branch in `src/emit/kotlin/library.rs` + the `ActiveSupport.parse_db_time`
// mapping in `src/emit/kotlin/expr.rs`). JSON serialization then formats an
// `OffsetDateTime` back to Rails' canonical `...Z` millisecond form via the
// `JsonBuilder.encodeDatetime(OffsetDateTime?)` overload below.

package roundhouse

import java.time.Instant
import java.time.LocalDateTime
import java.time.OffsetDateTime
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter
import java.time.format.DateTimeFormatterBuilder
import java.time.temporal.ChronoField

object RhDateTime {
    // DB-dump / seed form: "2026-05-15 21:14:56[.ffffff]" — space separator,
    // zone-less, implicitly UTC, 0-9 fractional digits.
    private val SPACE_FORMAT: DateTimeFormatter = DateTimeFormatterBuilder()
        .appendPattern("yyyy-MM-dd HH:mm:ss")
        .optionalStart()
        .appendFraction(ChronoField.NANO_OF_SECOND, 0, 9, true)
        .optionalEnd()
        .toFormatter()

    // Parse a stored ISO-8601 value into a native UTC `OffsetDateTime`.
    // Nil-safe: null / empty → null. Handles the two forms roundhouse ever
    // stores:
    //
    //   * DB-dump / seed form — "2026-05-15 21:14:56.300213" (space
    //     separator, zone-less, microsecond precision, implicitly UTC).
    //   * RFC3339 form — "2026-05-15T21:14:56Z" (what `fill_timestamps`
    //     writes via `Time.now.utc.iso8601`, and API-supplied values).
    //
    // A value that parses under neither returns null rather than raising —
    // a malformed stored timestamp shouldn't take down a read path.
    fun parse(s: String?): OffsetDateTime? {
        val str = (s ?: return null).trim()
        if (str.isEmpty()) return null
        // DB-dump / seed form: date and time separated by a space (index 10).
        if (str.length > 10 && str[10] == ' ') {
            return try {
                LocalDateTime.parse(str, SPACE_FORMAT).atOffset(ZoneOffset.UTC)
            } catch (e: Exception) {
                null
            }
        }
        // RFC3339 / ISO-8601 offset form ("2026-05-15T21:14:56Z", "...+02:00").
        return try {
            OffsetDateTime.parse(str)
        } catch (e: Exception) {
            // Zone-less ISO form ("2026-05-15T21:14:56") — read as UTC.
            try {
                LocalDateTime.parse(str).atOffset(ZoneOffset.UTC)
            } catch (e2: Exception) {
                null
            }
        }
    }

    // Rails' exact sqlite storage form: space separator, zero-padded 6-digit
    // fractional seconds (microseconds), no zone marker. Cached — a
    // `DateTimeFormatter` is immutable and thread-safe.
    private val DB_NOW_FORMAT: DateTimeFormatter =
        DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm:ss.SSSSSS").withZone(ZoneOffset.UTC)

    // Write-side sibling of `parse` — the `ActiveSupport.db_now` intrinsic.
    // Current UTC time in Rails' exact storage form
    // ("2026-07-02 21:33:40.675251"). `fill_timestamps` stamps with it so a
    // column's TEXT values stay homogeneous — and lexicographically
    // ordered — when a roundhouse-emitted app shares a database with a real
    // Rails app.
    fun dbNow(): String = DB_NOW_FORMAT.format(Instant.now())

    // Write-side normalize sibling of `dbNow` — the
    // `ActiveSupport.format_db_time` intrinsic behind the synthesized
    // public `<col>=` temporal writer. Formats a native
    // `OffsetDateTime` to the same storage text `dbNow` produces.
    // Non-null in and out, like `dbNow`: a NOT NULL column's writer
    // calls it directly (`String` = `String`), and a nullable column's
    // writer maps it over its optional (`value?.let { formatDbTime(it) }`
    // → `String?`) — the emitter picks the form from the argument's
    // stamped optionality.
    fun formatDbTime(value: OffsetDateTime): String = DB_NOW_FORMAT.format(value.toInstant())
}

// UTC, millisecond precision, `Z` suffix — Rails' canonical datetime JSON.
private val RH_JSON_DATETIME: DateTimeFormatter =
    DateTimeFormatter.ofPattern("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'")

// `OffsetDateTime` overload of `JsonBuilder.encodeDatetime`. The transpiled
// `String?` member (json_builder.kt, from the shared runtime) handles
// pre-formatted stored text; this formats a native `OffsetDateTime` — what a
// temporal column's reader yields — to Rails' canonical shape
// ("2026-05-15T21:14:56.300Z"). An extension function because Kotlin
// `object`s aren't reopenable across files; overload resolution selects it
// over the member for an `OffsetDateTime?` argument. The compare harness
// canonicalizes Rails' microsecond precision down to milliseconds, so this
// matches byte-for-byte.
fun JsonBuilder.encodeDatetime(t: OffsetDateTime?): String {
    if (t == null) return "null"
    return "\"" + t.withOffsetSameInstant(ZoneOffset.UTC).format(RH_JSON_DATETIME) + "\""
}
