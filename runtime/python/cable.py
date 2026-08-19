# Roundhouse Python cable runtime.
#
# Hand-written, shipped alongside generated code as `app/cable.py`.
# Implements the Action Cable WebSocket subprotocol (actioncable-v1-json)
# on top of aiohttp's WebSocketResponse, plus the `Broadcasts` Turbo
# Streams API the overlay's models call from their after_*_commit
# hooks — they render the partial inline and hand the html here.
#
# Mirrors runtime/rust/cable.rs and runtime/typescript/server.ts's
# cable handler. Same wire format. (The pre-overlay partial-renderer
# registry and `broadcast_*_to` helpers retired with the per-artifact
# model emit, 2026-08-19.)

from __future__ import annotations

import asyncio
import base64
import json
import time
from typing import Any

# aiohttp is imported only inside `cable_handler` — the rest of
# this module is duck-typed (calls `ws.send_str` / reads
# `ws.closed`) so model-only unit tests can import through
# `from app import cable` without having aiohttp installed. The
# broadcast dispatch path short-circuits when no subscribers are
# registered, which is the test-context state.

# ── Turbo Streams rendering ────────────────────────────────────


def turbo_stream_html(action: str, target: str, content: str) -> str:
    if content:
        return (
            f'<turbo-stream action="{action}" target="{target}">'
            f'<template>{content}</template></turbo-stream>'
        )
    return f'<turbo-stream action="{action}" target="{target}"></turbo-stream>'


# ── Subscriber registry + dispatch ─────────────────────────────

# channel name → list of (ws, identifier) pairs. Identifier is the
# raw subscribe-message `identifier` field echoed back on every
# broadcast so Turbo can route the frame to the matching
# <turbo-cable-stream-source> element. Typed as `Any` rather than
# `web.WebSocketResponse` so the module imports without aiohttp
# installed — the handler populates these at runtime and the
# broadcast path only calls `send_str` / `closed` (duck-typed).
_SUBSCRIBERS: dict[str, list[tuple[Any, str]]] = {}


def _dispatch(channel: str, html: str) -> None:
    """Schedule a broadcast frame for every subscriber on ``channel``.
    Called from model save/destroy paths which are synchronous;
    ``asyncio.ensure_future`` pushes the sends onto the running loop
    without blocking the caller. When no loop is running (model
    unit tests), the call silently no-ops."""
    subs = _SUBSCRIBERS.get(channel)
    if not subs:
        return
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        return
    frame_subs = list(subs)
    for ws, identifier in frame_subs:
        msg = json.dumps(
            {"type": "message", "identifier": identifier, "message": html}
        )
        asyncio.ensure_future(_safe_send(ws, msg))


async def _safe_send(ws: Any, msg: str) -> None:
    if ws.closed:
        return
    try:
        await ws.send_str(msg)
    except Exception:
        pass


# ── WebSocket handler ──────────────────────────────────────────


async def cable_handler(request: Any) -> Any:
    """aiohttp handler for ``GET /cable``. Negotiates the
    ``actioncable-v1-json`` subprotocol (Turbo's client requires it),
    sends the welcome frame, pings every 3s, and routes subscribe
    commands into ``_SUBSCRIBERS``. Cleans up on close.

    aiohttp is imported here rather than at module level so models
    can transitively import this module under the system Python
    (unit tests) without aiohttp installed — those tests never
    reach this handler."""
    from aiohttp import WSMsgType, web

    ws = web.WebSocketResponse(protocols=["actioncable-v1-json"])
    await ws.prepare(request)
    await ws.send_str(json.dumps({"type": "welcome"}))

    async def _ping() -> None:
        try:
            while not ws.closed:
                await asyncio.sleep(3)
                if ws.closed:
                    break
                await ws.send_str(
                    json.dumps({"type": "ping", "message": int(time.time())})
                )
        except Exception:
            pass

    ping_task = asyncio.create_task(_ping())
    sub_entries: list[tuple[str, tuple[Any, str]]] = []

    try:
        async for msg in ws:
            if msg.type != WSMsgType.TEXT:
                continue
            try:
                payload: Any = json.loads(msg.data)
            except Exception:
                continue
            if not isinstance(payload, dict):
                continue
            if payload.get("command") != "subscribe":
                continue
            identifier = payload.get("identifier")
            if not isinstance(identifier, str):
                continue
            channel = _decode_channel(identifier)
            if channel is None:
                continue
            entry = (ws, identifier)
            _SUBSCRIBERS.setdefault(channel, []).append(entry)
            sub_entries.append((channel, entry))
            await ws.send_str(
                json.dumps(
                    {"type": "confirm_subscription", "identifier": identifier}
                )
            )
    finally:
        ping_task.cancel()
        for channel, entry in sub_entries:
            subs = _SUBSCRIBERS.get(channel)
            if subs and entry in subs:
                subs.remove(entry)
            if subs is not None and not subs:
                _SUBSCRIBERS.pop(channel, None)

    return ws


def _decode_channel(identifier: str) -> str | None:
    """Recover the channel name from Turbo's signed_stream_name.
    The identifier is a JSON blob like
    ``{"channel":"Turbo::StreamsChannel","signed_stream_name":"<base64>--<digest>"}``;
    the base64 segment holds a JSON-encoded channel name. Invalid
    input returns None so the handler silently ignores it."""
    try:
        id_data = json.loads(identifier)
    except Exception:
        return None
    if not isinstance(id_data, dict):
        return None
    signed = id_data.get("signed_stream_name")
    if not isinstance(signed, str):
        return None
    b64 = signed.split("--", 1)[0]
    try:
        decoded = base64.b64decode(b64).decode("utf-8")
        value = json.loads(decoded)
    except Exception:
        return None
    return value if isinstance(value, str) else None


class Broadcasts:
    """Turbo Stream broadcast API the overlay's models call. The
    lowered `after_*_commit` callbacks render the partial themselves
    and pass the Ruby kwargs as real keyword arguments (`remove` omits
    `html`); each method wraps the html in a `<turbo-stream>` frame
    and pushes it to the stream's subscribers. Matches the
    go/rust/ts `Broadcasts` twins."""

    @staticmethod
    def prepend(*, stream: str, target: str, html: str) -> None:
        _dispatch(stream, turbo_stream_html("prepend", target, html))

    @staticmethod
    def append(*, stream: str, target: str, html: str) -> None:
        _dispatch(stream, turbo_stream_html("append", target, html))

    @staticmethod
    def replace(*, stream: str, target: str, html: str) -> None:
        _dispatch(stream, turbo_stream_html("replace", target, html))

    @staticmethod
    def remove(*, stream: str, target: str) -> None:
        _dispatch(stream, turbo_stream_html("remove", target, ""))
