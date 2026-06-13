// Roundhouse TypeScript main-thread runtime — SharedWorker client bridge.
//
// Loaded into the browser tab from the emitted `main.ts` entry. Runs
// strictly on the main thread — never imported by `server-worker.ts`
// or `db_worker.ts`. Its job is narrow:
//
//   1. Spawn the SharedWorker (URL read from `<meta
//      name="juntos-worker">` injected at build time by the Vite
//      manifest plugin).
//   2. Send the SharedWorker its initial config — most importantly
//      the dedicated DB Worker URL (also a fingerprinted manifest
//      lookup, in `<meta name="juntos-db-worker">`).
//   3. Intercept Turbo's `turbo:before-fetch-request` events,
//      serialize them over MessagePort, await the SharedWorker's
//      Response, and feed Turbo a synthetic `Response` so the
//      navigation/form submit completes as if it had hit a real
//      HTTP server.
//   4. Run the Chrome workaround: when the SharedWorker can't
//      construct a `Worker` directly (Chrome doesn't expose the
//      constructor inside `SharedWorkerGlobalScope`), it sends us a
//      `create-db-worker` request + a fresh MessageChannel port; we
//      create the Worker on its behalf and wire the channel.
//   5. Subscribe to BroadcastChannel-backed Turbo Streams via the
//      `juntos-stream-source` custom element — when the
//      SharedWorker renders a view containing `turbo_stream_from`,
//      it returns a `<juntos-stream-source channel="...">` element
//      and `connectedCallback` subscribes here.
//
// No view helpers, no Router, no controller dispatch — those all
// live in the SharedWorker. This file is the *transport* between
// Turbo (in the tab) and the SharedWorker (cross-tab application).

// ── Types ──

interface ResponsePayload {
  id: string;
  type: "response";
  status: number;
  headers: Record<string, string>;
  body: string;
  binary?: boolean;
}

interface ReadyPayload {
  type: "ready";
}

interface ErrorPayload {
  type: "error";
  error: string;
}

interface CreateDbWorkerPayload {
  type: "create-db-worker";
  url: string;
}

type IncomingMessage = ResponsePayload | ReadyPayload | ErrorPayload | CreateDbWorkerPayload;

interface PendingResolver {
  resolve: (data: ResponsePayload) => void;
}

// ── WorkerBridge: MessagePort fetch correlation ──

class WorkerBridge {
  private port: MessagePort;
  private pending = new Map<string, PendingResolver>();
  private _ready: Promise<void>;
  private _readyResolve!: () => void;
  private _readyReject!: (e: Error) => void;

  constructor(worker: SharedWorker) {
    this.port = worker.port;

    this._ready = new Promise((resolve, reject) => {
      this._readyResolve = resolve;
      this._readyReject = reject;
    });

    worker.onerror = (e: ErrorEvent | Event) => {
      console.error("[juntos] SharedWorker error:", e);
      this._readyReject(new Error("SharedWorker failed to load"));
    };

    this.port.onmessage = (event: MessageEvent) => {
      const data = event.data as IncomingMessage;
      if (data.type === "ready") {
        this._readyResolve();
        return;
      }
      if (data.type === "error") {
        console.error("[juntos] SharedWorker initialization error:", data.error);
        this._readyReject(new Error(data.error));
        return;
      }
      if (data.type === "create-db-worker") {
        this.handleCreateDbWorker(data.url, event.ports[0]);
        return;
      }
      if (data.type === "response" && data.id) {
        const resolver = this.pending.get(data.id);
        if (resolver) {
          this.pending.delete(data.id);
          resolver.resolve(data);
        }
      }
    };

    this.port.start();
  }

  /** Resolves once the SharedWorker has finished initialization
   *  (DB Worker spawned, schema applied, seeds run). */
  waitForReady(): Promise<void> {
    return this._ready;
  }

  /** Send a fetch-shaped request to the SharedWorker, await the
   *  serialized Response. */
  fetch(
    method: string,
    url: string,
    headers: Record<string, string>,
    body: string | null,
  ): Promise<ResponsePayload> {
    return new Promise((resolve) => {
      const id = crypto.randomUUID();
      this.pending.set(id, { resolve });
      this.port.postMessage({ id, type: "fetch", method, url, headers, body });
    });
  }

