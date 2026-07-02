// Hand-written roundhouse runtime primitive (no Ruby source) — the
// native-`time.Time` seam for temporal (Date/DateTime/Time) columns.
//
// Storage stays portable ISO-8601 TEXT: a temporal column hydrates into
// its `<col>_raw` string field like every other column (`Db_column_text`).
// The model's synthesized reader method parses that text into a native
// UTC `time.Time` via `Rh_parse_db_time` (the `ActiveSupport.parse_db_time`
// intrinsic — see the peephole in `src/emit/go2/expr.rs`). JSON
// serialization formats the native value back to Rails' canonical `...Z`
// millisecond form via `JsonBuilder_encode_datetime_time` below (Go has
// no overloading, so the emitter picks the `_time` variant when the
// argument's static type contains `Time`; the transpiled string variant
// in json_builder.go keeps serving stored-text callers).

package v2

import (
	"strings"
	"time"
)

// Parse a stored ISO-8601 value into a native UTC time.Time. Nil-safe:
// ""/unparseable → the zero Time (Go's empty-as-nil convention, same as
// the "" stand-in used for nilable strings). Handles the two forms
// roundhouse ever stores:
//
//   - DB-dump / seed form — "2026-05-15 21:14:56.300213" (space
//     separator, zone-less, up to microsecond precision, implicitly
//     UTC; also what `fill_timestamps` writes via Rh_db_now below).
//   - RFC3339 form — "2026-05-15T21:14:56Z" (API-supplied values and
//     rows written by pre-db_now builds); a zone-less "T" form reads
//     as UTC.
func Rh_parse_db_time(s string) time.Time {
	str := strings.TrimSpace(s)
	if str == "" {
		return time.Time{}
	}
	// DB-dump / seed form: date and time separated by a space (index 10).
	// ".999999999" tolerates 0-9 fractional digits (or none).
	if len(str) > 10 && str[10] == ' ' {
		if t, err := time.ParseInLocation("2006-01-02 15:04:05.999999999", str, time.UTC); err == nil {
			return t
		}
		return time.Time{}
	}
	// RFC3339 / ISO-8601 offset form ("2026-05-15T21:14:56Z", "...+02:00").
	if t, err := time.Parse(time.RFC3339Nano, str); err == nil {
		return t
	}
	// Zone-less ISO form ("2026-05-15T21:14:56[.ffffff]") — read as UTC.
	if t, err := time.ParseInLocation("2006-01-02T15:04:05.999999999", str, time.UTC); err == nil {
		return t
	}
	return time.Time{}
}

// Write-side sibling of Rh_parse_db_time — the `ActiveSupport.db_now`
// intrinsic (see the Const-receiver peephole in src/emit/go2/expr.rs).
// Returns the current UTC time in Rails' exact sqlite storage form:
//
//	"YYYY-MM-DD HH:MM:SS.ffffff"
//
// — space separator, zero-padded 6-digit fractional seconds
// (microseconds), no zone marker. E.g. "2026-07-02 21:33:40.675251".
// `fill_timestamps` stamps with this so a column's TEXT values stay
// homogeneous — and lexicographically ordered — when a
// roundhouse-emitted app shares a database with a real Rails app.
// Go's ".000000" verb zero-pads to exactly six fractional digits.
func Rh_db_now() string {
	return time.Now().UTC().Format("2006-01-02 15:04:05.000000")
}

// Native-`time.Time` twin of the transpiled `JsonBuilder_encode_datetime`
// (json_builder.go, string→string reformat): UTC, millisecond precision,
// `Z` suffix — Rails' canonical datetime JSON. Go's `Format(".000")`
// TRUNCATES sub-millisecond digits, matching Rails/the compare harness's
// micro→milli canonicalization (and the integer-time targets; see the
// swift rounding lesson). The zero Time (absent column) encodes as null.
func JsonBuilder_encode_datetime_time(t time.Time) string {
	if t.IsZero() {
		return "null"
	}
	return "\"" + t.UTC().Format("2006-01-02T15:04:05.000") + "Z\""
}
