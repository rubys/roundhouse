// Roundhouse TypeScript server runtime — libsql variant.
//
// Same shape as `server.ts` (HTTP + Action Cable glue) but opens
// the database via `@libsql/client` instead of better-sqlite3. The
// switch is opt-in via the `node-async` deployment profile; the
// emit pipeline picks this file (and `juntos-libsql.ts` alongside)
// when the active profile's database is async.
//
// Why libsql: native Promise API, supports remote (Turso) +
// embedded replicas + edge in addition to local file. The async-
// coloring pass in roundhouse already produces transpiled
// `Base.find/all/where/...` as `async` methods that `await
// adapter.X(...)` — this runtime swap is what makes those awaits
// load-bearing instead of trivial.
//
// Deltas vs `server.ts`:
//   - `Database from "better-sqlite3"` → `createClient from
//     "@libsql/client"`
//   - `openDatabase` becomes async (libsql `execute` is async)
//   - Schema DDL applied statement-by-statement via
//     `client.execute(stmt)` (libsql doesn't support multi-
//     statement batches in `execute`)
//   - Schema lookup goes through `pragma_table_info` the same way
//     (libsql implements pragma access)
//   - `installDb` accepts a Client, not a `Database.Database`
//
// Everything else (request dispatch, layout wrapping, Action
// Cable, params, session/flash plumbing) is target-mechanism-
// agnostic and identical to `server.ts`.

import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { mkdirSync, existsSync } from "node:fs";
import { dirname } from "node:path";
import { URL } from "node:url";
import { createClient, type Client } from "@libsql/client";

import { Router } from "./router.js";
import { Parameters } from "./parameters.js";
import { HashWithIndifferentAccess } from "./hash_with_indifferent_access.js";
import { setBroadcaster, installDb, type ActionResponse } from "./juntos.js";
import { ViewHelpers } from "./view_helpers.js";

// ── Action Cable server ────────────────────────────────────────

interface Subscriber {
  identifier: string;
  send: (json: string) => void;
}

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