  /** Chrome workaround: SharedWorker asks us (the main thread) to
   *  create the dedicated DB Worker on its behalf and wire it to a
   *  MessageChannel port we received in the same message. */
  private handleCreateDbWorker(url: string, workerPort: MessagePort | undefined): void {
    if (!workerPort) {
      this.port.postMessage({ type: "db-worker-error", error: "no MessagePort received" });
      return;
    }
    try {
      const dbWorker = new Worker(url, { type: "module" });
      dbWorker.onmessage = (e: MessageEvent) => workerPort.postMessage(e.data);
      workerPort.onmessage = (e: MessageEvent) => dbWorker.postMessage(e.data);
      workerPort.start();
      this.port.postMessage({ type: "db-worker-created" });
    } catch (e) {
      const error = e instanceof Error ? e.message : String(e);
      this.port.postMessage({ type: "db-worker-error", error });
    }
  }
}

// ── Turbo Streams via BroadcastChannel ──
//
// SharedWorker-rendered views emit `<juntos-stream-source channel="X">`
// when they call `turbo_stream_from(X)`. Inserting that element into
// the DOM triggers `connectedCallback`, which subscribes to channel
// X on this tab — incoming `<turbo-stream>` fragments are handed to
// `Turbo.renderStreamMessage`.

declare const Turbo: {
  renderStreamMessage?: (html: string) => void;
  visit?: (location: string, options?: { action?: "advance" | "replace" | "restore" }) => void;
};

class TurboBroadcast {
  private static channels = new Map<string, BroadcastChannel>();

  private static getChannel(name: string): BroadcastChannel {
    let ch = this.channels.get(name);
    if (!ch) {
      ch = new BroadcastChannel(name);
      this.channels.set(name, ch);
    }
    return ch;
  }

  static subscribe(channelName: string): void {
    const channel = this.getChannel(channelName);
    channel.onmessage = (event: MessageEvent) => {
      const html = String(event.data);
      if (html.startsWith("<turbo-stream") && typeof Turbo !== "undefined" && Turbo.renderStreamMessage) {
        Turbo.renderStreamMessage(html);
      }
    };
  }

  static unsubscribe(channelName: string): void {
    const channel = this.channels.get(channelName);
    if (channel) {
      channel.close();
      this.channels.delete(channelName);
    }
  }
}

function defineStreamSourceElement(): void {
  if (typeof customElements === "undefined") return;
  if (customElements.get("juntos-stream-source")) return;
  customElements.define("juntos-stream-source", class extends HTMLElement {
    connectedCallback(): void {
      const channel = this.getAttribute("channel");
      if (channel) TurboBroadcast.subscribe(channel);
    }
    disconnectedCallback(): void {
      const channel = this.getAttribute("channel");
      if (channel) TurboBroadcast.unsubscribe(channel);
    }
  });
}

// `turbo_stream_from(stream)` emits the Rails Action-Cable element
// `<turbo-cable-stream-source signed-stream-name="…">`. On the server
// targets, turbo-rails' JS upgrades that into a WebSocket cable
// subscription. The worker target has no cable — it broadcasts over
// BroadcastChannel — and without turbo-rails the element is an inert
// unknown tag, so nothing subscribes (broadcasts silently never reach
// the DOM). Rewrite each into the lifecycle-managed
// `<juntos-stream-source channel="…">` before the HTML reaches the DOM,
// so its connected/disconnected callbacks own subscribe/unsubscribe.
// The channel is the base64(JSON) `signed-stream-name` minus the
// `--unsigned` suffix Rails appends — and equals the `broadcast(stream,
// …)` name the worker posts to (e.g. "articles").
function decodeStreamName(signed: string): string | null {
  const b64 = signed.replace(/--unsigned$/, "");
  try {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    return String(JSON.parse(new TextDecoder().decode(bytes)));
  } catch {
    return null;
  }
}

