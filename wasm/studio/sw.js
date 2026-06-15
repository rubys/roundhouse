// Studio app-host service worker (rung D, Phase 5). Serves the in-browser-built
// app — its index shell + the esbuild bundles (main/worker/db_worker.js) — from
// an in-memory map at this SW's scope, so the emitted SharedWorker app runs from
// real, same-origin URLs: module loads, `new SharedWorker(...)`/`new Worker(...)`
// script fetches, and the app's own relative routes all resolve through here. No
// network server, no container. studio.js posts a fresh file map on every
// rebuild; the iframe mounted at the scope loads it.
//
// Scope is narrower than this script's location (registered with {scope}), so it
// only intercepts the app subtree (e.g. /studio/app/…), never the studio UI.

const SCOPE = new URL(self.registration.scope).pathname; // e.g. /studio/app/
let FILES = {}; // appPath (no leading slash) -> { body, type }

const MIME = {
  html: "text/html; charset=utf-8",
  js: "text/javascript; charset=utf-8",
  mjs: "text/javascript; charset=utf-8",
  css: "text/css; charset=utf-8",
  json: "application/json; charset=utf-8",
  map: "application/json; charset=utf-8",
  wasm: "application/wasm",
};
function mimeFor(path) {
  return MIME[path.split(".").pop()] || "text/plain; charset=utf-8";
}

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));

self.addEventListener("message", (e) => {
  const msg = e.data;
  if (msg && msg.type === "files") {
    FILES = msg.files || {};
    const reply = { type: "files-ack", count: Object.keys(FILES).length };
    if (e.ports && e.ports[0]) e.ports[0].postMessage(reply);
    else if (e.source) e.source.postMessage(reply);
  }
});

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  // Only serve same-origin requests inside our scope; everything else
  // (cross-origin CDN deps, the studio UI) passes through to the network.
  if (url.origin !== self.location.origin || !url.pathname.startsWith(SCOPE)) return;

  let rel = url.pathname.slice(SCOPE.length);
  if (rel === "" || rel.endsWith("/")) rel = "index.html"; // navigations → shell

  const file = FILES[rel];
  if (file) {
    return event.respondWith(
      new Response(file.body, { headers: { "content-type": file.type || mimeFor(rel) } }),
    );
  }
  // SPA fallback: an unknown extensionless path is a client-side route → shell.
  if (!rel.includes(".") && FILES["index.html"]) {
    return event.respondWith(
      new Response(FILES["index.html"].body, { headers: { "content-type": MIME.html } }),
    );
  }
  event.respondWith(new Response(`studio: not found: ${rel}`, { status: 404 }));
});
