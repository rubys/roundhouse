# Roundhouse Python view-helpers runtime.
#
# Hand-written, shipped alongside generated code (copied in by the
# Python emitter as `app/view_helpers.py`). Provides the Rails-
# compatible view helpers emitted view fns call into: `link_to`,
# `button_to`, `form_wrap`, `FormBuilder` methods,
# `turbo_stream_from`, `dom_id`, `pluralize`, plus layout-slot
# storage (`set_yield` / `content_for_*`).
#
# Mirrors runtime/typescript/view_helpers.ts + runtime/rust/
# view_helpers.rs in intent and byte-output shape — the scaffold
# blog's DOM-structural compare tolerates text differences but
# requires matching attribute sets + whitespace around tags.

from __future__ import annotations

import threading
from dataclasses import dataclass
from typing import Any


@dataclass
class RenderCtx:
    """Layout-slot context threaded through views. Populated by
    `content_for`; layouts read via `content_for(slot)` getter."""

    notice: str | None = None
    alert: str | None = None
    title: str | None = None


# ── Render-state (yield + content_for slots) ───────────────────
# Request-scoped storage for the currently rendering view's inner
# body (for `<%= yield %>`) and any `content_for :slot` deposits
# the view made along the way. Thread-local so the wsgiref
# single-thread server can still support a future multithreaded
# server without reworking the API.

_render_state = threading.local()


def _state() -> dict[str, Any]:
    s = getattr(_render_state, "state", None)
    if s is None:
        s = {"yield": "", "slots": {}}
        _render_state.state = s
    return s


def reset_render_state() -> None:
    """Called by the server middleware at the start of each
    request so a previous action's slots don't leak through."""
    _render_state.state = {"yield": "", "slots": {}}


def set_yield(body: str) -> None:
    """Stash the inner-view body for the layout's `<%= yield %>`."""
    _state()["yield"] = body


def get_yield() -> str:
    return _state().get("yield", "")


def get_slot(name: str) -> str:
    return _state().get("slots", {}).get(name, "")


def content_for_set(slot: str, body: str) -> None:
    _state().setdefault("slots", {})[slot] = body


def content_for_get(slot: str) -> str:
    return _state().get("slots", {}).get(slot, "")


def content_for(slot: str, body: str | None = None) -> str:
    """Dual-form helper — getter when body is None, setter
    otherwise. Matches Rails' ambidextrous API."""
    if body is None:
        return content_for_get(slot)
    content_for_set(slot, body)
    return ""


# ── Layout-meta helpers ────────────────────────────────────────


def csrf_meta_tags() -> str:
    # Scaffold layout emits these; we stub with empty tokens so the
    # DOM shape still matches (attribute set + element count).
    return (
        '<meta name="csrf-param" content="authenticity_token" />'
        '<meta name="csrf-token" content="" />'
    )


def csp_meta_tag() -> str:
    return ""


def stylesheet_link_tag(name: str, **opts: Any) -> str:
    href = f"/assets/{name}.css"
    attrs = "".join(
        f' {_attr_key(k)}="{_escape(str(v))}"' for k, v in opts.items()
    )
    return f'<link rel="stylesheet" href="{_escape(href)}"{attrs} />'


def javascript_importmap_tags(pins: list[tuple[str, str]], main_entry: str = "application") -> str:
    import json

    imports = {name: path for (name, path) in pins}
    return (
        '<script type="importmap">'
        + json.dumps({"imports": imports}, separators=(",", ":"))
        + "</script>"
        + f'<link rel="modulepreload" href="/assets/{main_entry}.js" />'
        + f'<script type="module" src="/assets/{main_entry}.js"></script>'
    )


# ── link_to / button_to ────────────────────────────────────────


def link_to(text: str, url: str, opts: dict[str, Any] | None = None, **kwargs: Any) -> str:
    """<a href="url" ...attrs>text</a>. Accepts opts as either an
    explicit dict or kwargs — emitters use the dict form."""
    merged: dict[str, Any] = {}
    if opts:
        merged.update(opts)
    merged.update(kwargs)
    attrs = "".join(
        f' {_attr_key(k)}="{_escape(str(v))}"' for k, v in merged.items()
    )
    return f'<a href="{_escape(url)}"{attrs}>{_escape(str(text))}</a>'


def button_to(text: str, target: str, opts: dict[str, Any] | None = None, **kwargs: Any) -> str:
    """<form><button>text</button></form>. `method: :delete` becomes
    a hidden `_method` input. `form_class` splits from `class` —
    `class` applies to the button, `form_class` to the wrapper
    form (Rails convention for button_to)."""
    merged: dict[str, Any] = {}
    if opts:
        merged.update(opts)
    merged.update(kwargs)
    method_raw = str(merged.pop("method", "post"))
    method_lower = method_raw.lower()
    form_class = str(merged.pop("form_class", ""))
    button_class = str(merged.pop("class", ""))

    method_input = (
        f'<input type="hidden" name="_method" value="{_escape(method_raw)}"/>'
        if method_lower not in ("post", "get")
        else ""
    )
    # Remaining keys become data-* / misc button attrs.
    button_attrs = "".join(
        f' {_attr_key(k)}="{_escape(str(v))}"' for k, v in merged.items()
    )
    form_cls_attr = f' class="{_escape(form_class)}"' if form_class else ""
    btn_cls_attr = f' class="{_escape(button_class)}"' if button_class else ""
    return (
        f'<form method="post" action="{_escape(target)}"{form_cls_attr}>'
        f"{method_input}"
        f"<button{btn_cls_attr}{button_attrs} type=\"submit\">"
        f"{_escape(str(text))}</button></form>"
    )