function rewriteStreamSources(html: string): string {
  return html.replace(
    /<turbo-cable-stream-source\b[^>]*?\bsigned-stream-name="([^"]*)"[^>]*><\/turbo-cable-stream-source>/g,
    (_match, signed: string) => {
      const channel = decodeStreamName(signed);
      return channel
        ? `<juntos-stream-source channel="${escapeHtml(channel)}"></juntos-stream-source>`
        : "";
    },
  );
}

// ── Public entry: startClient() ──

export interface StartClientOptions {
  /** Override the meta-tag URLs (mostly for tests). */
  workerUrl?: string;
  dbWorkerUrl?: string;
}

/** Stable global slot for the SharedWorker URLs + readiness flag.
 *  After the initial render swaps the document head, the
 *  `<meta name="juntos-worker">` tags from index.html are gone —
 *  they're not in the application layout's head. Tests + any
 *  late code that needs to spawn a fresh SharedWorker port read
 *  from here instead of the DOM. The values are captured before
 *  any document mutation. */
interface JuntosGlobal {
  workerUrl: string;
  dbWorkerUrl: string;
  ready: boolean;
}

declare global {
  interface Window {
    __juntos__?: JuntosGlobal;
  }
}

let _bridge: WorkerBridge | null = null;

export async function startClient(opts: StartClientOptions = {}): Promise<void> {
  if (!globalThis.SharedWorker) {
    const msg =
      "This app requires SharedWorker support (Chrome 80+, Firefox 114+, Safari 18.2+). " +
      "Please use a supported browser, or rebuild without -t worker.";
    console.error("[juntos]", msg);
    const el = document.getElementById("loading") ?? document.body;
    el.innerHTML = `<p style="color: red; padding: 20px;">${escapeHtml(msg)}</p>`;
    return;
  }

  const workerUrl =
    opts.workerUrl ??
    document.querySelector<HTMLMetaElement>('meta[name="juntos-worker"]')?.content ??
    "/worker.js";
  const dbWorkerUrl =
    opts.dbWorkerUrl ??
    document.querySelector<HTMLMetaElement>('meta[name="juntos-db-worker"]')?.content ??
    "/db_worker.js";

  // Capture URLs before renderInitial swaps the head — once the
  // layout's head replaces the index shell, the meta tags are gone.
  window.__juntos__ = { workerUrl, dbWorkerUrl, ready: false };

  const tabId = crypto.randomUUID();

  let worker: SharedWorker;
  try {
    worker = new SharedWorker(workerUrl, { type: "module", name: "juntos" });
  } catch (e) {
    showError(e);
    return;
  }

  worker.port.postMessage({ type: "config", dbWorkerUrl, tabId });

  // Announce close so the SharedWorker can release our port and, if
  // we're hosting the dedicated DB Worker (Chrome workaround), pick
  // a different tab to host the respawn.
  const lifecycle = new BroadcastChannel("juntos:lifecycle");
  window.addEventListener("beforeunload", () => {
    lifecycle.postMessage({ type: "tab-closing", tabId });
  });

  defineStreamSourceElement();

  _bridge = new WorkerBridge(worker);

  try {
    await _bridge.waitForReady();
  } catch (e) {
    showError(e);
    return;
  }

  installTurboIntercept(_bridge);

  const loadingEl = document.getElementById("loading");
  const appEl = document.getElementById("app");
  if (loadingEl) loadingEl.style.display = "none";
  if (appEl) appEl.style.display = "block";

  console.log("[juntos] Worker client bridge started");

  // Non-Turbo initial render. The index.html shell only has
  // #loading + empty #app — the actual route content lives in the
  // SharedWorker and only reaches the DOM after a fetch dispatches.
  //
  // Why not Turbo.visit here: the shell's minimal head (viewport +
  // a few <meta> tags) and the layout's full head (importmap +
  // Stimulus + Turbo module scripts + per-app meta tags) are
  // structurally incompatible. Turbo Drive's head merge ends up
  // re-running module scripts (or under `turbo-refresh-method=morph`,
  // morphing in elements that fight with what main.ts already set
  // up) and either loses state or — worse — triggers a full page
  // reload that re-runs main.ts and re-fires the auto-visit, an
  // infinite loop.
  //
  // First render bypasses Turbo: bridge.fetch directly, parse the
  // layout-wrapped response, swap document.documentElement so the
  // head + body both come from the layout. After this paint, head
  // is a real layout head; subsequent Turbo intercept-driven
  // navigations swap body without head conflict because both sides
  // of the merge are now layout-shaped.
  await renderInitial(_bridge);

  if (window.__juntos__) window.__juntos__.ready = true;
}

