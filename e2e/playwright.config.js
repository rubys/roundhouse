import { defineConfig, devices } from '@playwright/test'

// Playwright smoke tests run against a *target* server already booted by
// scripts/e2e (default port 3000), backed by a fresh DB seeded from the
// archive's own db/seed.sql. There is intentionally no `webServer` block:
// the server lifecycle (extract archive → seed → build → boot) is managed
// by scripts/e2e, both locally and in CI.
//
// E2E_BASE_URL overrides the target URL (scripts/e2e sets it). Defaults to
// the same :3000 the compare harness uses for target servers.
//
// E2E_SKIP is a space- or comma-separated list of spec basenames to skip
// (e.g. "tailwind action_cable") — used by the per-target CI matrix to gate
// specs a target can't yet satisfy, so the job is green-able while the gap
// is still recorded. See scripts/e2e and the e2e-<target> CI jobs.
const SKIP = (process.env.E2E_SKIP || '')
  .split(/[\s,]+/)
  .filter(Boolean)

export default defineConfig({
  testDir: '.',
  testIgnore: SKIP.map(name => `**/${name}*.spec.js`),
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  reporter: process.env.CI ? [['github'], ['list']] : 'list',
  use: {
    baseURL: process.env.E2E_BASE_URL || 'http://localhost:3000',
    trace: 'on-first-retry',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
  ],
})
