# Roundhouse Python datetime runtime — the native-`datetime` seam for
# temporal (Date/DateTime/Time) columns.
#
# Storage stays portable ISO-8601 TEXT: a temporal column hydrates into a
# `str` backing attribute (`Db.column_text`), exactly like every other
# target. The model's synthesized reader parses that text into a native
# `datetime.datetime` via `Roundhouse.RhDateTime.parse` (see the temporal
# branch in `src/emit/python/library.rs`, and `src/emit/python/expr.rs`,
# which maps the `ActiveSupport.parse_db_time` intrinsic here). The
# write-side sibling `ActiveSupport.db_now` maps to `db_now` below.
#
# JSON serialization then formats a `datetime` back to Rails' canonical
# `...Z` millisecond form. Python has no overloading, so importing this
# module patches the transpiled `json_builder.encode_datetime` to dispatch
# on the input type: a native `datetime` formats to the canonical shape, a
# `None` becomes `null`, and any other value (pre-formatted text) keeps the
# original string-reformatting path.
from __future__ import annotations

import datetime


class Roundhouse:
    class RhDateTime:
        @staticmethod
        def parse(s: str | None) -> datetime.datetime | None:
            # Parse a stored ISO-8601 value into a UTC-aware `datetime`.
            # Nil-safe: None / empty -> None. Handles the two forms
            # roundhouse ever stores:
            #
            #   * DB-dump / seed form — "2026-05-15 21:14:56.300213"
            #     (space separator, zone-less, microsecond precision,
            #     implicitly UTC).
            #   * RFC3339 form — "2026-05-15T21:14:56Z" (what
            #     `fill_timestamps` writes, and API-supplied values).
            #
            # A value that parses under neither returns None rather than
            # raising — a malformed stored timestamp shouldn't take down a
            # read path.
            if s is None or s == "":
                return None
            # `fromisoformat` (C-accelerated) covers every form above in
            # one call on Python >= 3.11 (the project pins 3.14): the
            # space-separated DB-dump forms with or without fractional
            # seconds, and RFC3339 with a trailing `Z`. A zone-less value
            # is implicitly UTC.
            try:
                dt = datetime.datetime.fromisoformat(s)
                if dt.tzinfo is None:
                    dt = dt.replace(tzinfo=datetime.timezone.utc)
                return dt
            except (ValueError, TypeError):
                return None

        @staticmethod
        def db_now() -> str:
            # Current UTC time in Rails' exact sqlite storage form:
            # "YYYY-MM-DD HH:MM:SS.ffffff" — space separator, zero-padded
            # 6-digit fractional seconds (microseconds; `%f` is exactly
            # that), no zone marker (e.g. "2026-07-02 21:33:40.675251").
            # `fill_timestamps` stamps with it so a column's TEXT values
            # stay homogeneous — and lexicographically ordered — when a
            # roundhouse-emitted app shares a database with a real Rails
            # app.
            return datetime.datetime.now(datetime.timezone.utc).strftime(
                "%Y-%m-%d %H:%M:%S.%f"
            )

        @staticmethod
        def format_db_time(value: object) -> str | None:
            # Write-side normalize sibling of `db_now` — the
            # `ActiveSupport.format_db_time` intrinsic behind the
            # synthesized public `<col>=` temporal writer. None -> None;
            # a native `datetime` formats to the same storage text
            # `db_now` produces (a zone-less value is implicitly UTC,
            # matching `parse` above); pre-formatted text passes through
            # untouched.
            if value is None:
                return None
            if isinstance(value, datetime.datetime):
                dt = value
                if dt.tzinfo is not None:
                    dt = dt.astimezone(datetime.timezone.utc)
                return dt.strftime("%Y-%m-%d %H:%M:%S.%f")
            return str(value)


def _encode_datetime_native(value: object) -> str:
    # `datetime` dispatch of `json_builder.encode_datetime`. Formats a
    # native `datetime` — what a temporal column's reader yields — to Rails'
    # canonical JSON shape: UTC, millisecond precision, `Z` suffix (e.g.
    # "2026-05-15T21:14:56.300Z"). The compare harness canonicalizes Rails'
    # microsecond precision down to milliseconds, so this matches
    # byte-for-byte. `None` -> `null`; anything else keeps the transpiled
    # string-reformatting path.
    if value is None:
        return "null"
    if isinstance(value, datetime.datetime):
        dt = value
        if dt.tzinfo is not None:
            dt = dt.astimezone(datetime.timezone.utc)
        millis = dt.microsecond // 1000
        return f'"{dt.strftime("%Y-%m-%dT%H:%M:%S")}.{millis:03d}Z"'
    return _ORIG_ENCODE_DATETIME(value)


# Wrap the transpiled `json_builder.encode_datetime` (a module-level
# function) so views calling `JsonBuilder.encode_datetime(article.created_at)`
# — where `created_at` now yields a native `datetime` — serialize correctly.
# Module attribute lookup is dynamic, so the `from app import json_builder as
# JsonBuilder` binding in views.py resolves the patched function at call time.
from app import json_builder as _json_builder  # noqa: E402

_ORIG_ENCODE_DATETIME = _json_builder.encode_datetime
_json_builder.encode_datetime = _encode_datetime_native