// Deploy base path, injected by Vite (`import.meta.env.BASE_URL`):
// "/" when served from the web root, or e.g. "/roundhouse/blog/" when
// mounted under a subdirectory (project GitHub Pages). Application
// routes are root-relative ("/articles"), so the worker's router
// expects app paths with the base stripped off; `toAppPath` strips it
// from the browser location, `toBrowserPath` re-adds it when writing
// history/URL so the address bar stays inside the mount. Accessed
// defensively so the file type-checks without vite/client ambient
// types in the emitted project.
const BASE: string =
  (import.meta as unknown as { env?: { BASE_URL?: string } }).env?.BASE_URL ?? "/";

function toAppPath(browserPath: string): string {
  if (BASE !== "/" && browserPath.startsWith(BASE)) {
    return "/" + browserPath.slice(BASE.length);
  }
  return browserPath || "/";
}

function toBrowserPath(appPath: string): string {
  if (BASE !== "/" && appPath.startsWith("/")) {
    return BASE.replace(/\/$/, "") + appPath;
  }
  return appPath;
}

// The emitted app's links/forms are root-absolute app paths
// ("/articles"). Under a subdirectory mount, Turbo would push those
// straight to history and the address bar would escape the mount
// (and a later reload 404). Rewrite same-origin root-absolute
// `href`/`action` targets to mount-prefixed paths so Turbo's own
// bookkeeping stays inside the mount; the intercept below strips the
// prefix back off before routing into the worker. No-op at base "/".
function rebaseLinks(root: ParentNode): void {
  if (BASE === "/") return;
  const rebaseAttr = (el: Element, attr: string) => {
    const v = el.getAttribute(attr);
    // root-absolute, same-origin, not protocol-relative, not already mounted
    if (v && v.startsWith("/") && !v.startsWith("//") && !v.startsWith(BASE)) {
      el.setAttribute(attr, toBrowserPath(v));
    }
  };
  for (const a of Array.from(root.querySelectorAll("a[href]"))) rebaseAttr(a, "href");
  for (const f of Array.from(root.querySelectorAll("form[action]"))) rebaseAttr(f, "action");
}

