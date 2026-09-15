// What a Rails (or Roda + Sequel) checkout contributes to an analyzer bundle —
// the ONE definition shared by the Node bundler (ide/bundle-src.mjs, which CI
// runs over the pinned apps) and the in-browser folder loader
// (local-bundle.mjs, which reads a visitor's own checkout). Keeping the rules
// here means a file the shipped bundles ingest is exactly a file the local
// loader ingests, and vice versa.

// Only the text the analyzer reads: Ruby plus the template languages it
// ingests. Assets, JS, images, node_modules and the like never leave disk.
export const SOURCE_EXT = /\.(rb|erb|haml|jbuilder|ruby|rabl|slim)$/;

// Rails-convention dirs, plus the Roda + Sequel layout (models/, views/,
// db/migrate/, and the root-level app.rb / db.rb / seeds.rb / config.ru —
// config.ru is what the roda front-end dispatches on). Each walk/read is a
// no-op when the dir/file doesn't exist, so one list serves both shapes.
export const WALK_DIRS = ["app", "extras", "lib", "config/routes", "models", "views", "db/migrate"];
export const SINGLE_FILES = ["db/schema.rb", "config/routes.rb", "config.ru", "app.rb", "db.rb", "seeds.rb"];

// Does a root-relative path belong in the bundle? Used by loaders that
// enumerate the whole tree (a `webkitdirectory` file input) rather than
// walking only the dirs above.
export function wantsPath(rel) {
  if (SINGLE_FILES.includes(rel)) return true;
  if (!SOURCE_EXT.test(rel)) return false;
  return WALK_DIRS.some((d) => rel.startsWith(d + "/"));
}
