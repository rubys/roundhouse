// Drives the running app in an iframe for /studio/ (rung D, Phase 5). Registers
// the studio app-host service worker (sw.js), pushes the freshly-built file map
// to it, and (re)loads the iframe. Full-reload loop: on each edit the bundles
// are re-pushed and the iframe reloads — the SW + the app's OPFS DB persist
// across reloads, so state survives a recompile.

export async function createAppHost(iframe, { swUrl, scope }) {
  if (!("serviceWorker" in navigator)) throw new Error("Service Workers unavailable");
  // NOTE: register with a scope NARROWER than the studio page (the app subtree),
  // so this SW controls only the iframe — never the studio UI. Don't await
  // navigator.serviceWorker.ready: that waits for a worker controlling THIS
  // page, which (by design) is out of scope and never happens. Wait for the
  // registration's own worker to activate instead.
  const reg = await navigator.serviceWorker.register(swUrl, { scope });
  const sw = reg.active || (await waitForActive(reg));

  let mounted = false;
  return {
    scope,
    /** Replace the served file map, then (first call) mount the iframe or
     *  (subsequent) reload it so the new bundles take effect. */
    async update(files) {
      await postFiles(sw, files);
      if (!mounted) {
        iframe.src = scope;
        mounted = true;
      } else {
        // Re-navigate (rather than location.reload) so a cross-origin-isolation
        // or detached-doc state can't wedge the reload.
        iframe.contentWindow?.location.replace(scope);
      }
    },
  };
}

function waitForActive(reg) {
  return new Promise((resolve) => {
    if (reg.active) return resolve(reg.active);
    const w = reg.installing || reg.waiting;
    w?.addEventListener("statechange", () => reg.active && resolve(reg.active));
    navigator.serviceWorker.addEventListener("controllerchange", () => reg.active && resolve(reg.active));
  });
}

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
