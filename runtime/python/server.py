# Roundhouse Python server runtime.
#
# Hand-written, shipped alongside generated code (copied in by the
# Python emitter as `app/server.py`). Runs aiohttp for HTTP + WebSocket
# support on one event loop — same shape as railcar's proven Python
# target. Dispatches HTTP via `http.Router.match`, upgrades
# WebSocket requests on `/cable` through `app.cable.cable_handler`,
# and wraps HTML responses in the emitted layout when one is provided.
#
# aiohttp is a pip dep; users run `uv run python3 -m app` (see the
# emitted `pyproject.toml`). The compile + unit-test paths never
# import this module, so missing aiohttp doesn't break those tests.

from __future__ import annotations

import asyncio
import inspect
import os
import sys
import traceback
from typing import Any, Callable
from urllib.parse import parse_qs, quote, unquote

from aiohttp import web

from . import cable as _cable
from . import db as _db
from . import http as _http
from . import view_helpers as _view_helpers
from .flash import Flash

# Flash is cookie-backed and per-session (per browser), so parallel clients
# never share a flash slot. The "show exactly once" lifecycle: the action
# that sets a flash carries it on `ActionResponse.flash`, which the server
# writes to the rh_flash cookie; the next request reloads it into
# `context.flash` for display and sets no new flash, so the cookie is
# cleared. Storage adapter only — mirrors go/kotlin/swift/elixir.
_FLASH_COOKIE = "rh_flash"


def _read_flash_cookie(request: web.Request) -> Flash:
    """Decode the rh_flash cookie into a Flash for view display. Absent ->
    empty Flash (first request in a session). Only the closed notice/alert
    key set; values percent-encoded, `key:value&…` so the urlencoded
    structure + cookie-octet rules survive arbitrary notice text (`:`/`&`
    avoid http.cookies' value-quoting that `=` would trigger)."""
    raw = request.cookies.get(_FLASH_COOKIE)
    if not raw:
        return Flash()
    out: dict[str, str] = {}
    for pair in raw.split("&"):
        if ":" not in pair:
            continue
        k, v = pair.split(":", 1)
        if k in ("notice", "alert"):
            decoded = unquote(v)
            if decoded:
                out[k] = decoded
    return Flash(out)


def _apply_flash_cookie(resp: web.Response, persisted: dict[str, str]) -> None:
    """Write the entries the action set (already the show-once set) to the
    rh_flash cookie. Empty -> clear it so a shown notice doesn't stick."""
    if not persisted:
        resp.del_cookie(_FLASH_COOKIE, path="/")
        return
    parts = [
        f"{k}:{quote(persisted[k], safe='')}"
        for k in ("notice", "alert")
        if k in persisted
    ]
    resp.set_cookie(_FLASH_COOKIE, "&".join(parts), path="/", httponly=True)


def start(
    *,
    schema_sql: str,
    layout: Callable[[], str] | None = None,
    db_path: str | None = None,
    port: int | None = None,
) -> None:
    """Open DB, apply schema, start an aiohttp server. Blocks until
    the process exits. Mirrors runtime/rust/server.rs's `start` and
    runtime/typescript/server.ts's `startServer`."""
    if db_path is None:
        db_path = "storage/development.sqlite3"
    if port is None:
        port_s = os.environ.get("PORT", "3000")
        try:
            port = int(port_s)
        except ValueError:
            port = 3000

    _db.open_production_db(db_path, schema_sql)

    application = _build_app(layout)
    host = "127.0.0.1"
    print(f"Roundhouse Python server listening on http://{host}:{port}")
    # aiohttp.web.run_app prints its own startup banner; suppress it
    # to keep the compare-tool log clean (stdout gets parsed).
    web.run_app(application, host=host, port=port, print=lambda _msg: None)


def _build_app(layout: Callable[[], str] | None) -> web.Application:
    """Assemble the aiohttp Application. One catch-all route fans out
    to `http.Router.match` for HTTP, plus an explicit `/cable` route
    for WebSocket upgrades. No aiohttp middleware — method override
    and layout wrap happen inline in the dispatch handler so we can
    share the single request-body read and preserve the order used by
    the rust/typescript runtimes."""
    application = web.Application()
    application["roundhouse.layout"] = layout
    application.router.add_get("/cable", _cable.cable_handler)
    # Serve compiled assets (tailwind.css, turbo.min.js, …) from
    # static/assets/ at /assets/* — the URLs the emitted layout's
    # stylesheet_link_tag / importmap reference. Registered before the
    # catch-all so /assets/* doesn't fall through to the router. aiohttp's
    # add_static raises if the dir is missing, so ensure it exists (a fresh
    # archive with no built assets still boots; the links just 404).
    _assets_dir = "static/assets"
    os.makedirs(_assets_dir, exist_ok=True)
    application.router.add_static("/assets", _assets_dir, show_index=False)
    application.router.add_route("*", "/{path:.*}", _dispatch_request)
    return application


