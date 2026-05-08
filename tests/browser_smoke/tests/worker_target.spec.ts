// SharedWorker browser-target smoke. The bar is narrow on purpose:
// catch framework-runtime portability regressions (Node-only globals
// in transpiled output, missing browser polyfills, etc.) by
// dispatching a real request through the SharedWorker and asserting
// the response isn't a 5xx.
//
// Why probe the SharedWorker directly instead of letting Turbo do it:
// when the SharedWorker returns a 5xx, Turbo Drive falls back to a
// full-page navigation — which causes vite preview's SPA fallback to
// serve the same index.html, masking the underlying error. By
// connecting our own SharedWorker port and reading the response
// payload, we see the controller's actual error string and surface
// it in the failure message.

import { test, expect } from "@playwright/test";

interface SharedWorkerResponse {
  status: number;
  body: string;
  headers: Record<string, string>;
}

/** Open a fresh SharedWorker port (the SharedWorker itself is
 *  shared by `name: "juntos"`), wait for `ready`, send a `fetch`
 *  message, return the parsed response. */
async function probeSharedWorker(
  page: import("@playwright/test").Page,
  args: {
    method: string;
    path: string;
    body?: string | null;
    headers?: Record<string, string>;
  },
): Promise<SharedWorkerResponse> {
  return await page.evaluate(async ({ method, path, body, headers }) => {
    const meta = document.querySelector<HTMLMetaElement>(
      'meta[name="juntos-worker"]',
    );
    if (!meta?.content) throw new Error("no juntos-worker meta tag");

    const sw = new SharedWorker(meta.content, {
      type: "module",
      name: "juntos",
    });
    sw.port.start();

    // Identify ourselves so the SharedWorker's tabPorts map can find
    // us if it ever needs a host tab (Chrome workaround). Doesn't
    // hurt to send even if the SharedWorker is already ready.
    sw.port.postMessage({ type: "config", tabId: "smoke-probe" });

    return await new Promise<SharedWorkerResponse>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error("SharedWorker probe timeout")),
        15_000,
      );
      const id = crypto.randomUUID();
      let readySeen = false;
      const sendFetch = () => {
        sw.port.postMessage({
          id,
          type: "fetch",
          method,
          url: new URL(path, location.origin).href,
          headers: headers ?? {},
          body: body ?? null,
        });
      };

      sw.port.addEventListener("message", (event) => {
        const data = event.data as
          | { type: "ready" }
          | { type: "error"; error: string }
          | { id: string; type: "response"; status: number; body: string; headers: Record<string, string> };

        if (data.type === "ready") {
          readySeen = true;
          sendFetch();
          return;
        }
        if (data.type === "error") {
          clearTimeout(timer);
          reject(new Error(`SharedWorker init error: ${data.error}`));
          return;
        }
        if ("id" in data && data.type === "response" && data.id === id) {
          clearTimeout(timer);
          resolve({ status: data.status, body: data.body, headers: data.headers });
        }
      });

      // If the SharedWorker is already ready when we connect, it
      // sends `ready` immediately upon `start()`. If we miss that
      // edge (already started before our listener attached), fire
      // the fetch anyway after a short delay — the SharedWorker
      // accepts fetch messages once ready.
      setTimeout(() => {
        if (!readySeen) sendFetch();
      }, 500);
    });
  }, args);
}

test.describe("SharedWorker target — real-blog", () => {
  test("page loads + SharedWorker reaches ready (no JS errors)", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(`pageerror: ${err.message}`));
    page.on("requestfailed", (req) =>
      errors.push(`requestfailed: ${req.url()} — ${req.failure()?.errorText}`),
    );

    let bridgeStarted = false;
    page.on("console", (msg) => {
      if (msg.text().includes("[juntos] Worker client bridge started")) {
        bridgeStarted = true;
      }
    });

    await page.goto("/", { waitUntil: "domcontentloaded" });
    // The bridge logs after `bridge.waitForReady()` resolves —
    // SharedWorker has spawned the dedicated DB Worker, applied
    // schema, run seeds. 15s budget covers cold sqlite-wasm load.
    await expect.poll(() => bridgeStarted, { timeout: 15_000 }).toBe(true);

    expect(errors, "no page errors during initial load").toEqual([]);
  });

  test("GET /articles dispatches through SharedWorker without 5xx", async ({ page }) => {
    await page.goto("/", { waitUntil: "domcontentloaded" });
    // Wait for the bridge to finish init before probing — otherwise
    // the SharedWorker may not have ActiveRecord.adapter installed.
    await page.waitForFunction(
      () =>
        typeof (
          globalThis as { __juntosBridgeStarted?: boolean }
        ).__juntosBridgeStarted !== "undefined" ||
        document.getElementById("loading")?.style.display === "none",
      null,
      { timeout: 15_000 },
    );

    const response = await probeSharedWorker(page, {
      method: "GET",
      path: "/articles",
    });

    expect(
      response.status,
      `SharedWorker returned ${response.status}: ${response.body.slice(0, 300)}`,
    ).toBeLessThan(500);
    // Defensive: a 200 on /articles should be HTML.
    if (response.status === 200) {
      expect(response.headers["content-type"] ?? "").toContain("text/html");
    }
  });

  test("POST /articles round-trips form body + creates record", async ({ page }) => {
    // POST exercises a different surface from GET: form-encoded body
    // parsing in dispatchRequest, controller.create → Article.new
    // → ActiveRecord.adapter.insert (which round-trips MessagePort
    // to the dedicated DB Worker), validations, redirect_to with
    // flash. Many framework-runtime portability gaps surface here
    // that a GET wouldn't trip.
    //
    // article model requires title + body (>= 10 chars). Body
    // shape matches Rails permit `article: [:title, :body]`.

    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () => document.getElementById("loading")?.style.display === "none",
      null,
      { timeout: 15_000 },
    );

    const body =
      "article%5Btitle%5D=Smoke+test+article" +
      "&article%5Bbody%5D=This+article+was+posted+from+the+smoke+harness.";

    const response = await probeSharedWorker(page, {
      method: "POST",
      path: "/articles",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body,
    });

    expect(
      response.status,
      `SharedWorker returned ${response.status}: ${response.body.slice(0, 300)}`,
    ).toBeLessThan(500);

    // Happy path: redirect to the created article. 302/303 is a
    // strong signal that the controller's create flow succeeded
    // end-to-end (validations passed, insert succeeded, redirect
    // built). A 422 here would indicate validation failure — also
    // not a 5xx, but suggests our form-encoded body didn't reach
    // the controller correctly.
    expect(
      [302, 303],
      `expected redirect; got ${response.status} body=${response.body.slice(0, 200)}`,
    ).toContain(response.status);

    const location = response.headers["location"] ?? response.headers["Location"];
    expect(location, "redirect should set Location header").toBeTruthy();
    expect(location, `Location should point at /articles/<id>; got ${location}`).toMatch(
      /^\/articles\/\d+$/,
    );
  });
});
