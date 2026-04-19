# Roundhouse Python test-support runtime.
#
# Hand-written, shipped alongside generated code (copied in by the
# Python emitter as `app/test_support.py`). Controller tests call
# into `TestClient` for pure in-process HTTP dispatch (no real
# server, no socket setup) and wrap responses in `TestResponse`
# for Rails-compatible assertions.
#
# Mirrors runtime/typescript/test_support.ts in intent, shape, and
# assertion semantics — substring-match on the response body,
# loose but good-enough for the scaffold blog's HTML. A later
# phase can swap in a real parser (lxml, BeautifulSoup) by
# touching only this file; emitted test call sites stay stable.

from __future__ import annotations

import asyncio
import inspect
from typing import Any

from . import http as _http


class TestResponse:
    """Wrapper around `ActionResponse` exposing Rails-Minitest-
    compatible assertion helpers. Method names mirror the Ruby
    source (snake_case); bodies substring-match for
    `assert_select`-style queries."""

    def __init__(self, raw: _http.ActionResponse):
        self.body: str = raw.body or ""
        self.status: int = raw.status or 200
        self.location: str = raw.location or ""

    def assert_ok(self) -> None:
        if self.status != 200:
            raise AssertionError(f"expected 200 OK, got {self.status}")

    def assert_unprocessable(self) -> None:
        if self.status != 422:
            raise AssertionError(
                f"expected 422 Unprocessable Entity, got {self.status}"
            )

    def assert_status(self, code: int) -> None:
        if self.status != code:
            raise AssertionError(f"expected status {code}, got {self.status}")

    def assert_redirected_to(self, path: str) -> None:
        if not (300 <= self.status < 400):
            raise AssertionError(f"expected a redirection, got {self.status}")
        if path not in self.location:
            raise AssertionError(
                f"expected Location to contain {path!r}, got {self.location!r}"
            )

    def assert_select(self, selector: str) -> None:
        fragment = _selector_fragment(selector)
        if fragment not in self.body:
            raise AssertionError(
                f"expected body to match selector {selector!r} "
                f"(looked for {fragment!r})"
            )

    def assert_select_text(self, selector: str, text: str) -> None:
        self.assert_select(selector)
        if str(text) not in self.body:
            raise AssertionError(
                f"expected body to contain text {text!r} "
                f"under selector {selector!r}"
            )

    def assert_select_min(self, selector: str, n: int) -> None:
        fragment = _selector_fragment(selector)
        count = 0
        start = 0
        while True:
            i = self.body.find(fragment, start)
            if i < 0:
                break
            count += 1
            start = i + len(fragment)
        if count < n:
            raise AssertionError(
                f"expected at least {n} matches for selector "
                f"{selector!r}, got {count}"
            )


class TestClient:
    """Pure in-process HTTP client — dispatches through
    `http.Router.match` directly. No real HTTP, no asyncio event
    loop (actions that return awaitables get awaited here so
    tests can stay synchronous)."""

    def get(self, path: str) -> TestResponse:
        return self._dispatch("GET", path, {})

    def post(self, path: str, body: dict[str, Any] | None = None) -> TestResponse:
        return self._dispatch("POST", path, body or {})

    def patch(self, path: str, body: dict[str, Any] | None = None) -> TestResponse:
        return self._dispatch("PATCH", path, body or {})

    def put(self, path: str, body: dict[str, Any] | None = None) -> TestResponse:
        return self._dispatch("PUT", path, body or {})

    def delete(self, path: str) -> TestResponse:
        return self._dispatch("DELETE", path, {})

    def _dispatch(
        self, method: str, path: str, body: dict[str, Any]
    ) -> TestResponse:
        matched = _http.Router.match(method, path)
        if matched is None:
            raise AssertionError(f"no route for {method} {path}")
        handler, path_params = matched
        params: dict[str, Any] = {}
        params.update(path_params)
        params.update(body)
        context = _http.ActionContext(params=params)
        result = handler(context)
        if inspect.isawaitable(result):
            result = asyncio.get_event_loop().run_until_complete(result)
        if not isinstance(result, _http.ActionResponse):
            result = _http.ActionResponse()
        return TestResponse(result)


def _selector_fragment(selector: str) -> str:
    """Turn a loose selector into a substring fragment that
    probably appears in matching HTML. Same rules as TS/Rust
    twins:
        "#id"    → 'id="id"'
        ".cls"   → 'cls"'
        "tag"    → "<tag"
    Compound selectors pick the first chunk.
    """
    first = selector.split()[0] if selector.split() else ""
    if first.startswith("#"):
        return f'id="{first[1:]}"'
    if first.startswith("."):
        return f'{first[1:]}"'
    return f"<{first}"
