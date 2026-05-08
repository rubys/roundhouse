// Roundhouse TypeScript server runtime — SharedWorker variant.
//
// Same shape as `server-libsql.ts` (request dispatch, layout
// wrapping, params/session/flash plumbing) but the request transport
// is `onconnect`/MessagePort fetch messages from connected tabs
// instead of `node:http`. The database lives in a dedicated DB
// Worker (see `db_worker.ts`) and is reached via MessagePort through
// the worker-backed `ActiveRecordAdapter` in `juntos-worker.ts`.
//
// Deltas vs `server-libsql.ts`:
//   - `createServer` / `IncomingMessage` / `ServerResponse` →
//     `self.onconnect` listener + MessagePort `fetch` protocol;
//     responses serialized as `{ status, headers, body }`.
//   - `openDatabase` → `spawnDbWorker` + `sendDbInit` +
//     `installDb(workerHandle)`. Schema DDL is applied through
//     `execSQL` from `juntos-worker.ts` (round-trips through
//     `db_worker.ts`).
//   - `attachCable` (WebSocket Action Cable) → no-op. Broadcasting
//     is done in the framework runtime via `juntos-worker.ts`'s
//     default `BroadcastChannel`-backed broadcaster, which natively
//     reaches all tabs sharing this SharedWorker.
//   - `installDb(client)` → `installDb(workerHandle)` (Worker or
//     MessagePort, both have `postMessage`/`addEventListener`).
//
// Chrome workaround: Firefox lets a SharedWorker spawn a dedicated
// Worker directly; Chrome doesn't expose the `Worker` constructor
// inside `SharedWorkerGlobalScope`. When direct spawn fails, ask a
// connected tab to create the dedicated Worker on our behalf and
// hand back a MessageChannel port that proxies the same interface.
// Tab-host re-spawn on host-tab close is driven by a
// `juntos:lifecycle` BroadcastChannel.

import { Router } from "./router.js";
import { Parameters } from "./parameters.js";
import { HashWithIndifferentAccess } from "./hash_with_indifferent_access.js";
// Note: this file is emitted to `src/server.ts` and the
// `juntos-worker.ts` source is emitted to `src/juntos.ts` — same
// rename pattern as `juntos-libsql.ts` → `juntos.ts`. Imports
// reference the emitted name.
import { installDb, execSQL, type ActionResponse } from "./juntos.js";
import { ViewHelpers } from "./view_helpers.js";

// ── SharedWorker global scope declaration ──

declare const self: SharedWorkerGlobalScope;

// Common shape for both `Worker` (Firefox direct) and `MessagePort`
// (Chrome tab-delegated) — both expose `postMessage` +
// `addEventListener("message", ...)`.
type DbWorkerHandle = Pick<
  Worker,
  "postMessage" | "addEventListener" | "removeEventListener"
>;

// ── Public option types ──

export type RouteRow = Record<string, any>;
export type ControllerClass = new () => any;

export interface StartOptions {
  /** Database name passed to `db_worker`'s sqlite-wasm engine
   *  (used as the OPFS file path). Default: `app.sqlite3`. */
  database?: string;
  /** Per-statement schema DDL applied at startup via `execSQL`. */
  schemaStatements: string[];
  /** Seeds run if `shouldSeed` returns true. Async — the
   *  worker-backed adapter is async-only. */
  seeds?: () => Promise<void>;
  shouldSeed?: () => boolean;
  layout?: (body: string) => string;
  routes: RouteRow[];
  rootRoute?: RouteRow;
  controllers: Record<string, ControllerClass>;
}

// ── Module-level state ──

let _layoutRenderer: ((body: string) => string) | null = null;
let _dispatchTable: RouteRow[] = [];
let _controllerRegistry: Record<string, ControllerClass> = {};
const _sessionStore: Record<string, any> = {};
let _flashStore: Record<string, any> = {};

// Connected tab ports. We keep the full set for broadcasting `ready`
// + error notifications; `_tabPorts` is the indexed view used by the
// lifecycle channel to find the host tab when a Worker proxy needs
// to be re-hosted.
const _ports = new Set<MessagePort>();
const _tabPorts = new Map<string, MessagePort>();

