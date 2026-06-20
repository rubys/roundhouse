// Hand-written roundhouse runtime primitive (no Ruby source).
// Minimal shim for `Time.now.utc.iso8601`, used by
// ActiveRecord::Base#fill_timestamps to stamp created_at/updated_at.

package roundhouse

import java.time.OffsetDateTime
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter
import java.time.temporal.ChronoUnit

object Time {
    fun now(): TimeInstant = TimeInstant(OffsetDateTime.now())
}

class TimeInstant(private val dt: OffsetDateTime) {
    // `Time#utc` — the same instant at a UTC offset.
    val utc: TimeInstant
        get() = TimeInstant(dt.withOffsetSameInstant(ZoneOffset.UTC))

    // `Time#iso8601` — seconds precision, `Z` for a zero offset (the `XXX`
    // pattern renders UTC as `Z`, matching Ruby): `2026-06-07T17:30:00Z`.
    val iso8601: String
        get() = dt.truncatedTo(ChronoUnit.SECONDS)
            .format(DateTimeFormatter.ofPattern("yyyy-MM-dd'T'HH:mm:ssXXX"))
}
