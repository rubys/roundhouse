// Roundhouse TypeScript server runtime — HTTP + Action Cable glue.
//
// The emitted `main.ts` imports `startServer` from here (via
// tsconfig path mapping to `runtime/typescript/server.ts`) and
// hands it the Router's match function. startServer:
//   1. Opens a file-backed better-sqlite3 database.
//   2. Runs schema DDL (from the generated schema_sql.ts).
//   3. Installs the Action Cable broadcaster on ApplicationRecord.
//   4. Starts an HTTP listener that routes requests through
//      Router.match → ActionContext → controller action →
//      ActionResponse → HTTP response.
//   5. Upgrades WebSocket connections on `/cable` into
//      Action Cable clients with the `actioncable-v1-json`
//      subprotocol, pings every 3s, and broadcasts turbo-stream
//      fragments to subscribed channels.
//
// Based on railcar's TS app.ts pattern, adapted to roundhouse's
// emitted Router / ActionContext / ActionResponse shapes.

import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { mkdirSync, existsSync } from "node:fs";
import { dirname } from "node:path";
import { URL } from "node:url";
import Database from "better-sqlite3";

import { Router } from "./router.js";
import { Parameters } from "./parameters.js";
import { setBroadcaster, installDb, type ActionResponse } from "./juntos.js";
import { ViewHelpers } from "./view_helpers.js";

// ── Action Cable server ────────────────────────────────────────

/** Minimal Action Cable subscriber bookkeeping. `identifier` is
 *  the opaque string the client sent in its `subscribe` command;
 *  `send` writes a JSON-framed message back to the client. */
interface Subscriber {
  identifier: string;
  send: (json: string) => void;
}

/** Tracks subscriptions per channel. Model lifecycle callbacks
 *  call `broadcast(channel, html)` to push a Turbo Stream
 *  fragment to every subscriber of that channel. */
class CableServer {
  private channels: Map<string, Set<Subscriber>> = new Map();

  subscribe(channel: string, sub: Subscriber): void {
    if (!this.channels.has(channel)) this.channels.set(channel, new Set());
    this.channels.get(channel)!.add(sub);
  }

  unsubscribe(sub: Subscriber): void {
    for (const subs of this.channels.values()) subs.delete(sub);
  }

  broadcast(channel: string, html: string): void {
    const subs = this.channels.get(channel);
    if (!subs) return;
    for (const sub of subs) {
      sub.send(JSON.stringify({
        type: "message",
        identifier: sub.identifier,
        message: html,
      }));
    }
  }
}

// ── Form data parsing ──────────────────────────────────────────

/** Parse `application/x-www-form-urlencoded` body into a flat
 *  object. Rails scaffold forms send data in this format; this
 *  parser handles the shapes we need (`article[title]=...&
 *  article[body]=...&_method=delete`) without an external
 *  dependency. Keys with `[nested]` brackets become the literal
 *  bracketed string — the emitter's controller code already
 *  reads `context.params["article[title]"]` directly. */
function parseFormData(body: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const pair of body.split("&")) {
    if (!pair) continue;
    const eq = pair.indexOf("=");
    const key = decodeURIComponent((eq < 0 ? pair : pair.slice(0, eq)).replace(/\+/g, " "));
    const val = eq < 0 ? "" : decodeURIComponent(pair.slice(eq + 1).replace(/\+/g, " "));
    out[key] = val;
  }
  return out;
}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on("data", (c: Buffer) => chunks.push(c));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

// ── HTTP request dispatcher ────────────────────────────────────

/** Handle one HTTP request: parse body, route to a controller
 *  action via Router.match, translate the returned ActionResponse
 *  into the outgoing HTTP response. Rails method-override via
 *  `_method=delete|patch|put` hidden field is honored. */