# ── form_with wrapper ──────────────────────────────────────────


def form_wrap(action: str | None, is_persisted: bool, cls: str, inner: str) -> str:
    action_attr = f' action="{_escape(action)}"' if action is not None else ""
    cls_attr = f' class="{_escape(cls)}"' if cls else ""
    method_input = (
        '<input type="hidden" name="_method" value="patch"/>' if is_persisted else ""
    )
    return (
        f'<form method="post"{action_attr} accept-charset="UTF-8"{cls_attr}>'
        f"{method_input}{inner}</form>"
    )


# ── FormBuilder ────────────────────────────────────────────────


class FormBuilder:
    """One instance per form_with block. Field-name prefix binds the
    input names (`article[title]`). `is_persisted` drives the submit
    button's default label ("Update" vs "Create")."""

    def __init__(self, prefix: str, cls: str = "", is_persisted: bool = False):
        self.prefix = prefix
        self.cls = cls
        self.is_persisted = is_persisted

    def _name(self, field: str) -> str:
        return f"{self.prefix}[{field}]" if self.prefix else field

    def label(self, field: str, opts: dict[str, Any] | None = None) -> str:
        cls = (opts or {}).get("class", "")
        cls_attr = f' class="{_escape(str(cls))}"' if cls else ""
        return f'<label for="{_escape(self._input_id(field))}"{cls_attr}>{_escape(field.capitalize())}</label>'

    def _input_id(self, field: str) -> str:
        return f"{self.prefix}_{field}" if self.prefix else field

    def text_field(
        self, field: str, value: str | None = None, opts: dict[str, Any] | None = None
    ) -> str:
        opts = opts or {}
        cls = str(opts.get("class", ""))
        cls_attr = f' class="{_escape(cls)}"' if cls else ""
        value_attr = f' value="{_escape(value)}"' if value else ""
        return (
            f'<input type="text" name="{_escape(self._name(field))}"'
            f' id="{_escape(self._input_id(field))}"{value_attr}{cls_attr} />'
        )

    def textarea(
        self, field: str, value: str | None = None, opts: dict[str, Any] | None = None
    ) -> str:
        opts = opts or {}
        cls = str(opts.get("class", ""))
        rows = opts.get("rows")
        cls_attr = f' class="{_escape(cls)}"' if cls else ""
        rows_attr = f' rows="{_escape(str(rows))}"' if rows is not None else ""
        body = f"\n{_escape(value)}\n" if value else ""
        return (
            f'<textarea name="{_escape(self._name(field))}"'
            f' id="{_escape(self._input_id(field))}"{rows_attr}{cls_attr}>'
            f"{body}</textarea>"
        )

    def submit(self, opts: dict[str, Any] | None = None) -> str:
        opts = opts or {}
        cls = str(opts.get("class", ""))
        cls_attr = f' class="{_escape(cls)}"' if cls else ""
        label = opts.get("label")
        if label is None:
            prefix_human = self.prefix[:1].upper() + self.prefix[1:] if self.prefix else ""
            label = (
                f"Update {prefix_human}" if self.is_persisted else f"Create {prefix_human}"
            )
        esc = _escape(str(label))
        return (
            f'<input type="submit" name="commit" value="{esc}"{cls_attr}'
            f' data-disable-with="{esc}" />'
        )


# ── Turbo / misc ───────────────────────────────────────────────


def turbo_stream_from(channel: str) -> str:
    return f'<turbo-cable-stream-source channel="{_escape(channel)}"/>'


def dom_id(singular: str, id: int, prefix: str | None = None) -> str:
    base = f"{singular}_{id}"
    return f"{prefix}_{base}" if prefix else base


def pluralize(count: int, word: str) -> str:
    return f"1 {word}" if count == 1 else f"{count} {word}s"


def truncate(text: str, opts: dict[str, Any] | None = None) -> str:
    opts = opts or {}
    length = int(opts.get("length", 30))
    if len(text) <= length:
        return text
    return text[:length].rstrip() + "..."


def field_has_error(errors: list[Any], field: str) -> bool:
    return any(getattr(e, "field", None) == field for e in errors)


def error_messages_for(errors: list[Any], noun: str) -> str:
    # Deliberately empty — the emitter's form_with block renders
    # error explanations inline via the scaffold's own block; this
    # stub is called by the degraded path before the block takes
    # over.
    return ""


# ── helpers ────────────────────────────────────────────────────


def _attr_key(k: str) -> str:
    # Python identifiers can't contain `-`, so emitters pass keys
    # as-is; no translation needed here. Stays a helper in case we
    # decide to snake→kebab later.
    return k


def _escape(s: str) -> str:
    return (
        str(s)
        .replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
        .replace("'", "&#39;")
    )
