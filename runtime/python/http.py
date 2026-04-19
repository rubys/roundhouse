# Roundhouse Python HTTP runtime — Phase 4d pass-2 shape.
#
# Hand-written, shipped alongside generated code (copied in by the
# Python emitter as `app/http.py`). Provides the controller-facing
# types + the Router match table; the test-support module imports
# Router.match to dispatch.
#
# Mirrors runtime/typescript/juntos.ts (the dynamic reference) in
# intent and shape: ActionResponse/ActionContext value types, a
# module-level Router with resources()/get()/post()/etc. builders
# and a match() dispatcher. Keeps the Phase-4c compile-only stubs
# (`render`, `redirect_to`, `head`, `respond_to`) so hand-written
# code outside the emitter still works.

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Callable


@dataclass
class ActionResponse:
    """Every generated controller action returns one of these.
    Fields are optional so actions pick only what they need:
        body: HTML string for GET actions
        status: HTTP status code (default 200)
        location: redirect target URL (for 3xx responses)
    """

    body: str = ""
    status: int = 200
    location: str = ""


@dataclass
class ActionContext:
    """Request context passed to every action. `params` merges path
    params (from the URL pattern) with form body fields."""

    params: dict[str, Any] = field(default_factory=dict)


class Params(dict):
    """Rails-style params bag. Subclasses `dict` so `params["key"]`
    access works directly. Phase 4c's `.expect(...)` is a no-op
    that returns `None`; Phase 4d controller templates read
    directly from `context.params` so this class is only used
    by hand-written code outside the emitter."""

    def expect(self, *args, **kwargs):
        return None


def params() -> Params:
    """Bare `params` in a controller body lowers here."""
    return Params()


# ── Router ────────────────────────────────────────────────

@dataclass
class _Route:
    method: str
    path: str
    handler: Callable[[ActionContext], ActionResponse]


class Router:
    """Process-wide route table. Generated `app/routes.py`
    registers handlers via `Router.resources(...)` / `Router.root
    (...)` / `Router.get/post/patch/delete(...)` at import time;
    `TestClient` in `test_support.py` dispatches through
    `Router.match(method, path)`.
    """

    _routes: list[_Route] = []

    @classmethod
    def reset(cls) -> None:
        cls._routes = []

    @classmethod
    def root(cls, *args) -> None:
        """Two forms:
            Router.root(controller, "action")
            Router.root("/", controller, "action")
        Either way the path is "/".
        """
        if len(args) == 3:
            path, controller, action = args
        else:
            path = "/"
            controller, action = args
        handler = _resolve_handler(controller, action)
        if handler is not None:
            cls._routes.append(_Route("GET", path, handler))

    @classmethod
    def resources(
        cls,
        name: str,
        controller: Any,
        only: list[str] | None = None,
        except_: list[str] | None = None,
        nested: list[dict] | None = None,
    ) -> None:
        cls._add_resource_routes(name, controller, only, except_, None)
        if nested:
            parent_singular = _singularize(name)
            for n in nested:
                cls._add_resource_routes(
                    n["name"],
                    n["controller"],
                    n.get("only"),
                    n.get("except"),
                    (parent_singular, name),
                )

    @classmethod
    def _add_resource_routes(
        cls,
        name: str,
        controller: Any,
        only: list[str] | None,
        except_: list[str] | None,
        scope: tuple[str, str] | None,
    ) -> None:
        standard = [
            ("index", "GET", ""),
            ("new", "GET", "/new"),
            ("create", "POST", ""),
            ("show", "GET", "/:id"),
            ("edit", "GET", "/:id/edit"),
            ("update", "PATCH", "/:id"),
            ("destroy", "DELETE", "/:id"),
        ]
        for action, method, suffix in standard:
            if only and action not in only:
                continue
            if except_ and action in except_:
                continue
            base = (
                f"/{scope[1]}/:{scope[0]}_id/{name}"
                if scope
                else f"/{name}"
            )
            path = f"{base}{suffix}"
            handler = _resolve_handler(controller, action)
            if handler is not None:
                cls._routes.append(_Route(method, path, handler))

    @classmethod
    def get(cls, path: str, controller: Any, action: str) -> None:
        h = _resolve_handler(controller, action)
        if h is not None:
            cls._routes.append(_Route("GET", path, h))

    @classmethod
    def post(cls, path: str, controller: Any, action: str) -> None:
        h = _resolve_handler(controller, action)
        if h is not None:
            cls._routes.append(_Route("POST", path, h))

    @classmethod
    def put(cls, path: str, controller: Any, action: str) -> None:
        h = _resolve_handler(controller, action)
        if h is not None:
            cls._routes.append(_Route("PUT", path, h))

    @classmethod
    def patch(cls, path: str, controller: Any, action: str) -> None:
        h = _resolve_handler(controller, action)
        if h is not None:
            cls._routes.append(_Route("PATCH", path, h))

    @classmethod
    def delete(cls, path: str, controller: Any, action: str) -> None:
        h = _resolve_handler(controller, action)
        if h is not None:
            cls._routes.append(_Route("DELETE", path, h))

    @classmethod
    def match(
        cls, method: str, path: str
    ) -> tuple[Callable[[ActionContext], ActionResponse], dict[str, str]] | None:
        """Resolve a (method, path) pair to a handler + path
        params. Used by TestClient; real HTTP dispatch is Phase
        4e."""
        for route in cls._routes:
            if route.method != method:
                continue
            p = _try_match_path(route.path, path)
            if p is not None:
                return route.handler, p
        return None


def _resolve_handler(controller: Any, action: str):
    """Look up a controller action by name. Uses `$new` mangling
    to avoid colliding with Python's `__new__` / `new` reserved
    vibes; the Python emitter writes `new` actions as `new_`
    functions on the controller module."""
    # Controllers are passed as modules (from `from app.controllers
    # import articles`). The emitter exports each action as a
    # module-level `async def <action>(context)`.
    name = action if action != "new" else "new_"
    return getattr(controller, name, None)


def _try_match_path(pattern: str, path: str) -> dict[str, str] | None:
    pat_parts = [p for p in pattern.split("/") if p]
    path_parts = [p for p in path.split("/") if p]
    if len(pat_parts) != len(path_parts):
        return None
    params: dict[str, str] = {}
    for p, v in zip(pat_parts, path_parts):
        if p.startswith(":"):
            params[p[1:]] = v
        elif p != v:
            return None
    return params


def _singularize(plural: str) -> str:
    """Minimal English singularizer for router-internal use —
    covers the scaffold blog's shapes (`articles` → `article`,
    `comments` → `comment`). Fuller inflection lives in the
    generator's naming module; this is enough for the runtime
    path."""
    if plural.endswith("ies"):
        return plural[:-3] + "y"
    if plural.endswith("ses"):
        return plural[:-2]
    if plural.endswith("s"):
        return plural[:-1]
    return plural


# ── Phase 4c compile-only stubs ───────────────────────────
# Hand-written code outside the emitter may still call these.
# The emitter itself no longer generates calls to them — the
# controller template returns `ActionResponse` directly.


def render(*args, **kwargs) -> ActionResponse:
    return ActionResponse()


def redirect_to(*args, **kwargs) -> ActionResponse:
    return ActionResponse()


def head(*args, **kwargs) -> ActionResponse:
    return ActionResponse()


class FormatRouter:
    """Phase 4c wires only the HTML branch."""

    def html(self, fn) -> ActionResponse:
        fn()
        return ActionResponse()

    def json(self, fn) -> ActionResponse:
        return ActionResponse()


def respond_to(fn) -> ActionResponse:
    fn(FormatRouter())
    return ActionResponse()
