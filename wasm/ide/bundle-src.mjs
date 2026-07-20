// Bundle a Rails app's analyzable sources into app-src.json for the
// /ide/ page. CI runs this against a pinned-SHA Mastodon checkout; for
// local development point it anywhere:
//
//   node bundle-src.mjs ~/git/mastodon [out.json] [--name mastodon] [--open app/controllers/statuses_controller.rb]
//
// Ships only the text the analyzer ingests (.rb + template files under
// the app dirs, plus db/schema.rb, config/routes.rb, and the
// config/routes/ draw(:name) split files). When bundling a third-party
// app, include its LICENSE and record the commit — the output embeds
// both when discoverable.

import { readFile, readdir, writeFile } from "node:fs/promises";
import { execSync } from "node:child_process";
import { relative, join } from "node:path";

const args = process.argv.slice(2);
const root = args[0];
if (!root) {
  console.error("usage: node bundle-src.mjs <rails-app-root> [out.json] [--name N] [--open PATH]");
  process.exit(2);
}
const out = args[1] && !args[1].startsWith("--") ? args[1] : "app-src.json";
const flag = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};

const src = {};
async function walk(dir, rootDir) {
  let entries;
  try { entries = await readdir(dir, { withFileTypes: true }); } catch { return; }
  for (const e of entries) {
    const full = join(dir, e.name);
    if (e.isDirectory()) await walk(full, rootDir);
    else if (/\.(rb|erb|haml|jbuilder|ruby|rabl|slim)$/.test(e.name)) {
      src[relative(rootDir, full)] = await readFile(full, "utf8");
    }
  }
}
// Rails-convention dirs, plus the Roda + Sequel layout (models/, views/,
// db/migrate/, and the root-level app.rb / db.rb / seeds.rb / config.ru —
// config.ru is what the roda front-end dispatches on). Each walk/read is a
// no-op when the dir/file doesn't exist, so one list serves both shapes.
for (const sub of ["app", "extras", "lib", "config/routes", "models", "views", "db/migrate"]) {
  await walk(join(root, sub), root);
}
for (const single of [
  "db/schema.rb",
  "config/routes.rb",
  "config.ru",
  "app.rb",
  "db.rb",
  "seeds.rb",
]) {
  try { src[single] = await readFile(join(root, single), "utf8"); } catch {}
}

let commit = flag("--commit");
if (!commit) {
  try {
    commit = execSync("git rev-parse HEAD", { cwd: root }).toString().trim();
  } catch {}
}
let license;
for (const f of ["LICENSE", "LICENSE.md", "LICENSE.txt", "COPYING", "MIT-LICENSE"]) {
  try { license = await readFile(join(root, f), "utf8"); break; } catch {}
}

const bundle = {
  name: flag("--name") || root.split("/").filter(Boolean).pop(),
  commit,
  license,
  open: flag("--open"),
  src,
};
await writeFile(out, JSON.stringify(bundle));
const mb = (JSON.stringify(bundle).length / 1024 / 1024).toFixed(1);
console.log(`${out}: ${Object.keys(src).length} files, ${mb} MB${commit ? `, @${commit.slice(0, 12)}` : ""}`);