async function handleRequest(req: IncomingMessage, res: ServerResponse): Promise<void> {
  const url = new URL(req.url ?? "/", `http://${req.headers.host ?? "localhost"}`);
  let method = (req.method ?? "GET").toUpperCase();

  // Collect form body for non-GET requests.
  let params: Record<string, string> = {};
  if (method !== "GET" && method !== "HEAD") {
    const raw = await readBody(req);
    const contentType = req.headers["content-type"] ?? "";
    if (contentType.includes("application/x-www-form-urlencoded")) {
      params = parseFormData(raw);
    } else if (contentType.includes("application/json") && raw) {
      try { params = JSON.parse(raw); } catch { /* ignore malformed */ }
    }
    // Method override: POST with `_method=delete` dispatches as DELETE.
    const override = (params._method ?? "").toUpperCase();
    if (method === "POST" && (override === "DELETE" || override === "PATCH" || override === "PUT")) {
      method = override;
      delete params._method;
    }
  }

  const match = Router.match(method, url.pathname, dispatchTable);
  if (!match) {
    res.statusCode = 404;
    res.setHeader("Content-Type", "text/plain; charset=utf-8");
    res.end(`Not Found: ${method} ${url.pathname}`);
    return;
  }

  const ctrlClass = controllerRegistry[match.controller];
  if (!ctrlClass) {
    res.statusCode = 500;
    res.setHeader("Content-Type", "text/plain; charset=utf-8");
    res.end(`No controller registered: ${match.controller}`);
    return;
  }

  // Merge: path params + query string + form/json body.
  const merged: Record<string, any> = { ...match.path_params };
  for (const [k, v] of url.searchParams) merged[k] = v;
  Object.assign(merged, params);

  // Reset per-request render state (yield body, content_for
  // slots) so nothing leaks across requests.
  ViewHelpers.reset_slots_bang();

  let response: ActionResponse;
  try {
    const controller = new ctrlClass();
    controller.params = new Parameters(merged);
    controller.session = sessionStore;
    controller.flash = flashStore;
    controller.request_method = method;
    controller.request_path = url.pathname;
    await controller.process_action(match.action);
    // Rails carries flash forward exactly once: the action that
    // sets `flash[:notice] = ...` then `redirect_to`s, the next
    // request reads the notice, and a request after that sees an
    // empty flash. Mirror that with a per-request swap — keep the
    // current flash if the action set anything, replace with a
    // fresh hash for the next request.
    flashStore = controller.flash ?? {};
    response = {
      body: controller.body,
      status: controller.status,
      location: controller.location,
    };
  } catch (err) {
    console.error("handler error:", err);
    res.statusCode = 500;
    res.setHeader("Content-Type", "text/plain; charset=utf-8");
    res.end(`Server error: ${(err as Error).message}`);
    return;
  }

  if (response.location) {
    res.statusCode = response.status ?? 303;
    res.setHeader("Location", response.location);
    res.end();
    return;
  }

  res.statusCode = response.status ?? 200;
  res.setHeader("Content-Type", "text/html; charset=utf-8");
  if (layoutRenderer) {
    // Pass body explicitly to the layout — matches the lowered-IR
    // shape (`def self.application(body) ... io << body ... end`).
    // The slot-store `setYield` is still populated for layouts that
    // also need `<% yield :head %>` / `<% yield :alt %>` style
    // named-yield reads.
    ViewHelpers.set_yield(response.body ?? "");
    res.end(layoutRenderer(response.body ?? ""));
  } else {
    res.end(renderLayout(response.body ?? ""));
  }
}

/** Per-process layout renderer. Set by `startServer` via
 *  `opts.layout`; the emitted `main.ts` passes the transpiled
 *  `renderLayoutsApplication` here so the dispatcher wraps each
 *  view in the real Rails layout (reading the yield body and any
 *  `content_for` slots via the module-level state in
 *  view_helpers). When unset, `renderLayout` below provides a
 *  minimal fallback so the server still renders in isolation. */
let layoutRenderer: ((body: string) => string) | null = null;

// ── Layout wrapping ────────────────────────────────────────────

/** Wrap a controller's returned HTML body in the full HTML
 *  document shell. Only used for fixtures without a
 *  `layouts/application` ERB template (e.g. tiny-blog); apps with
 *  a layout reach this file's `renderLayouts_application` instead
 *  via the emitter-supplied `opts.layout` callback.
 *
 *   - Tailwind Play CDN (`cdn.tailwindcss.com`): compiles utility
 *     classes in the browser. Fine for dev; production swaps in
 *     a real tailwind-cli build.
 *   - `@hotwired/turbo` via importmap: provides Turbo's form
 *     submission + Stream subscription. Matches the Rust
 *     runtime's fallback shape — no `action-cable-url` meta
 *     (the `@rails/actioncable` default `/cable` is what our
 *     cable handler listens on) and plain turbo (not
 *     turbo-rails) since we don't need the Rails-specific
 *     helpers here.
 */
function renderLayout(body: string): string {
  return `<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8">
    <title>Roundhouse App</title>
    <meta name="viewport" content="width=device-width,initial-scale=1">
    <link rel="icon" href="data:,">
    <script src="https://cdn.tailwindcss.com"></script>
    <script type="importmap">
    {
      "imports": {
        "@hotwired/turbo": "https://ga.jspm.io/npm:@hotwired/turbo@8.0.0/dist/turbo.es2017-esm.js"
      }
    }
    </script>
    <script type="module">import "@hotwired/turbo";</script>
  </head>
  <body>
    <main class="container mx-auto mt-8 px-5 flex flex-col">
      ${body}
    </main>
  </body>
</html>
`;
}