// Dedicated DB Worker handle + spawn metadata.
let _dbWorker: DbWorkerHandle | null = null;
let _dbWorkerUrl: string | null = null;
let _dbWorkerHostId: string | null = null;
let _canCreateWorker: boolean | null = null;
let _databaseName = "app.sqlite3";

let _ready = false;

// ── Public entry ──

/** Start the SharedWorker application: register routes/controllers,
 *  spawn the dedicated DB Worker, apply schema DDL, run seeds, then
 *  signal `ready` to all connected tabs and dispatch their
 *  subsequent `fetch` messages. */
export async function startApplication(opts: StartOptions): Promise<void> {
  _layoutRenderer = opts.layout ?? null;
  _dispatchTable = opts.rootRoute ? [opts.rootRoute, ...opts.routes] : [...opts.routes];
  _controllerRegistry = opts.controllers;
  _databaseName = opts.database ?? "app.sqlite3";

  // The first connecting tab delivers the dbWorkerUrl in its
  // `config` message — built at Vite-build time and injected via
  // `<meta name="juntos-db-worker">`. Wait for that before spawning.
  let configResolve!: () => void;
  const configReady = new Promise<void>((r) => { configResolve = r; });

  // Listen for tab connections immediately — before any await — so
  // the very first connecting tab's onconnect event isn't missed.
  self.onconnect = (event: MessageEvent) => {
    const port = event.ports[0];
    if (!port) return;
    _ports.add(port);

    port.onmessage = (ev: MessageEvent) => {
      void onTabMessage(port, ev.data, configResolve);
    };

    port.onmessageerror = () => handleTabDisconnect(port);
    port.start();

    if (_ready) port.postMessage({ type: "ready" });
  };

  // Lifecycle channel: tabs announce close via `beforeunload` so we
  // can release their port and respawn the DB Worker if the closing
  // tab was hosting it.
  const lifecycle = new BroadcastChannel("juntos:lifecycle");
  lifecycle.onmessage = ({ data }) => {
    if (data?.type !== "tab-closing" || !data.tabId) return;
    const port = _tabPorts.get(data.tabId);
    if (port) {
      _tabPorts.delete(data.tabId);
      _ports.delete(port);
    }
    if (data.tabId === _dbWorkerHostId) {
      _dbWorkerHostId = null;
      respawnDbWorker().catch((e) => console.error("respawn failed:", e));
    }
  };

  try {
    await configReady;
    await spawnDbWorker();
    await sendDbInit(_databaseName);
    installDb(_dbWorker!);

    for (const stmt of opts.schemaStatements) {
      await execSQL(stmt);
    }

    if (opts.seeds && (opts.shouldSeed ?? (() => true))()) {
      await opts.seeds();
    }

    _ready = true;
    for (const port of _ports) port.postMessage({ type: "ready" });
    console.log("SharedWorker started");
  } catch (e) {
    console.error("SharedWorker initialization failed:", e);
    const error = e instanceof Error ? e.message : String(e);
    for (const port of _ports) port.postMessage({ type: "error", error });
  }
}

// ── Tab message dispatch ──

interface ConfigMessage {
  type: "config";
  dbWorkerUrl?: string;
  tabId?: string;
}

interface FetchMessage {
  type: "fetch";
  id: string;
  method?: string;
  url: string;
  headers?: Record<string, string>;
  body?: string | null;
}

type IncomingTabMessage = ConfigMessage | FetchMessage | { type: string; [k: string]: unknown };

async function onTabMessage(
  port: MessagePort,
  message: IncomingTabMessage,
  configResolve: () => void,
): Promise<void> {
  if (message.type === "config") {
    const cfg = message as ConfigMessage;
    if (cfg.dbWorkerUrl) _dbWorkerUrl = cfg.dbWorkerUrl;
    if (cfg.tabId) _tabPorts.set(cfg.tabId, port);
    configResolve();
    return;
  }
  // `create-db-worker` is the Chrome-workaround response; it's
  // handled inline by `requestWorkerFromTab`'s temporary listener,
  // not here.
  if (message.type === "create-db-worker") return;

  if (message.type === "fetch") {
    await handleFetchMessage(port, message as FetchMessage);
    return;
  }
}

