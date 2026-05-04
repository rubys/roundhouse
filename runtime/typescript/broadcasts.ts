// Broadcasts — turbo-stream emit bridge.
//
// The model lowerer's `broadcasts_to` expansion produces calls like
// `Broadcasts.prepend({ stream, target, html })` from inside model
// callback methods (`after_create`, `after_update`, etc.). This shim
// adapts those calls to the juntos broadcaster (whichever is
// installed via `setBroadcaster`).

import type { Broadcaster } from "./juntos.js";

let _broadcaster: Broadcaster | null = null;

/** Production server installs a broadcaster that pumps fragments out
 *  through the WebSocket cable. Tests / CLI runs leave this unset and
 *  the calls become silent no-ops. */
export function installBroadcastsBroadcaster(fn: Broadcaster | null): void {
  _broadcaster = fn;
}

interface BroadcastOpts {
  stream: string;
  target?: string;
  html?: string;
}

function emit(action: string, opts: BroadcastOpts): void {
  if (!_broadcaster) return;
  const target = opts.target ?? opts.stream;
  const html = opts.html ?? "";
  // Match the turbo-stream wire format the cable speaks: a tagged
  // `<turbo-stream action="…" target="…">…</turbo-stream>` element.
  const fragment =
    `<turbo-stream action="${action}" target="${target}">` +
    `<template>${html}</template>` +
    `</turbo-stream>`;
  _broadcaster(opts.stream, fragment);
}

/** Static-method facade matching the lowerer's typing stub
 *  (see `broadcasts_class_info` in `model_to_library`). Each method
 *  takes a kwargs-shaped hash and returns nothing. The action set
 *  matches `BroadcastAct` in the lowerer (Append/Prepend/Replace/
 *  Remove) — adding methods here without a matching variant there
 *  produces dead code. */
export class Broadcasts {
  static prepend(opts: BroadcastOpts): void {
    emit("prepend", opts);
  }
  static append(opts: BroadcastOpts): void {
    emit("append", opts);
  }
  static replace(opts: BroadcastOpts): void {
    emit("replace", opts);
  }
  static remove(opts: BroadcastOpts): void {
    emit("remove", opts);
  }
}