async def _dispatch_request(request: web.Request) -> web.StreamResponse:
    """Core dispatcher: parse body (with `_method` override), look up
    the matched handler via `http.Router.match`, await the result,
    then wrap HTML responses in the layout. Exceptions surface as
    500s with the traceback on stderr so the compare tool sees the
    same message it would from wsgiref."""
    _view_helpers.reset_render_state()

    method = request.method.upper()
    path = request.rel_url.path

    # Per-request format inference: strip a `.json` suffix before route
    # matching so `/articles/1.json` and `/articles/1` share one route
    # entry, and remember the format so the controller's implicit-render
    # branch picks the json view + Content-Type. Mirrors the TS server.
    request_format = "html"
    if path.endswith(".json"):
        request_format = "json"
        path = path[:-5]

    body_text, body_params = await _read_form_body(request)

    # Rails scaffold forms submit POST with `_method=patch|put|delete`
    # when the real verb isn't supported in browsers. Rewrite before
    # the route lookup so the downstream handler sees the true verb.
    if method == "POST":
        override = body_params.get("_method", "")
        if isinstance(override, str):
            override = override.upper()
            if override in ("PATCH", "PUT", "DELETE"):
                method = override

    matched = _http.Router.match(method, path)
    if matched is None:
        return web.Response(
            status=404,
            text="Not Found",
            content_type="text/plain",
            charset="utf-8",
        )

    handler, path_params = matched
    params: dict[str, Any] = {}
    params.update(path_params)
    params.update(body_params)

    ctx = _http.ActionContext(
        params=params,
        request_format=request_format,
        flash=_read_flash_cookie(request),
    )
    try:
        result = handler(ctx)
        if inspect.isawaitable(result):
            result = await result
    except Exception:
        traceback.print_exc()
        return web.Response(
            status=500,
            text="Internal Server Error",
            content_type="text/plain",
            charset="utf-8",
        )

    if not isinstance(result, _http.ActionResponse):
        result = _http.ActionResponse()

    status = result.status or 200
    body = result.body or ""
    layout = request.app["roundhouse.layout"]
    is_redirect = 300 <= status < 400 and bool(result.location)

    if is_redirect:
        resp = web.Response(
            status=status,
            text=body,
            headers={"Location": result.location or ""},
            content_type="text/html",
            charset="utf-8",
        )
        _apply_flash_cookie(resp, result.flash)
        return resp

    # JSON responses ship the controller body verbatim under the
    # controller-supplied Content-Type and skip the html layout wrap.
    # Mirrors the TS server's `request_format === "json"` branch.
    if request_format == "json" or result.content_type:
        resp = web.Response(
            status=status,
            text=body,
            content_type=result.content_type or "application/json",
            charset="utf-8",
        )
        _apply_flash_cookie(resp, result.flash)
        return resp

    # Wrap the view body in the emitted layout (when present). The
    # fallback when no layout is provided matches the rust/typescript
    # runtimes' minimal shell — Tailwind CDN + plain Turbo importmap.
    if layout is not None:
        _view_helpers.set_yield(body)
        wrapped = layout()
    else:
        wrapped = _fallback_layout(body)

    resp = web.Response(
        status=status,
        text=wrapped,
        content_type="text/html",
        charset="utf-8",
    )
    _apply_flash_cookie(resp, result.flash)
    return resp


async def _read_form_body(request: web.Request) -> tuple[str, dict[str, Any]]:
    """Read + parse an urlencoded form body. Returns the raw text
    (unused today; kept for parity with the rust/typescript shape)
    and a Rails-flattened params dict — `article[title]=foo` lands
    as `params["article[title]"] = "foo"` rather than a nested
    dict, matching the emitted controllers' lookup shape."""
    ct = request.content_type or ""
    if not ct.startswith("application/x-www-form-urlencoded"):
        return "", {}
    raw = await request.read()
    if not raw:
        return "", {}
    text = raw.decode("utf-8", errors="replace")
    parsed = parse_qs(text, keep_blank_values=True)
    out: dict[str, Any] = {}
    for k, vs in parsed.items():
        out[k] = vs[-1] if vs else ""
    return text, out


def _fallback_layout(body: str) -> str:
    """Last-resort document shell for fixtures without a
    `layouts/application` ERB. Matches runtime/rust/server.rs's
    `render_layout` — Tailwind Play CDN + plain `@hotwired/turbo`
    via importmap. Never emits `<meta name="action-cable-url">`; the
    `@rails/actioncable` default `/cable` is what `cable_handler`
    listens on. Asset paths point at `/assets/tailwind.css` +
    `/assets/turbo.min.js` — `bin/rh transpile python` (or the
    target's local build step) is expected to have populated
    `static/assets/`. If the build didn't run, the page is unstyled
    but functional."""
    return f"""<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8">
    <title>Roundhouse App</title>
    <meta name="viewport" content="width=device-width,initial-scale=1">
    <link rel="icon" href="data:,">
    <link rel="stylesheet" href="/assets/tailwind.css">
    <script type="importmap">
    {{
      "imports": {{
        "@hotwired/turbo": "/assets/turbo.min.js"
      }}
    }}
    </script>
    <script type="module">import "@hotwired/turbo";</script>
  </head>
  <body>
    <main class="container mx-auto mt-8 px-5 flex flex-col">
      {body}
    </main>
  </body>
</html>
"""


if __name__ == "__main__":
    sys.stderr.write(
        "run the emitted `python -m app` entry point instead of app.server directly\n"
    )
    sys.exit(2)