async function handleFetchMessage(port: MessagePort, msg: FetchMessage): Promise<void> {
  const { id, method = "GET", url, headers = {}, body = null } = msg;
  try {
    const response = await dispatchRequest(method, url, headers, body);
    port.postMessage({
      id,
      type: "response",
      status: response.status,
      headers: response.headers,
      body: response.body,
    });
  } catch (e) {
    const err = e instanceof Error ? e : new Error(String(e));
    port.postMessage({
      id,
      type: "response",
      status: 500,
      headers: { "content-type": "text/html; charset=utf-8" },
      body: `<h1>500 Internal Server Error</h1><pre>${escapeHtml(err.stack ?? err.message)}</pre>`,
    });
  }
}

function handleTabDisconnect(port: MessagePort): void {
  _ports.delete(port);
  for (const [id, p] of _tabPorts) {
    if (p === port) {
      _tabPorts.delete(id);
      if (id === _dbWorkerHostId) {
        _dbWorkerHostId = null;
        respawnDbWorker().catch((e) => console.error("respawn failed:", e));
      }
      break;
    }
  }
}

// ── DB Worker spawn (with Chrome workaround) ──

async function spawnDbWorker(tabPort: MessagePort | null = null): Promise<void> {
  if (!_dbWorkerUrl) throw new Error("dbWorkerUrl not set — first tab must send config");

  // Try direct spawn first (works in Firefox).
  if (_canCreateWorker === null || _canCreateWorker === true) {
    try {
      _dbWorker = new Worker(_dbWorkerUrl, { type: "module" });
      _canCreateWorker = true;
      return;
    } catch {
      _canCreateWorker = false;
    }
  }

  // Chrome path: ask a connected tab to create the Worker for us.
  const hostTab = tabPort ?? firstPort();
  if (!hostTab) throw new Error("No connected tabs available to create DB Worker");
  _dbWorker = await requestWorkerFromTab(hostTab);
  for (const [id, p] of _tabPorts) {
    if (p === hostTab) { _dbWorkerHostId = id; break; }
  }
}

function firstPort(): MessagePort | null {
  for (const p of _ports) return p;
  return null;
}

/** Chrome workaround: SharedWorkerGlobalScope can't construct a
 *  Worker. Send a `create-db-worker` message + a fresh
 *  MessageChannel port to a tab; the tab creates the Worker, wires
 *  it to the channel, and confirms back. We use our end of the
 *  channel as a Worker proxy. */
function requestWorkerFromTab(tab: MessagePort): Promise<MessagePort> {
  return new Promise((resolve, reject) => {
    const channel = new MessageChannel();
    const handler = (event: MessageEvent) => {
      const data = event.data as { type: string; error?: string };
      if (data.type === "db-worker-created") {
        tab.removeEventListener("message", handler as EventListener);
        channel.port1.start();
        resolve(channel.port1);
      } else if (data.type === "db-worker-error") {
        tab.removeEventListener("message", handler as EventListener);
        reject(new Error(data.error ?? "tab-side Worker creation failed"));
      }
    };
    tab.addEventListener("message", handler as EventListener);
    tab.postMessage(
      { type: "create-db-worker", url: _dbWorkerUrl },
      [channel.port2],
    );
  });
}

async function respawnDbWorker(): Promise<void> {
  if (_ports.size === 0) {
    console.warn("No tabs available to respawn DB Worker");
    return;
  }
  console.log("DB Worker host disconnected, respawning…");
  // Reset the worker handle so spawnDbWorker picks a new path
  // (direct or tab-delegated). The DB file persists in OPFS, so
  // re-init reopens the same database — queries resume seamlessly.
  _dbWorker = null;
  await spawnDbWorker();
  await sendDbInit(_databaseName);
  installDb(_dbWorker!);
  console.log("DB Worker respawned");
}

