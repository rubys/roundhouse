// Pretest build pipeline:
//
//   1. `cargo run --bin emit_preview --target typescript --profile worker
//       --out tests/browser_smoke/.emitted fixtures/real-blog`
//   2. `npm install --silent` inside `.emitted/` (vite, sqlite-wasm, turbo)
//   3. `npm run build` (Vite produces dist/ + manifest meta injection)
//
// Runs before `playwright test` (chained via `npm test`). Has to run
// before Playwright loads, because Playwright validates `webServer.cwd`
// at config-load time — if `.emitted/` doesn't exist yet, you get a
// misleading `Failed to launch: spawn /bin/sh ENOENT` from the shell
// it tries to start there.
//
// Set `SKIP_EMIT=1` to reuse an existing `.emitted/dist` (useful when
// iterating on a single spec — the full loop is ~60s).

import { execSync } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const HARNESS_DIR = resolve(__dirname, "..");
const EMIT_DIR = resolve(HARNESS_DIR, ".emitted");
const REPO_ROOT = resolve(HARNESS_DIR, "..", "..");

if (process.env.SKIP_EMIT === "1" && existsSync(resolve(EMIT_DIR, "dist"))) {
  console.log(`[smoke] SKIP_EMIT=1 + existing dist/ — reusing ${EMIT_DIR}`);
  process.exit(0);
}

console.log(`[smoke] emitting worker target → ${EMIT_DIR}`);
execSync(
  `cargo run --quiet --bin emit_preview -- --target typescript --profile worker --out "${EMIT_DIR}" fixtures/real-blog`,
  { cwd: REPO_ROOT, stdio: "inherit" },
);

console.log(`[smoke] npm install`);
execSync("npm install --silent", { cwd: EMIT_DIR, stdio: "inherit" });

console.log(`[smoke] vite build`);
execSync("npm run build", { cwd: EMIT_DIR, stdio: "inherit" });

console.log(`[smoke] prebuild complete — playwright can now launch`);
