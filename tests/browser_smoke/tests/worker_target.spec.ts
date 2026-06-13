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
    // After the initial render swaps the head, the `<meta
    // name="juntos-worker">` is gone (the layout's head replaces
    // the index.html shell's head). client.ts publishes the URLs
    // to `window.__juntos__` before any swap; fall back to the
    // meta tag for the brief window before the initial render
    // resolves.
    const stash = (window as { __juntos__?: { workerUrl: string } }).__juntos__;
    const url =
      stash?.workerUrl ??
      document.querySelector<HTMLMetaElement>('meta[name="juntos-worker"]')?.content;
    if (!url) throw new Error("no juntos-worker URL (window.__juntos__ unset, meta tag missing)");

    const sw = new SharedWorker(url, {
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
    // requestfailed for asset paths is filtered: the application
    // layout's importmap references Rails-style asset paths
    // (`/assets/turbo.min.js`, `/assets/controllers/*.js`) that
    // don't exist in the Vite build — those 404s are cosmetic
    // (Stimulus/Turbo come in via the Vite-bundled main.ts, not
    // the importmap), and a separate ticket from bridge readiness.
    page.on("requestfailed", (req) => {
      const url = req.url();
      if (url.includes("/assets/") && (url.endsWith(".js") || url.endsWith(".mjs"))) {
        return; // missing-importmap-target — not a JS error
      }
      errors.push(`requestfailed: ${url} — ${req.failure()?.errorText}`);
    });

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
    // `window.__juntos__.ready` is set by client.ts after the
    // initial render resolves.
    await page.waitForFunction(
      () => (window as Window & { __juntos__?: { ready?: boolean } }).__juntos__?.ready === true,
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

  test("multi-tab BroadcastChannel: POST in tab A reaches tab B", async ({ browser }) => {
    // The SharedWorker is shared across all tabs of the same origin
    // (`name: "juntos"`). Article#broadcasts_to ->(_a) { "articles" }
    // fires on insert; the SharedWorker's framework runtime calls
    // `broadcast("articles", html)` which goes through
    // juntos-worker.ts's default BroadcastChannel-backed broadcaster.
    // Any tab subscribed to BroadcastChannel("articles") receives
    // the turbo-stream HTML.
    //
    // This probe exercises the multi-tab story end-to-end without
    // touching Turbo: tab B subscribes raw to BroadcastChannel,
    // tab A POSTs to create, tab B asserts the broadcast arrived.
    // No turbo-stream rendering, no DOM assertions — just "did the
    // message cross from SharedWorker to a different tab?"

    const context = await browser.newContext();
    const tabA = await context.newPage();
    const tabB = await context.newPage();

    await Promise.all([
      tabA.goto("/", { waitUntil: "domcontentloaded" }),
      tabB.goto("/", { waitUntil: "domcontentloaded" }),
    ]);

    // Both bridges must reach ready before tab B can subscribe and
    // tab A can POST through the dispatcher.
    for (const tab of [tabA, tabB]) {
      await tab.waitForFunction(
        () => (window as Window & { __juntos__?: { ready?: boolean } }).__juntos__?.ready === true,
        null,
        { timeout: 15_000 },
      );
    }

    // Tab B installs a BroadcastChannel listener BEFORE the POST so
    // there's no race between the broadcast firing and the listener
    // attaching.
    await tabB.evaluate(() => {
      (globalThis as { __received?: string[] }).__received = [];
      const ch = new BroadcastChannel("articles");
      ch.onmessage = (event) => {
        (globalThis as { __received?: string[] }).__received!.push(
          String(event.data),
        );
      };
    });

    // Tab A POSTs through its own SharedWorker port. The shared
    // SharedWorker dispatches → controller.create → Article.create
    // → broadcasts_to fires → broadcast("articles", "<turbo-stream
    // ...>"). Tab B's BroadcastChannel receives.
    const post = await probeSharedWorker(tabA, {
      method: "POST",
      path: "/articles",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body:
        "article%5Btitle%5D=Multi-tab+broadcast+test" +
        "&article%5Bbody%5D=Posted+from+tab+A+to+test+broadcast+to+tab+B.",
    });

    expect(
      post.status,
      `POST returned ${post.status}: ${post.body.slice(0, 200)}`,
    ).toBeLessThan(400);

    // Wait for tab B to see the broadcast. 5s is plenty — the
    // broadcast posts synchronously after INSERT inside the
    // SharedWorker; latency is just the postMessage hop to tab B.
    await tabB.waitForFunction(
      () => {
        const r = (globalThis as { __received?: string[] }).__received;
        return Array.isArray(r) && r.length > 0;
      },
      null,
      { timeout: 5_000 },
    );

    const received = await tabB.evaluate(
      () => (globalThis as { __received?: string[] }).__received ?? [],
    );

    expect(
      received.length,
      "tab B should receive at least one broadcast on 'articles' channel",
    ).toBeGreaterThan(0);
    expect(
      received[0],
      `first broadcast should be a turbo-stream fragment, got: ${received[0]?.slice(0, 200)}`,
    ).toMatch(/<turbo-stream/);

    await context.close();
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
      () => (window as Window & { __juntos__?: { ready?: boolean } }).__juntos__?.ready === true,
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

  test("DELETE via _method override removes article + redirects to /articles", async ({
    page,
  }) => {
    // Rails forms can't issue DELETE directly, so the scaffold form
    // POSTs with `_method=delete` and the dispatcher rewrites the
    // method (server-worker.ts dispatchRequest, mirror of
    // server.ts handleRequest). Two server-side surfaces exercise
    // here that no other probe covers:
    //
    //   - the _method-override branch in dispatchRequest
    //   - controller.destroy → ActiveRecord.adapter.delete (UPDATE/
    //     DELETE was never round-tripped before — only INSERT)
    //
    // Seed the test by creating an article first so we have a
    // real id to delete (self-contained; doesn't depend on
    // fixtures / seeds in the database state).

    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () => (window as Window & { __juntos__?: { ready?: boolean } }).__juntos__?.ready === true,
      null,
      { timeout: 15_000 },
    );

    // 1. Create.
    const createBody =
      "article%5Btitle%5D=Article+to+delete" +
      "&article%5Bbody%5D=Will+be+removed+by+the+DELETE+probe.";
    const create = await probeSharedWorker(page, {
      method: "POST",
      path: "/articles",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: createBody,
    });
    expect(create.status, `create POST returned ${create.status}`).toBeLessThan(400);
    const createdLocation =
      create.headers["location"] ?? create.headers["Location"] ?? "";
    const idMatch = createdLocation.match(/^\/articles\/(\d+)$/);
    expect(idMatch, `create should redirect to /articles/<id>; got "${createdLocation}"`).not.toBeNull();
    const id = idMatch![1];

    // 2. Delete via POST + _method override.
    const deleteResponse = await probeSharedWorker(page, {
      method: "POST",
      path: `/articles/${id}`,
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: "_method=delete",
    });

    expect(
      deleteResponse.status,
      `DELETE returned ${deleteResponse.status}: ${deleteResponse.body.slice(0, 300)}`,
    ).toBeLessThan(500);
    expect(
      [302, 303],
      `expected redirect after destroy; got ${deleteResponse.status}`,
    ).toContain(deleteResponse.status);

    const redirectTo =
      deleteResponse.headers["location"] ?? deleteResponse.headers["Location"];
    expect(redirectTo, "destroy should redirect").toBeTruthy();
    expect(
      redirectTo,
      `destroy should redirect to /articles index; got ${redirectTo}`,
    ).toBe("/articles");
  });

  test("POST /articles with invalid params returns 422 (not 5xx, not redirect)", async ({
    page,
  }) => {
    // Article validations: title presence, body presence + minimum
    // length 10 (see fixtures/real-blog/app/models/article.rb).
    // Empty title + 4-char body fail both. Controller flow:
    //   Article.new(article_params)
    //   if @article.save  → redirect_to @article (302/303)
    //   else              → render :new, status: :unprocessable_entity (422)
    //
    // This probe exercises the validation-failure path: form
    // parsing reaches the controller (already covered by the create
    // test), validations fire, the framework's HWIA-shaped errors
    // populate, the re-render returns with status 422. Different
    // failure modes get different statuses — 5xx means the
    // controller crashed before validations ran; 302/303 means
    // validations didn't fire; 422 is the happy "validation
    // failure observable to the client" path.

    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () => (window as Window & { __juntos__?: { ready?: boolean } }).__juntos__?.ready === true,
      null,
      { timeout: 15_000 },
    );

    const body =
      "article%5Btitle%5D=" + // empty title — fails presence
      "&article%5Bbody%5D=tiny"; // 4 chars — fails minimum length 10

    const response = await probeSharedWorker(page, {
      method: "POST",
      path: "/articles",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body,
    });

    expect(
      response.status,
      `validation-failure POST returned ${response.status}: ${response.body.slice(0, 300)}`,
    ).toBe(422);

    // No Location header on a 422 — it's a re-render, not a
    // navigation. (Also asserts the path didn't accidentally take
    // the success branch.)
    const location =
      response.headers["location"] ?? response.headers["Location"];
    expect(location, "422 should not set Location").toBeFalsy();

    // Body should be HTML (the re-rendered form).
    expect(response.headers["content-type"] ?? "").toContain("text/html");
  });

  test("broadcast prepends a new article into a subscribed tab's DOM", async ({
    browser,
  }) => {
    // End-to-end Turbo Stream DOM application (the gap the raw
    // BroadcastChannel probe above can't see): the index view calls
    // `turbo_stream_from "articles"`, which renders the Rails Action-Cable
    // `<turbo-cable-stream-source>` element. The worker has no cable, so
    // client.ts rewrites it into `<juntos-stream-source channel="articles">`
    // — whose connectedCallback subscribes over BroadcastChannel. When
    // tab A creates an article, Article#broadcasts_to prepends a
    // `<turbo-stream action="prepend" target="articles">` onto channel
    // "articles"; tab B must render it into `#articles` with no reload.
    const context = await browser.newContext();
    const tabA = await context.newPage();
    const tabB = await context.newPage();

    await Promise.all([
      tabA.goto("/articles", { waitUntil: "domcontentloaded" }),
      tabB.goto("/articles", { waitUntil: "domcontentloaded" }),
    ]);
    for (const tab of [tabA, tabB]) {
      await tab.waitForFunction(
        () => (window as Window & { __juntos__?: { ready?: boolean } }).__juntos__?.ready === true,
        null,
        { timeout: 15_000 },
      );
    }

    // The cable element must have been rewritten + subscribed in tab B.
    const wiring = await tabB.evaluate(() => ({
      cableLeft: document.querySelectorAll("turbo-cable-stream-source").length,
      channels: [...document.querySelectorAll("juntos-stream-source")].map((e) =>
        e.getAttribute("channel"),
      ),
      target: !!document.getElementById("articles"),
    }));
    expect(wiring.cableLeft, "turbo-cable-stream-source should be rewritten away").toBe(0);
    expect(wiring.channels, "tab B should subscribe to the 'articles' stream").toContain("articles");
    expect(wiring.target, "index should have the #articles prepend target").toBe(true);

    const marker = `Broadcast DOM proof ${Date.now()}`;
    const before = await tabB.evaluate(
      (m) => document.getElementById("articles")?.textContent?.includes(m) ?? false,
      marker,
    );
    expect(before, "marker must not be present before the broadcast").toBe(false);

    // Create from tab A (its own port; still fires the shared broadcast).
    const create = await probeSharedWorker(tabA, {
      method: "POST",
      path: "/articles",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body:
        `article%5Btitle%5D=${encodeURIComponent(marker)}` +
        "&article%5Bbody%5D=Posted+from+tab+A+to+prove+broadcast+DOM+application.",
    });
    expect(create.status, `create returned ${create.status}`).toBeLessThan(400);

    // Tab B renders the prepended article via its subscription — no reload.
    await tabB.waitForFunction(
      (m) => document.getElementById("articles")?.textContent?.includes(m) ?? false,
      marker,
      { timeout: 8_000 },
    );

    await context.close();
  });
});
