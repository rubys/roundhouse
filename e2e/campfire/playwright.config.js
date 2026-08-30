import { defineConfig, devices } from '@playwright/test'

// Campfire's browser specs. The server lifecycle — transpile, `make
// assets`, seed, boot Puma, complete `/first_run` — belongs to
// scripts/campfire-e2e, which sets the three env vars below; there is
// deliberately no `webServer` block, matching the blog harness one
// directory up.
//
// NOT PARALLEL. Every spec drives ONE campfire, backed by one SQLite
// file that `database.yml` opens in `default_transaction_mode:
// immediate` — a single writer. Two specs posting messages at once
// serialize behind that lock in the best case and interleave rows in
// the room in the worst, and a room's message list is exactly what the
// behavioural specs assert on. The blog harness runs `fullyParallel`
// because its specs scope themselves to their own article; campfire has
// one room and every spec is in it.
export default defineConfig({
  testDir: '.',
  fullyParallel: false,
  // EVERY SPEC IS A "SECOND CLIENT", so every spec pays the stall.
  // Playwright gives each test its own browser context, and a
  // single-worker Tep server parks inside the first connection's
  // keep-alive loop instead of returning to accept() — measured at ~30s
  // before a new client is served (docs/pipeline/runtime.md § A
  // single-worker Tep server serializes on one keep-alive connection).
  // Playwright's 30s default expires INSIDE that window, so at the
  // default only the first spec in a run can pass.
  //
  // 90s is a workaround for a ledgered defect, not a considered budget.
  // Drop it back to the default when that entry closes.
  timeout: 90_000,
  workers: 1,
  forbidOnly: !!process.env.CI,
  // No retries. A retry turns a flaky asset 404 into a green run, and
  // this suite exists to catch exactly that.
  retries: 0,
  reporter: process.env.CI ? [['github'], ['list']] : 'list',
  use: {
    baseURL: process.env.CAMPFIRE_BASE_URL || 'http://localhost:3000',
    trace: 'on-first-retry',
    // Campfire's own system tests run 1400x1400; the room layout moves
    // its sidebar below a breakpoint and the composer is what most
    // specs reach for.
    viewport: { width: 1400, height: 1400 },
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
  ],
})