async function renderInitial(bridge: WorkerBridge): Promise<void> {
  const initialPath = toAppPath(location.pathname || "/");
  let response;
  try {
    response = await bridge.fetch("GET", new URL(initialPath, location.origin).href, {
      cookie: document.cookie,
      accept: "text/html",
    }, null);
  } catch (e) {
    console.error("[juntos] initial render fetch failed:", e);
    return;
  }

  // Follow a single redirect (e.g. root→/articles); chained
  // redirects fall through to a manual click on the final
  // location string in the body, since runaway redirect chains
  // are very likely a bug worth surfacing rather than papering
  // over.
  if (
    (response.status === 301 || response.status === 302 || response.status === 303) &&
    response.headers.location
  ) {
    const redirectTo = response.headers.location;
    history.replaceState({}, "", toBrowserPath(redirectTo));
    try {
      response = await bridge.fetch("GET", new URL(redirectTo, location.origin).href, {
        cookie: document.cookie,
        accept: "text/html",
      }, null);
    } catch (e) {
      console.error("[juntos] initial-render redirect fetch failed:", e);
      return;
    }
  }

  if (response.status >= 400) {
    document.body.innerHTML = `<pre style="padding: 20px; color: #b00;">${escapeHtml(
      `Initial render returned ${response.status}\n\n${response.body.slice(0, 2000)}`,
    )}</pre>`;
    return;
  }

  const contentType = response.headers["content-type"] ?? response.headers["Content-Type"] ?? "";
  if (!contentType.includes("text/html")) {
    console.warn("[juntos] initial render returned non-HTML content-type:", contentType);
    return;
  }

  // Parse the layout-wrapped response. We need a full document
  // parse (not just innerHTML) because the response includes
  // <html>/<head>/<body> from the application layout.
  const parser = new DOMParser();
  const doc = parser.parseFromString(rewriteStreamSources(response.body), "text/html");

  // Swap the live <html>'s child structure. Replacing the entire
  // documentElement is heavy-handed and breaks document.body
  // listeners; replacing head + body individually is the
  // conservative path. The script tags in the new body re-execute
  // because the browser parses HTML strings into nodes; innerHTML
  // assignment doesn't re-run scripts, so we adopt nodes manually
  // for any <script> we want active (Stimulus controllers, Turbo
  // bootstrapping if not already loaded).
  reconcileHead(doc.head);
  document.body.replaceChildren(...Array.from(doc.body.childNodes));
  reActivateScripts(document.body);
  rebaseLinks(document.body);

  // Update title (DOMParser preserves <title>, but innerHTML swap
  // sometimes leaves the old title on the document object).
  const newTitle = doc.querySelector("title")?.textContent;
  if (newTitle) document.title = newTitle;
}

/** Merge the worker-rendered layout's `<head>` into the live head
 *  without disturbing Vite-managed assets.
 *
 *  The index.html shell's head carries Vite's fingerprinted, base-
 *  prefixed bundle links — the `<link rel="stylesheet">` for the
 *  compiled Tailwind CSS and the entry `<script type="module">`. Those
 *  are SPA infrastructure: identical across pages and already executing,
 *  so we keep the live nodes in place (re-adding the module would re-run
 *  it). From the layout head we take title + meta + other non-asset
 *  nodes, and DROP its Rails asset-pipeline tags — the importmap, module
 *  preloads, the bare `import "application"` bootstrap, and the
 *  `stylesheet_link_tag` `/assets/*.css` links — because under the
 *  bundling model (issue #6) Vite owns assets and those files don't
 *  exist in the build. Dropping the importmap is also what lets us stop
 *  replacing the head wholesale (the original reason for the full swap). */
function reconcileHead(layoutHead: HTMLHeadElement): void {
  const keep = Array.from(document.head.children).filter(
    (el) =>
      (el.tagName === "LINK" && el.getAttribute("rel") === "stylesheet") ||
      (el.tagName === "SCRIPT" &&
        el.getAttribute("type") === "module" &&
        el.hasAttribute("src")),
  );
  const incoming = Array.from(layoutHead.childNodes).filter((node) => {
    if (node.nodeType !== 1) return true; // text / comments
    const el = node as Element;
    const rel = el.getAttribute("rel");
    const type = el.getAttribute("type");
    if (el.tagName === "SCRIPT" && type === "importmap") return false;
    if (el.tagName === "LINK" && rel === "modulepreload") return false;
    if (el.tagName === "LINK" && rel === "stylesheet") return false; // Vite owns CSS
    if (el.tagName === "SCRIPT" && type === "module" && !el.hasAttribute("src")) {
      return false; // bare `import "application"` importmap bootstrap
    }
    return true;
  });
  document.head.replaceChildren(...keep, ...incoming);
}

/** `innerHTML`/`replaceChildren` with parsed `<script>` nodes
 *  doesn't actually execute them — the browser flags scripts
 *  inserted that way as already-executed. Clone each script into a
 *  fresh element to make it run. */
function reActivateScripts(root: ParentNode): void {
  for (const oldScript of Array.from(root.querySelectorAll("script"))) {
    const newScript = document.createElement("script");
    for (const { name, value } of Array.from(oldScript.attributes)) {
      newScript.setAttribute(name, value);
    }
    if (oldScript.textContent) newScript.textContent = oldScript.textContent;
    oldScript.replaceWith(newScript);
  }
}

// ── Turbo intercept ──

