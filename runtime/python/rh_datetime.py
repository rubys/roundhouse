# Roundhouse Python datetime runtime — the native-`datetime` seam for
# temporal (Date/DateTime/Time) columns.
#
# Storage stays portable ISO-8601 TEXT: a temporal column hydrates into a
# `str` backing attribute (`Db.column_text`), exactly like every other
# target. The model's synthesized reader parses that text into a native
# `datetime.datetime` via `Roundhouse.RhDateTime.parse` (see the temporal
# branch in `src/emit/python/library.rs`, and `src/emit/python/expr.rs`,
# which maps the `ActiveSupport.parse_db_time` intrinsic here).
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
            utc = datetime.timezone.utc
            # DB-dump form with fractional seconds.
            try:
                dt = datetime.datetime.strptime(s, "%Y-%m-%d %H:%M:%S.%f")
                return dt.replace(tzinfo=utc)
            except (ValueError, TypeError):
                pass
            # DB-dump form without fractional seconds.
            try:
                dt = datetime.datetime.strptime(s, "%Y-%m-%d %H:%M:%S")
                return dt.replace(tzinfo=utc)
            except (ValueError, TypeError):
                pass
            # RFC3339 / ISO-8601 fallback (a trailing `Z` is not accepted by
            # `fromisoformat` before 3.11, so normalize it to `+00:00`).
            try:
                iso = s[:-1] + "+00:00" if s.endswith("Z") else s
                dt = datetime.datetime.fromisoformat(iso)
                if dt.tzinfo is None:
                    dt = dt.replace(tzinfo=utc)
                return dt
            except (ValueError, TypeError):
                return None


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
