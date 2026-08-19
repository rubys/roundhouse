# Roundhouse Python HTTP runtime.
#
# Hand-written, shipped alongside generated code (copied in by the
# Python emitter as `app/http.py`). Reduced to the response value type
# by the CtrlWalker retirement: request matching now rides the
# transpiled `app/router.py` over the overlay's `app/v2/routes.py`
# RouteTable, and dispatch is `app/v2/dispatch.handle` (construct the
# controller class, seed request state, `process_action`, translate).
# `server.py` and `test_support.py` both route through it and consume
# this type; the module-handler Router, `ActionContext`, and the
# `respond_to`/`FormatRouter` stubs that served the per-artifact
# controllers retired with them.

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class ActionResponse:
    """The translated response `app.v2.dispatch.handle` returns.
    Fields are optional so call sites pick only what they need:
        body: rendered (layout-wrapped) HTML, or verbatim non-HTML
        status: HTTP status code (default 200)
        location: redirect target URL (for 3xx responses)
    """

    body: str = ""
    status: int = 200
    location: str = ""
    # Set for non-HTML responses; the server ships the body verbatim
    # under this Content-Type and skips nothing further (the layout
    # decision already happened in `handle`).
    content_type: str = ""
    # Flash the action NEWLY set this request (`Flash.to_persisted`'s
    # diff) — a String-keyed map (notice/alert) the server persists to
    # the rh_flash cookie so it shows on the next request, exactly
    # once. Empty otherwise.
    flash: dict[str, str] = field(default_factory=dict)
