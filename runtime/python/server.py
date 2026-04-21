# Roundhouse Python server runtime.
#
# Hand-written, shipped alongside generated code (copied in by the
# Python emitter as `app/server.py`). Starts a stdlib wsgiref server,
# dispatches through `http.Router.match`, and wraps HTML responses in
# the emitted layout (when provided).
#
# Mirrors runtime/rust/server.rs and runtime/typescript/server.ts in
# intent: the caller-supplied `layout` closure is invoked per-request
# with the inner view body stashed via `view_helpers.set_yield`, so
# `<%= yield %>` in the layout template resolves to the action's
# rendered HTML.

from __future__ import annotations

import os
import sys
import traceback
from typing import Any, Callable
from urllib.parse import parse_qs
from wsgiref.simple_server import make_server

from . import db as _db
from . import http as _http
from . import view_helpers as _view_helpers


def start(
    *,
    schema_sql: str,
    layout: Callable[[], str] | None = None,
    db_path: str | None = None,
    port: int | None = None,
) -> None:
    """Open DB, apply schema, start the wsgiref server. Blocks until
    the process exits."""
    if db_path is None:
        db_path = "storage/development.sqlite3"
    if port is None:
        port_s = os.environ.get("PORT", "3000")
        try:
            port = int(port_s)
        except ValueError:
            port = 3000

    _db.open_production_db(db_path, schema_sql)

    app = _wrap_layout(_method_override(_wsgi_dispatch), layout)
    host = "127.0.0.1"
    with make_server(host, port, app) as httpd:
        print(f"Roundhouse Python server listening on http://{host}:{port}")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            pass


def _wsgi_dispatch(environ: dict[str, Any], start_response: Callable) -> list[bytes]:
    """Core dispatcher: Router.match → handler(ActionContext) →
    ActionResponse → WSGI tuple."""
    method = environ.get("REQUEST_METHOD", "GET").upper()
    path = environ.get("PATH_INFO", "/") or "/"

    # Wipe yield/slot state before each request so the layout doesn't
    # see stale body from a prior dispatch.
    _view_helpers.reset_render_state()

    matched = _http.Router.match(method, path)
    if matched is None:
        start_response("404 Not Found", [("Content-Type", "text/plain; charset=utf-8")])
        return [b"Not Found"]

    handler, path_params = matched
    body_params = _parse_body(environ)
    params: dict[str, Any] = {}
    params.update(path_params)
    params.update(body_params)

    ctx = _http.ActionContext(params=params)
    try:
        result = handler(ctx)
        import inspect, asyncio
        if inspect.isawaitable(result):
            result = asyncio.new_event_loop().run_until_complete(result)
    except Exception:
        traceback.print_exc()
        start_response(
            "500 Internal Server Error",
            [("Content-Type", "text/plain; charset=utf-8")],
        )
        return [b"Internal Server Error"]

    if not isinstance(result, _http.ActionResponse):
        result = _http.ActionResponse()

    status = result.status or 200
    body = result.body or ""
    headers: list[tuple[str, str]] = []

    if 300 <= status < 400 and result.location:
        headers.append(("Location", result.location))
        headers.append(("Content-Type", "text/html; charset=utf-8"))
        payload = body.encode("utf-8")
    else:
        headers.append(("Content-Type", "text/html; charset=utf-8"))
        payload = body.encode("utf-8")

    start_response(f"{status} {_status_phrase(status)}", headers)
    return [payload]


def _method_override(inner: Callable) -> Callable:
    """Rails scaffold forms submit POST with hidden `_method=patch|
    put|delete` when the real verb isn't supported in browsers. Parse
    the body, rewrite REQUEST_METHOD, and pass the buffered body
    through so the downstream parser still sees it."""

    def wrapped(environ: dict[str, Any], start_response: Callable):
        if environ.get("REQUEST_METHOD", "").upper() != "POST":
            return inner(environ, start_response)
        content_type = environ.get("CONTENT_TYPE", "")
        if not content_type.startswith("application/x-www-form-urlencoded"):
            return inner(environ, start_response)
        length = int(environ.get("CONTENT_LENGTH") or 0)
        if length <= 0:
            return inner(environ, start_response)
        raw = environ["wsgi.input"].read(length)
        text = raw.decode("utf-8", errors="replace")
        parsed = parse_qs(text, keep_blank_values=True)
        override = parsed.get("_method", [""])[0].upper()
        if override in ("PATCH", "PUT", "DELETE"):
            environ["REQUEST_METHOD"] = override
        # Re-inject the body for the dispatcher's parser.
        import io as _io
        environ["wsgi.input"] = _io.BytesIO(raw)
        environ["CONTENT_LENGTH"] = str(length)
        return inner(environ, start_response)

    return wrapped


