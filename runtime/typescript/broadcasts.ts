// Broadcasts — turbo-stream emit bridge.
//
// The model lowerer's `broadcasts_to` expansion produces calls like
// `Broadcasts.prepend({ stream, target, html })` from inside model
// callback methods (`after_create`, `after_update`, etc.). This shim
// wraps those calls in the turbo-stream wire format and delegates
// to whatever `broadcast(stream, html)` is provided by the active
// `juntos.ts` variant:
//
//   - juntos.ts (better-sqlite3 / sync): null until the HTTP server
//     installs a CableServer-backed broadcaster via `setBroadcaster`.
//     Pre-install calls (and test runs that never install) are
//     silent no-ops, matching the prior shim behavior.
//   - juntos-libsql.ts (libsql / async): same shape — the server
//     installs a CableServer-backed broadcaster.
//   - juntos-worker.ts (SharedWorker): a default
//     BroadcastChannel-backed broadcaster is installed at module
//     load (no setBroadcaster needed); broadcasts reach all tabs
//     of the same origin natively.
//
// One slot, one install path. Previously this file kept its own
// `_broadcaster` slot and exposed `installBroadcastsBroadcaster`
// — neither was wired up anywhere, making `broadcasts_to` a silent
// no-op on every target. Surfaced by the multi-tab smoke probe.

import { broadcast } from "./juntos.js";

interface BroadcastOpts {
  stream: string;
  target?: string;
  html?: string;
  // Rendered attribute text carrying its own leading space —
  // ` maintain_scroll="true"`. Composed by the broadcast lowering, where
  // the literal hash the app wrote can be read; a string rather than a
  // hash so no target needs a markup renderer of its own.
  attributes?: string;
}

function emit(action: string, opts: BroadcastOpts): void {
  const target = opts.target ?? opts.stream;
  const html = opts.html ?? "";
  // Match the turbo-stream wire format the cable speaks: a tagged
  // `<turbo-stream action="…" target="…">…</turbo-stream>` element.
  // `attributes` rides ahead of `action`/`target`, where turbo-rails'
  // `tag.turbo_stream(template, **attributes, action:, target:)` puts it.
  const attributes = opts.attributes ?? "";
  const fragment =
    `<turbo-stream${attributes} action="${action}" target="${target}">` +
    `<template>${html}</template>` +
    `</turbo-stream>`;
  broadcast(opts.stream, fragment);
}

/** Static-method facade matching the lowerer's typing stub
 *  (see `broadcasts_class_info` in `model_to_library`). Each method
 *  takes a kwargs-shaped hash and returns nothing. The action set
 *  matches `BroadcastAct` in the lowerer (Append/Prepend/Replace/
 *  Remove) — adding methods here without a matching variant there
 *  produces dead code. */
export class Broadcasts {
  // Compose the `<turbo-stream>` element for a `turbo_stream.<action>`
  // call in a `.turbo_stream.erb` template. Positional and String-typed on
  // purpose: it is the ONE shape the view lowerer emits on every target
  // (the older keyword/Symbol `render_fragment` spellings had drifted
  // apart between targets).
  //
  // `attributes` carries its own leading space and is written BEFORE
  // `action`/`target`, where turbo-rails' `tag.turbo_stream(template,
  // **attributes, action:, target:)` puts it. Optional, so the
  // three-argument call the view lowerer emits is unchanged.
  static turbo_stream_fragment(
    action: string,
    target: string,
    html: string,
    attributes: string = "",
  ): string {
    if (action === "remove") {
      return `<turbo-stream${attributes} action="remove" target="${target}"></turbo-stream>`;
    }
    return `<turbo-stream${attributes} action="${action}" target="${target}"><template>${html}</template></turbo-stream>`;
  }

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