async function handleRequest(req: IncomingMessage, res: ServerResponse): Promise<void> {
  const url = new URL(req.url ?? "/", `http://${req.headers.host ?? "localhost"}`);
  let method = (req.method ?? "GET").toUpperCase();

  let params: Record<string, string> = {};
  if (method !== "GET" && method !== "HEAD") {
    const raw = await readBody(req);
    const contentType = req.headers["content-type"] ?? "";
    if (contentType.includes("application/x-www-form-urlencoded")) {
      params = parseFormData(raw);
    } else if (contentType.includes("application/json") && raw) {
      try { params = JSON.parse(raw); } catch { /* ignore malformed */ }
    }
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

  const merged: Record<string, any> = { ...match.path_params.to_h() };
  for (const [k, v] of url.searchParams) merged[k] = v;
  Object.assign(merged, params);

  ViewHelpers.reset_slots_bang();

  let response: ActionResponse;
  try {
    const controller = new ctrlClass();
    controller.params = new Parameters(merged);
    controller.session = sessionStore;
    controller.flash = new HashWithIndifferentAccess(flashStore);
    controller.request_method = method;
    controller.request_path = url.pathname;
    await controller.process_action(match.action);
    flashStore = controller.flash ? controller.flash.to_h() : {};
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
    ViewHelpers.set_yield(response.body ?? "");
    res.end(layoutRenderer(response.body ?? ""));
  } else {
    res.end(renderLayout(response.body ?? ""));
  }
}

let layoutRenderer: ((body: string) => string) | null = null;

// ── Layout wrapping ────────────────────────────────────────────

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

async function attachCable(
  server: ReturnType<typeof createServer>,
  cable: CableServer,
): Promise<void> {
  const { WebSocketServer, WebSocket } = await import("ws");
  const wss = new WebSocketServer({
    server,
    path: "/cable",
    handleProtocols: (protocols: Set<string>) => {
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
      sub.identifier = data.identifier;
      let channel = "";
      try {
        const id = JSON.parse(data.identifier);
        const signed = String(id.signed_stream_name ?? "");
        const base64 = signed.split("--")[0];
        channel = JSON.parse(Buffer.from(base64, "base64").toString("utf8"));
      } catch {
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

/** Open a libsql Client at `dbPath`, apply schema DDL statement-
 *  by-statement, and install it via `juntos.installDb`. The libsql
 *  URL form `file:` is required for local file paths; bare paths
 *  parse as remote DB names. In-memory DBs use the `:memory:` URL
 *  alias. */
async function openDatabase(dbPath: string, schemaStatements: string[]): Promise<Client> {
  // libsql creates the file if it doesn't exist, but NOT the
  // parent directory — same constraint as better-sqlite3. Create
  // intermediate dirs so first-run startup works without manual
  // mkdir. Skip for `:memory:` (no parent).
  if (dbPath !== ":memory:") {
    const parent = dirname(dbPath);
    if (parent && parent !== "." && !existsSync(parent)) {
      mkdirSync(parent, { recursive: true });
    }
  }

  const url = dbPath === ":memory:" ? ":memory:" : `file:${dbPath}`;
  const client = createClient({ url });

  // libsql doesn't accept multi-statement scripts in `execute`,
  // so apply each `IF NOT EXISTS`-guarded DDL statement in turn.
  // PRAGMAs run first to match the better-sqlite3 setup. Note:
  // libsql's local mode honors WAL/foreign-key PRAGMAs; remote
  // (Turso) ignores some of them but the call itself is harmless.
  await client.execute("PRAGMA journal_mode = WAL");
  await client.execute("PRAGMA foreign_keys = ON");

  for (const stmt of schemaStatements) {
    await client.execute(stmt);
  }

  installDb(client);
  return client;
}

// ── Public entry point ─────────────────────────────────────────

export type RouteRow = Record<string, any>;
export type ControllerClass = new () => any;

export interface StartOptions {
  /** libsql URL or local file path. Defaults to
   *  `./db/development.sqlite3` (interpreted as `file:` URL). For
   *  Turso remote DBs, pass `libsql://<host>` or set via
   *  `TURSO_DATABASE_URL` env var; auth tokens go through the
   *  client constructor and are out of scope for this minimal
   *  startup helper (extend `StartOptions` when remote support is
   *  needed). */
  dbPath?: string;
  port?: number;
  schemaStatements: string[];
  /** Seeds run if `shouldSeed` returns true. Now `Promise<void>`-
   *  shaped because the libsql-backed transpiled `seeds.run()` is
   *  async (it `await`s adapter calls). */
  seeds?: () => Promise<void>;
  shouldSeed?: () => boolean;
  layout?: (body: string) => string;
  routes: RouteRow[];
  rootRoute?: RouteRow;
  controllers: Record<string, ControllerClass>;
}

let dispatchTable: RouteRow[] = [];
let controllerRegistry: Record<string, ControllerClass> = {};
const sessionStore: Record<string, any> = {};
let flashStore: Record<string, any> = {};

/** Start the server. Same lifecycle as `server.ts::startServer` but
 *  awaits the libsql DB open + schema apply before binding the
 *  HTTP listener. */
export async function startServer(opts: StartOptions): Promise<void> {
  const dbPath = opts.dbPath ?? "./db/development.sqlite3";
  const port = opts.port ?? Number(process.env.PORT ?? 3000);

  layoutRenderer = opts.layout ?? null;
  dispatchTable = opts.rootRoute ? [opts.rootRoute, ...opts.routes] : [...opts.routes];
  controllerRegistry = opts.controllers;

  await openDatabase(dbPath, opts.schemaStatements);

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
  console.log(`Roundhouse server (libsql) listening on http://localhost:${port}`);
}
