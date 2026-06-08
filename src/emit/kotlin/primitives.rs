//! Hand-written Kotlin runtime primitives.
//!
//! These are the target-specific bottom layer (per `project_two_layer_
//! runtime.md`): types the transpiled framework runtime calls into but
//! that have no Ruby source — they bridge to the JVM/JDBC/Javalin stack.
//! The transpiled `runtime/ruby/*.rb` files reach them by name (same
//! `roundhouse` package), so the surface each exposes is dictated by how
//! the emitter renders the corresponding Ruby calls.
//!
//! Grown one primitive at a time, mirroring the runtime-transpile order:
//! Time first (the only thing standing between `ActiveRecordBase.kt` and a
//! clean compile), then Db / Server / ParamValue / the adapter.

use std::path::PathBuf;

use crate::emit::EmittedFile;

/// `Time.now.utc.iso8601` is the sole Time API the framework runtime uses
/// (`ActiveRecord::Base#fill_timestamps`). The emitter renders that chain
/// as `Time.now().utc.iso8601` — a method call then two property reads —
/// so `now()` returns a `TimeInstant` whose `utc`/`iso8601` are `val`s.
const TIME_KT: &str = r#"// Hand-written roundhouse runtime primitive (no Ruby source).
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
"#;

/// The hand-written runtime primitives, emitted under `src/main/kotlin/`.
pub fn primitives() -> Vec<EmittedFile> {
    vec![EmittedFile {
        path: PathBuf::from("src/main/kotlin/Time.kt"),
        content: TIME_KT.to_string(),
    }]
}
