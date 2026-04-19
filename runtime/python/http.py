# Roundhouse Python HTTP runtime — Phase 4c compile-only stubs.
#
# Hand-written, shipped alongside generated code (copied in by the
# Python emitter as `app/http.py`). Provides just enough surface that
# emitted controller actions parse cleanly: a `Response` class and
# module-level `render`, `redirect_to`, `head`, `respond_to`, `params`.
#
# Mirrors the five sibling stubs (Rust/Crystal/Go/Elixir/TS). Every
# call returns `Response()`. Python's dynamic typing means there's no
# type-checker to satisfy — the purpose is that `python -m compileall`
# accepts the file and controller tests stay `@unittest.skip`-ped.

from __future__ import annotations


class Response:
    """Opaque response value. Phase 4c stub."""

    pass


class Params(dict):
    """Request parameters. Subclasses `dict` so `params[:key]`-style
    access works directly. `.expect(...)` is a no-op stub."""

    def expect(self, *args, **kwargs):
        return None


def params() -> Params:
    """Bare ``params`` in a controller body lowers here."""
    return Params()


def render(*args, **kwargs) -> Response:
    return Response()


def redirect_to(*args, **kwargs) -> Response:
    return Response()


def head(*args, **kwargs) -> Response:
    return Response()


class FormatRouter:
    """Phase 4c wires only the HTML branch; the JSON branch is
    replaced at the call site with a ``# TODO: JSON branch`` comment."""

    def html(self, fn) -> Response:
        fn()
        return Response()

    def json(self, fn) -> Response:
        return Response()


def respond_to(fn) -> Response:
    """``respond_to do |format| ... end`` lowers to
    ``respond_to(lambda fr: ...)``."""
    fn(FormatRouter())
    return Response()
