// Playwright config for roundhouse's SharedWorker browser smoke.
//
// Pipeline:
//   1. `npm run prebuild` (chained from `npm test`) emits + npm
//      installs + vite-builds the real-blog fixture into `.emitted/`.
//      Must run before Playwright loads this config — Playwright
//      validates `webServer.cwd` at load time.
//   2. `webServer` block below runs `npm run preview` against
//      `.emitted/` once the prebuild is done.
//   3. Specs in `./tests/` drive the served app via Chromium.
//
// `reuseExistingServer: !CI` keeps local iteration fast — kill the
// preview server once, leave it running across repeat
// `npx playwright test` invocations (use `npm run test-only`).

import { defineConfig, devices } from "@playwright/test";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const EMIT_DIR = resolve(__dirname, ".emitted");

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false, // single emitted+built tree shared across tests
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: process.env.CI ? "github" : "list",
  timeout: 60_000,
  use: {
    baseURL: "http://localhost:5173",
    trace: "on-first-retry",
  },
  webServer: {
    command: "npm run preview -- --port 5173 --strictPort",
    cwd: EMIT_DIR,
    url: "http://localhost:5173",
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
