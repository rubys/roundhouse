"""Roundhouse Python runtime.

Hand-written Python shipped alongside each generated app. The Python
emitter copies this file verbatim into the generated project as
`app/runtime.py`. Mirrors runtime/{rust,crystal,go,typescript,elixir}.*
— same per-target posture: minimal surface, each new lowering adds
exactly what it needs.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass
class ValidationError:
    """A single validation failure produced by a model's generated
    ``validate`` method. Carries the attribute name and a human-readable
    message; ``full_message`` composes them into a Rails-compatible
    display string (``"Title can't be blank"``).
    """

    field: str
    message: str

    def full_message(self) -> str:
        """Rails-compatible display form: capitalize the field name,
        replace underscores with spaces, prepend to the message.
        ``ValidationError("post_id", "can't be blank")`` becomes
        ``"Post id can't be blank"``.
        """
        label = self.field.replace("_", " ")
        if label:
            label = label[0].upper() + label[1:]
        return f"{label} {self.message}"