// ── Action Cable upgrade handling ──────────────────────────────

/** Minimal Action Cable handshake + framing. The browser's
 *  `@rails/actioncable` client sends `Sec-WebSocket-Protocol:
 *  actioncable-v1-json`; we echo it back. Standard messages:
 *  welcome on connect, ping every 3s, confirm_subscription on
 *  subscribe, regular JSON-message broadcasts to subscribers.
 *
 *  Dynamic-import of `ws` so the package is optional — the
 *  emitted project only pulls it in when it actually runs a
 *  server. */
async function attachCable(
  server: ReturnType<typeof createServer>,
  cable: CableServer,
): Promise<void> {
  const { WebSocketServer, WebSocket } = await import("ws");
  const wss = new WebSocketServer({
    server,
    path: "/cable",
    handleProtocols: (protocols: Set<string>) => {
      // Echo the Action Cable subprotocol if the client requested
      // it; falls back to the first offered protocol for safety.
      if (protocols.has("actioncable-v1-json")) return "actioncable-v1-json";
      return protocols.values().next().value ?? false;
    },
  });

  wss.on("connection", (ws) => {
    ws.send(JSON.stringify({ type: "welcome" }));

    const ping = setInterval(() => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: "ping", message: Date.now() }));
      }
    }, 3000);

    const sub: Subscriber = {
      identifier: "",
      send: (json: string) => {
        if (ws.readyState === WebSocket.OPEN) ws.send(json);
      },
    };

    ws.on("message", (raw: Buffer) => {
      let data: any;
      try { data = JSON.parse(raw.toString()); } catch { return; }
      if (data.command !== "subscribe") return;
      // Identifier is a JSON-encoded blob of the stream name.
      // Turbo signs the stream name with a base64 prefix; we
      // decode the base64 to recover the channel name for
      // broadcast routing.
      sub.identifier = data.identifier;
      let channel = "";
      try {
        const id = JSON.parse(data.identifier);
        const signed = String(id.signed_stream_name ?? "");
        const base64 = signed.split("--")[0];
        channel = JSON.parse(Buffer.from(base64, "base64").toString("utf8"));
      } catch {
        // If we can't decode a turbo signed stream name, fall
        // back to using the raw identifier as the channel — lets
        // tests subscribe directly by stream name.
        channel = String(data.identifier);
      }
      cable.subscribe(channel, sub);
      ws.send(JSON.stringify({
        type: "confirm_subscription",
        identifier: sub.identifier,
      }));
    });

    ws.on("close", () => {
      clearInterval(ping);
      cable.unsubscribe(sub);
    });
  });
}

// ── Database + schema bootstrap ────────────────────────────────

function openDatabase(dbPath: string, schemaStatements: string[]): void {
  // better-sqlite3 creates the file if it doesn't exist, but
  // NOT the parent directory — if we're opening `./db/
  // development.sqlite3` and `./db/` doesn't exist, the
  // constructor throws. Create intermediate dirs so first-run
  // startup works without the user having to mkdir manually.
  // In-memory DBs (`:memory:`) don't have a parent dir; skip.
  if (dbPath !== ":memory:") {
    const parent = dirname(dbPath);
    if (parent && parent !== "." && !existsSync(parent)) {
      mkdirSync(parent, { recursive: true });
    }
  }

  // We open with WAL + foreign keys, apply the schema
  // statement-by-statement, and hand the connection to the juntos
  // runtime via installDb. All subsequent AR queries run against
  // this connection. Per-statement execution is the portable form;
  // each statement is `IF NOT EXISTS`-guarded by the lowerer so
  // re-opening an existing DB is a no-op.
  const db = new Database(dbPath);
  db.exec("PRAGMA journal_mode = WAL");
  db.exec("PRAGMA foreign_keys = ON");

  for (const stmt of schemaStatements) {
    db.exec(stmt);
  }

  installDb(db);
}

// ── Public entry point ─────────────────────────────────────────

/** One row from the emitted `Routes.table()` / `Routes.root()`. The
 *  routes lowerer emits these as symbol-keyed Ruby hashes whose TS
 *  rendering is `Record<string, any>` — TS doesn't narrow Record to
 *  a struct shape, so we mirror that here for type compatibility
 *  with the lowered output. Reads always go through `route["method"]`
 *  etc., never struct member access. */
