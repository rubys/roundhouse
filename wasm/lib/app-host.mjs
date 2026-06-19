// Drives the running app in an iframe for /studio/ (rung D, Phase 5). Registers
// the studio app-host service worker (sw.js), pushes the freshly-built file map
// to it, and (re)loads the iframe. Full-reload loop: on each edit the bundles
// are re-pushed and the iframe reloads — the SW + the app's OPFS DB persist
// across reloads, so state survives a recompile.

export async function createAppHost(iframe, { swUrl, scope }) {
  if (!("serviceWorker" in navigator)) throw new Error("Service Workers unavailable");
  // Register with a scope NARROWER than the studio page (the app subtree), so
  // this SW controls only the iframe — never the studio UI. Don't await
  // navigator.serviceWorker.ready: that waits for a worker controlling THIS
  // page, which (by design) is out of scope and never happens.
  const reg = await navigator.serviceWorker.register(swUrl, { scope });
  // Wait for the worker to be fully ACTIVATED (not merely `reg.active` set,
  // which can happen while still "activating"). A brand-new iframe navigation
  // racing activation falls through to the network → 404 on the app scope; this
  // + the mount retry below close that cold-start race.
  const sw = await waitForActivated(reg);

  let mounted = false;
  return {
    scope,
    async update(files) {
      await postFiles(sw, files);
      if (!mounted) {
        await mountWithRetry(iframe, scope);
        mounted = true;
      } else {
        iframe.contentWindow?.location.replace(scope);
      }
    },
    // Hot-swap (no iframe reload): push the new bundles to the SW, then ask the
    // RUNNING app to respawn just its SharedWorker and Turbo-morph in place
    // (client.ts `reconnect`). Resolves true if the app acked the swap, false if
    // it timed out or isn't mounted yet — the caller falls back to update().
    async hotSwap(files, v) {
      if (!mounted) return false;
      await postFiles(sw, files);
      return await requestSwap(iframe, v);
    },
  };
}

// postMessage the running app a swap request + a reply port; resolve true on its
// ack, false on timeout (→ caller reloads instead).
function requestSwap(iframe, v) {
  return new Promise((resolve) => {
    const win = iframe.contentWindow;
    if (!win) return resolve(false);
    const ch = new MessageChannel();
    let done = false;
    const finish = (ok) => { if (!done) { done = true; resolve(ok); } };
    ch.port1.onmessage = (e) => { if (e.data?.type === "rh-swap-ack") finish(e.data.ok !== false); };
    win.postMessage({ type: "rh-hot-swap", v }, location.origin, [ch.port2]);
    setTimeout(() => finish(false), 8000);
  });
}

function waitForActivated(reg) {
  return new Promise((resolve) => {
    const ready = () => reg.active && reg.active.state === "activated" ? reg.active : null;
    const r = ready();
    if (r) return resolve(r);
    const onChange = () => { const a = ready(); if (a) resolve(a); };
    (reg.installing || reg.waiting || reg.active)?.addEventListener("statechange", onChange);
    navigator.serviceWorker.addEventListener("controllerchange", onChange);
    const t = setInterval(() => { const a = ready(); if (a) { clearInterval(t); resolve(a); } }, 50);
  });
}

// Mount the iframe, and if the first navigation slipped past the SW (cold start:
// served a network 404 instead of our shell), reload it — the SW is controlling
// by then. Detect by the shell's worker <meta>, absent from a Pages 404 page.
async function mountWithRetry(iframe, scope, tries = 5) {
  for (let i = 0; i < tries; i++) {
    await navigateIframe(iframe, scope, i === 0);
    if (iframeServedShell(iframe)) return;
    await delay(120 * (i + 1));
  }
}

function navigateIframe(iframe, scope, first) {
  return new Promise((resolve) => {
    const onload = () => { iframe.removeEventListener("load", onload); resolve(); };
    iframe.addEventListener("load", onload);
    if (first) iframe.src = scope;
    else iframe.contentWindow?.location.replace(scope);
    setTimeout(resolve, 4000); // don't hang if load never fires
  });
}

function iframeServedShell(iframe) {
  try {
    return !!iframe.contentDocument?.querySelector('meta[name="juntos-worker"]');
  } catch {
    return false;
  }
}

const delay = (ms) => new Promise((r) => setTimeout(r, ms));

// Push the file map and await the SW's ack (so the iframe never loads before the
// SW holds the files). Falls back to a short timeout if the ack path is missed.
function postFiles(sw, files) {
  return new Promise((resolve) => {
    const ch = new MessageChannel();
    let done = false;
    const finish = () => { if (!done) { done = true; resolve(); } };
    ch.port1.onmessage = (e) => { if (e.data?.type === "files-ack") finish(); };
    sw.postMessage({ type: "files", files }, [ch.port2]);
    setTimeout(finish, 500);
  });
}