/** Send `init` to the dedicated Worker, await `ready`. */
function sendDbInit(database: string): Promise<void> {
  return new Promise((resolve, reject) => {
    if (!_dbWorker) { reject(new Error("DB Worker not spawned")); return; }
    const handler = (event: Event) => {
      const data = (event as MessageEvent).data as { type: string; error?: string };
      if (data.type === "ready") {
        _dbWorker!.removeEventListener("message", handler);
        resolve();
      } else if (data.type === "error") {
        _dbWorker!.removeEventListener("message", handler);
        reject(new Error(data.error ?? "db init failed"));
      }
    };
    _dbWorker.addEventListener("message", handler);
    _dbWorker.postMessage({ type: "init", config: { database } });
  });
}

// ── Form data parsing (parallel to server-libsql.ts) ──

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

// ── Request dispatch ──

interface DispatchResponse {
  status: number;
  headers: Record<string, string>;
  body: string;
}

async function dispatchRequest(
  rawMethod: string,
  rawUrl: string,
  headers: Record<string, string>,
  rawBody: string | null,
): Promise<DispatchResponse> {
  const url = new URL(rawUrl);
  let method = rawMethod.toUpperCase();

  let params: Record<string, any> = {};
  if (method !== "GET" && method !== "HEAD" && rawBody) {
    const contentType = (headers["content-type"] ?? "").toLowerCase();
    if (contentType.includes("application/x-www-form-urlencoded")) {
      params = parseFormData(rawBody);
    } else if (contentType.includes("application/json")) {
      try { params = JSON.parse(rawBody); } catch { /* malformed body, ignore */ }
    }
    const override = String(params._method ?? "").toUpperCase();
    if (method === "POST" && (override === "DELETE" || override === "PATCH" || override === "PUT")) {
      method = override;
      delete params._method;
    }
  }

  const match = Router.match(method, url.pathname, _dispatchTable);
  if (!match) {
    return {
      status: 404,
      headers: { "content-type": "text/plain; charset=utf-8" },
      body: `Not Found: ${method} ${url.pathname}`,
    };
  }

  const ctrlClass = _controllerRegistry[match.controller];
  if (!ctrlClass) {
    return {
      status: 500,
      headers: { "content-type": "text/plain; charset=utf-8" },
      body: `No controller registered: ${match.controller}`,
    };
  }

  const merged: Record<string, any> = { ...match.path_params.to_h() };
  for (const [k, v] of url.searchParams) merged[k] = v;
  Object.assign(merged, params);

  ViewHelpers.reset_slots_bang();

  let response: ActionResponse;
  try {
    const controller = new ctrlClass();
    controller.params = new Parameters(merged);
    controller.session = _sessionStore;
    controller.flash = new HashWithIndifferentAccess(_flashStore);
    controller.request_method = method;
    controller.request_path = url.pathname;
    await controller.process_action(match.action);
    _flashStore = controller.flash ? controller.flash.to_h() : {};
    response = {
      body: controller.body,
      status: controller.status,
      location: controller.location,
    };
  } catch (err) {
    console.error("handler error:", err);
    return {
      status: 500,
      headers: { "content-type": "text/plain; charset=utf-8" },
      body: `Server error: ${(err as Error).message}`,
    };
  }

  if (response.location) {
    return {
      status: response.status ?? 303,
      headers: { location: response.location },
      body: "",
    };
  }

  let body: string;
  if (_layoutRenderer) {
    ViewHelpers.set_yield(response.body ?? "");
    body = _layoutRenderer(response.body ?? "");
  } else {
    body = renderLayout(response.body ?? "");
  }
  return {
    status: response.status ?? 200,
    headers: { "content-type": "text/html; charset=utf-8" },
    body,
  };
}

// ── Layout fallback ──

function renderLayout(body: string): string {
  return `<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8">
    <title>Roundhouse App</title>
    <meta name="viewport" content="width=device-width,initial-scale=1">
    <link rel="icon" href="data:,">
  </head>
  <body>
    <main>${body}</main>
  </body>
</html>
`;
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}