def _wrap_layout(inner: Callable, layout: Callable[[], str] | None) -> Callable:
    """Wrap HTML 2xx / 422 responses in the emitted layout. Redirects
    and non-HTML responses pass through. Mirrors the TS/rust layout-
    wrap middleware."""

    def wrapped(environ: dict[str, Any], start_response: Callable):
        captured: dict[str, Any] = {}

        def capture(status: str, headers: list[tuple[str, str]], exc_info=None):
            captured["status"] = status
            captured["headers"] = headers
            return lambda b: None  # WSGI write() — unused

        body_chunks = inner(environ, capture)
        body_bytes = b"".join(body_chunks)

        status = captured.get("status", "200 OK")
        headers = captured.get("headers", [])
        code_s = status.split(" ", 1)[0]
        try:
            code = int(code_s)
        except ValueError:
            code = 200

        ct = ""
        for k, v in headers:
            if k.lower() == "content-type":
                ct = v
                break
        is_html = ct.startswith("text/html")
        is_redirect = 300 <= code < 400

        if not is_html or is_redirect or layout is None:
            # Replace Content-Length if present — we may have buffered.
            headers = [(k, v) for (k, v) in headers if k.lower() != "content-length"]
            headers.append(("Content-Length", str(len(body_bytes))))
            start_response(status, headers)
            return [body_bytes]

        inner_text = body_bytes.decode("utf-8", errors="replace")
        _view_helpers.set_yield(inner_text)
        wrapped_text = layout()
        payload = wrapped_text.encode("utf-8")
        headers = [(k, v) for (k, v) in headers if k.lower() != "content-length"]
        headers.append(("Content-Length", str(len(payload))))
        start_response(status, headers)
        return [payload]

    return wrapped


def _parse_body(environ: dict[str, Any]) -> dict[str, Any]:
    """Parse a form-urlencoded body into a params dict. Handles the
    Rails convention of `article[title]=foo` bracket-notation so
    nested keys land at `params["article"]["title"]`."""
    ct = environ.get("CONTENT_TYPE", "")
    if not ct.startswith("application/x-www-form-urlencoded"):
        return {}
    length = int(environ.get("CONTENT_LENGTH") or 0)
    if length <= 0:
        return {}
    raw = environ["wsgi.input"].read(length)
    text = raw.decode("utf-8", errors="replace")
    parsed = parse_qs(text, keep_blank_values=True)
    out: dict[str, Any] = {}
    for k, vs in parsed.items():
        v = vs[-1] if vs else ""
        # `article[title]` → out["article"]["title"] = v
        if "[" in k and k.endswith("]"):
            outer, rest = k.split("[", 1)
            inner = rest[:-1]
            bucket = out.setdefault(outer, {})
            if isinstance(bucket, dict):
                bucket[inner] = v
                continue
        out[k] = v
    return out


def _status_phrase(code: int) -> str:
    return {
        200: "OK",
        201: "Created",
        204: "No Content",
        301: "Moved Permanently",
        302: "Found",
        303: "See Other",
        400: "Bad Request",
        404: "Not Found",
        422: "Unprocessable Entity",
        500: "Internal Server Error",
    }.get(code, "")


if __name__ == "__main__":
    # Running `python -m app.server` isn't the emitted entry point —
    # that's `app.main`. Keep this short stub so `python -m app.server`
    # yields a helpful error instead of a silent exit.
    sys.stderr.write(
        "run the emitted `python -m app` entry point instead of app.server directly\n"
    )
    sys.exit(2)
