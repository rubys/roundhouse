# Hand-written roundhouse runtime primitive (no Ruby source) — the
# native-`DateTime` seam for temporal (Date/DateTime/Time) columns.
#
# Storage stays portable ISO-8601 TEXT: a temporal column hydrates into
# its `<col>_raw` defstruct slot like every other column
# (`Db.column_text`). The model's synthesized reader function parses
# that text into a native UTC `%DateTime{}` via `RhDateTime.parse` (the
# `ActiveSupport.parse_db_time` intrinsic — see the peephole in
# `src/emit/elixir2/expr.rs`). JSON serialization goes through
# `RhDateTime.encode_datetime` — every emitted
# `JsonBuilder.encode_datetime` call routes here, and guard-clause
# dispatch (Elixir's idiomatic runtime overloading) formats a native
# `%DateTime{}` to Rails' canonical `...Z` millisecond form while
# delegating stored-text/nil arguments to the transpiled String variant.
defmodule RhDateTime do
  # Parse a stored ISO-8601 value into a native UTC %DateTime{}.
  # Nil-safe: nil / "" / unparseable → nil. Handles the two forms
  # roundhouse ever stores:
  #
  #   * DB-dump / seed form — "2026-05-15 21:14:56.300213" (space
  #     separator, zone-less, up to microsecond precision, implicitly
  #     UTC).
  #   * RFC3339 form — "2026-05-15T21:14:56Z" (what `fill_timestamps`
  #     writes via `Time.now.utc.iso8601`, and API-supplied values); a
  #     zone-less form reads as UTC.
  def parse(nil), do: nil

  def parse(s) when is_binary(s) do
    case String.trim(s) do
      "" ->
        nil

      str ->
        # Offset-carrying form first (DateTime.from_iso8601 requires an
        # offset and normalizes the result to UTC); zone-less falls back
        # to NaiveDateTime (which accepts both "T" and " " separators)
        # read as UTC.
        case DateTime.from_iso8601(str) do
          {:ok, dt, _offset} ->
            dt

          _ ->
            case NaiveDateTime.from_iso8601(str) do
              {:ok, ndt} -> DateTime.from_naive!(ndt, "Etc/UTC")
              _ -> nil
            end
        end
    end
  end

  def parse(_), do: nil

  # UTC, millisecond precision, `Z` suffix — Rails' canonical datetime
  # JSON (`"2026-05-15T21:14:56.300Z"`). Sub-millisecond digits are
  # TRUNCATED (integer division), matching Rails and the compare
  # harness's micro→milli canonicalization. The microsecond precision
  # field is pinned to 3 so `to_iso8601` always renders exactly `.SSS`
  # (a whole-second value still prints `.000`). A nil (absent column)
  # encodes as null; anything else — the stored-text passthrough path —
  # delegates to the transpiled String variant.
  def encode_datetime(%DateTime{} = dt) do
    {us, _precision} = dt.microsecond
    dt = %{dt | microsecond: {div(us, 1000) * 1000, 3}}
    "\"" <> DateTime.to_iso8601(dt) <> "\""
  end

  def encode_datetime(nil), do: "null"
  def encode_datetime(other), do: JsonBuilder.encode_datetime(other)
end