function installTurboIntercept(bridge: WorkerBridge): void {
  // Re-apply the mount prefix to links/forms Turbo renders on each
  // navigation (the worker returns app-path HTML). No-op at base "/".
  document.addEventListener("turbo:render", () => rebaseLinks(document.body));

  document.addEventListener("turbo:before-fetch-request", async (event: Event) => {
    const detail = (event as CustomEvent<{
      url: string;
      fetchOptions: {
        method?: string;
        body?: BodyInit | null;
        headers?: Record<string, string>;
      };
      fetchRequest?: { response: Promise<Response> };
      resume: () => void;
    }>).detail;

    const fetchOptions = detail.fetchOptions;
    const method = (fetchOptions.method ?? "GET").toUpperCase();
    const url = new URL(detail.url);

    // Same-origin only — let the browser handle cross-origin requests.
    if (url.origin !== location.origin) return;

    event.preventDefault();

    const bodyString = await serializeBody(fetchOptions.body);

    const headers: Record<string, string> = {
      cookie: document.cookie,
      accept: pickHeader(fetchOptions.headers, "accept") ?? "text/html",
      "content-type":
        pickHeader(fetchOptions.headers, "content-type") ??
        "application/x-www-form-urlencoded",
    };

    // Route the app path (mount prefix stripped) into the worker; the
    // rendered links were rebased to the mount, so url.pathname carries
    // the prefix under a subdirectory deploy.
    const appUrl = new URL(toAppPath(url.pathname) + url.search, location.origin).href;
    const response = await bridge.fetch(method, appUrl, headers, bodyString);

    // Apply Set-Cookie from response (flash, session).
    const setCookie = response.headers["set-cookie"];
    if (setCookie) document.cookie = setCookie;

    // 301/302 redirects: hand to Turbo.visit so it animates. The
    // worker's Location is an app path; re-add the mount so the URL bar
    // stays inside the mount (and the follow-up intercept strips it).
    if ((response.status === 301 || response.status === 302) && typeof Turbo !== "undefined" && Turbo.visit) {
      const loc = response.headers.location ?? response.headers.Location;
      if (loc) {
        Turbo.visit(toBrowserPath(loc));
        return;
      }
    }

    detail.fetchRequest = {
      response: Promise.resolve(
        new Response(rewriteStreamSources(response.body), {
          status: response.status,
          headers: response.headers,
        }),
      ),
    };
    detail.resume();
  });
}

// ── Helpers ──

async function serializeBody(body: BodyInit | null | undefined): Promise<string | null> {
  if (body == null) return null;
  if (typeof body === "string") return body;
  if (body instanceof URLSearchParams) return body.toString();
  if (body instanceof FormData) {
    // Convert File entries to data URIs so they survive postMessage
    // (Files don't structured-clone through URLSearchParams).
    for (const [key, value] of body.entries()) {
      if (value instanceof File && value.size > 0) {
        const dataURI = await fileToDataURI(value);
        body.set(key, `datauri:${value.name}:${dataURI}`);
      }
    }
    return new URLSearchParams(body as unknown as Record<string, string>).toString();
  }
  // Blob / ArrayBuffer / etc. — best effort: stringify via Response.
  try {
    return await new Response(body).text();
  } catch {
    return null;
  }
}

function fileToDataURI(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error ?? new Error("FileReader error"));
    reader.readAsDataURL(file);
  });
}

function pickHeader(
  headers: Record<string, string> | undefined,
  name: string,
): string | undefined {
  if (!headers) return undefined;
  const lower = name.toLowerCase();
  for (const [k, v] of Object.entries(headers)) {
    if (k.toLowerCase() === lower) return v;
  }
  return undefined;
}

function showError(e: unknown): void {
  const err = e instanceof Error ? e : new Error(String(e));
  const el = document.getElementById("loading") ?? document.body;
  el.innerHTML = `<p style="color: red; padding: 20px;">Error: ${escapeHtml(err.message)}</p>` +
    `<pre>${escapeHtml(err.stack ?? "")}</pre>`;
  console.error(err);
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}