export type RouteRow = Record<string, any>;

/** Constructable controller shape — bare class. Each emitted
 *  controller exports its class; `main.ts` builds a
 *  `{ articles: ArticlesController, ... }` map keyed by the
 *  controller_symbol form (`ArticlesController` → `articles`)
 *  the routes table uses. Constructed per-request, fields set
 *  from path/query/body params, then `process_action(action)`.
 *
 *  Typed `any` for the construct return so the emitted controllers'
 *  declared field types don't fight this contract — controllers
 *  declare `body: string`, `status: any`, etc. in different
 *  combinations across the kind-agnostic emit, and TS variance on
 *  construct signatures is invariant. The runtime expectation is
 *  documented in the dispatcher: each controller carries `params`,
 *  `session`, `flash`, `request_method`, `request_path`, `body`,
 *  `status`, `location` plus `process_action(action)`. */
export type ControllerClass = new () => any;

export interface StartOptions {
  /** File path for the sqlite DB. Defaults to `./db/development.sqlite3`. */
  dbPath?: string;
  /** HTTP port. Defaults to 3000 or `PORT` env var. */
  port?: number;
  /** Schema DDL statements to apply on startup, one per CREATE
   *  TABLE / CREATE INDEX. The generated `Schema.statements()`
   *  module returns this list. Each statement is `IF NOT EXISTS`-
   *  guarded so re-opening an existing DB is a no-op. */
  schemaStatements: string[];
  /** Optional seed function, run if `shouldSeed` returns true. */
  seeds?: () => void | Promise<void>;
  /** Predicate controlling whether to run seeds. Default: run if
   *  the database's first AR table is empty. Emitter can override
   *  with model-specific logic. */
  shouldSeed?: () => boolean;
  /** Layout renderer — the emitted `renderLayoutsApplication`
   *  (or equivalent). Called after each non-redirect response
   *  with the inner view body as the first arg (matches the
   *  lowered-IR shape `def self.application(body) ... io << body
   *  ... end`); the slot store is also populated via
   *  `Helpers.setYield` for named-yield reads. When omitted, the
   *  server falls back to
   *  the minimal `renderLayout` shell below. */
  layout?: (body: string) => string;
  /** Routes table — `Routes.table()` from the emitted
   *  `app/routes.ts`. Order is config/routes.rb order. */
  routes: RouteRow[];
  /** Optional root route — `Routes.root()` from the emitted
   *  `app/routes.ts`. Composed at the head of `routes` so a
   *  GET `/` matches before fallthroughs. */
  rootRoute?: RouteRow;
  /** Map from controller-symbol (`articles` for `ArticlesController`)
   *  to the controller class. Built by `main.ts` from the imported
   *  controller modules. */
  controllers: Record<string, ControllerClass>;
}

// Per-process dispatch table — set by `startServer`. Composed
// `[rootRoute, ...routes]` so GET `/` matches before fallthroughs.
let dispatchTable: RouteRow[] = [];
let controllerRegistry: Record<string, ControllerClass> = {};
// Persistent session/flash stores. Real Rails session/flash carry
// per-cookie scoping; this minimal stub is process-global so the
// scaffold blog's flash-after-redirect works in single-user dev.
const sessionStore: Record<string, any> = {};
let flashStore: Record<string, any> = {};

/** Start the server. Returns a promise that resolves once the
 *  HTTP + WebSocket listeners are accepting connections. */
export async function startServer(opts: StartOptions): Promise<void> {
  const dbPath = opts.dbPath ?? "./db/development.sqlite3";
  const port = opts.port ?? Number(process.env.PORT ?? 3000);

  layoutRenderer = opts.layout ?? null;
  dispatchTable = opts.rootRoute ? [opts.rootRoute, ...opts.routes] : [...opts.routes];
  controllerRegistry = opts.controllers;

  openDatabase(dbPath, opts.schemaStatements);

  if (opts.seeds && (opts.shouldSeed ?? (() => true))()) {
    await opts.seeds();
  }

  const cable = new CableServer();
  setBroadcaster((stream, html) => cable.broadcast(stream, html));

  const server = createServer((req, res) => {
    handleRequest(req, res).catch((err) => {
      console.error("request error:", err);
      res.statusCode = 500;
      res.end("Server error");
    });
  });

  await attachCable(server, cable);

  await new Promise<void>((resolve) => server.listen(port, () => resolve()));
  console.log(`Roundhouse server listening on http://localhost:${port}`);
}
