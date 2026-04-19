# Roundhouse Python view-helpers runtime.
#
# Hand-written, shipped alongside generated code (copied in by the
# Python emitter as `app/view_helpers.py`). Provides the Rails-
# compatible view helpers emitted view fns call into: `link_to`,
# `button_to`, `form_wrap`, `FormBuilder` methods,
# `turbo_stream_from`, `dom_id`, `pluralize`, plus a `RenderCtx`
# for layout slots (notice / alert / title).
#
# Mirrors runtime/typescript/view_helpers.ts in intent + method
# signatures. Implementations are deliberately minimal — enough
# HTML for the scaffold blog's tests to pass. Emitted views stay
# stable when later phases swap in faithful output.

from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass
class RenderCtx:
    """Layout-slot context threaded through views. Populated by
    `content_for`; layouts would consult but Phase 4d doesn't
    wire a layout dispatcher."""

    notice: str | None = None
    alert: str | None = None
    title: str | None = None


def link_to(text: str, url: str, **opts: str) -> str:
    """<a href="url" ...attrs>text</a>. `opts` is an attribute map."""
    attrs = "".join(f' {k}="{_escape(v)}"' for k, v in opts.items())
    return f'<a href="{_escape(url)}"{attrs}>{_escape(str(text))}</a>'


def button_to(text: str, target: str, **opts: Any) -> str:
    """<form method="post" action="..."><button>text</button></form>.
    `method: :delete` becomes a hidden `_method` input."""
    method_raw = opts.get("method", "post")
    method = str(method_raw) if isinstance(method_raw, str) else "post"
    cls_raw = opts.get("class", "")
    cls = str(cls_raw) if isinstance(cls_raw, str) else ""
    method_input = (
        f'<input type="hidden" name="_method" value="{_escape(method)}"/>'
        if method.lower() not in ("post", "get")
        else ""
    )
    return (
        f'<form method="post" action="{_escape(target)}" '
        f'class="{_escape(cls)}">{method_input}'
        f'<button>{_escape(str(text))}</button></form>'
    )


def form_wrap(action: str | None, cls: str, inner: str) -> str:
    """Form-tag wrapper. Called by the emitter after rendering a
    `form_with` block's inner buffer."""
    action_attr = f' action="{_escape(action)}"' if action is not None else ""
    return (
        f'<form method="post"{action_attr} class="{_escape(cls)}">{inner}</form>'
    )


class FormBuilder:
    """Stub FormBuilder. One instance per form_with block. Minimal
    option support — the scaffold tests don't check input
    attributes."""

    def __init__(self, record: Any = None, cls: str = ""):
        self.record = record
        self.cls = cls

    def label(self, field: str) -> str:
        return f'<label for="{_escape(field)}">{_escape(field)}</label>'

    def text_field(self, field: str) -> str:
        return f'<input type="text" name="{_escape(field)}"/>'

    def textarea(self, field: str) -> str:
        return f'<textarea name="{_escape(field)}"></textarea>'

    def submit(self) -> str:
        return '<input type="submit" value="Submit"/>'


def turbo_stream_from(channel: str) -> str:
    """<turbo-cable-stream-source> tag. Visible in rendered output
    without a live websocket."""
    return f'<turbo-cable-stream-source channel="{_escape(channel)}"/>'


def dom_id(record: Any) -> str:
    """dom_id(record) → "record_<id>"."""
    if record is None:
        return ""
    rid = getattr(record, "id", None)
    return f"record_{rid}" if rid is not None else ""


def pluralize(count: int, word: str) -> str:
    return f"1 {word}" if count == 1 else f"{count} {word}s"


def content_for(_slot: str, _body: str) -> str:
    """content_for stash. Phase 4d's emitted views don't route
    through a layout, so this returns an empty string."""
    return ""


def _escape(s: str) -> str:
    """Conservative HTML escaping."""
    return (
        str(s)
        .replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
        .replace("'", "&#39;")
    )
