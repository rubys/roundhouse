//! Project-shape assembly: given an ingested + analyzed [`App`] plus
//! a [`BuildTarget`], return the canonical file set for that target as
//! a `Vec<(path, content)>`. Shared by the `roundhouse` binary's
//! `--target LANG` (single target → directory) and `--site` (all
//! targets → archives) modes.
//!
//! The per-target dispatch matches `src/emit/`: most targets are a
//! thin wrapper over `emit::<lang>::emit(&app)`, while `spinel` and
//! `ruby` compose a scaffold + runtime overlay on top of the lowered
//! emit (mirroring the Makefile's `ruby-transpile` / `spinel-transpile`
//! rules). `Blog` is a special target — the source fixture walked
//! verbatim, only used by the `--site` archive matrix.
//!
//! The emit dispatch is host-only because the scaffold/runtime walks
//! read from disk (`runtime/spinel/scaffold/`, `runtime/ruby/`); WASM
//! builds use a different entry point and don't pull this module.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use zip::write::SimpleFileOptions;

use crate::App;
use crate::analyze::Analyzer;
use crate::emit::{self, EmittedFile};
use crate::ingest::ingest_app;

/// Targets the `roundhouse` binary can produce. Matches the
/// `TARGETS` list in the legacy `build-site` binary plus the `Blog`
/// pseudo-target (verbatim source archive).
///
/// The transpile targets (`Spinel` through `TypescriptWorker`) are
/// valid for both `--target LANG` and `--site` modes. `Blog` is only
/// valid for `--site` — it's the source fixture, not a transpile
/// output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildTarget {
    /// Source fixture, walked verbatim. `--site` only.
    Blog,
    /// Spinel-target emit: scaffold + runtime + lowered app, FFI db.rb.
    Spinel,
    /// CRuby-target emit: spinel files + ruby_overlay + gem db.rb +
    /// fixture's app/javascript + public assets.
    Ruby,
    /// JRuby-target emit: byte-identical to the Ruby target except the
    /// SQLite backend — ships the JDBC `db_jruby.rb` (the `sqlite3` gem
    /// is a C extension with no JRuby build) so the same emitted source
    /// runs on the JVM.
    Jruby,
    Crystal,
    Elixir,
    Go,
    Python,
    Rust,
    Typescript,
    /// TypeScript emit under the `worker` deployment profile
    /// (SharedWorker browser deployment).
    TypescriptWorker,
}

impl BuildTarget {
    /// All targets that participate in `--site` archive generation,
    /// in the same order the legacy `build-site` binary iterated them.
    pub const ALL: &'static [BuildTarget] = &[
        BuildTarget::Blog,
        BuildTarget::Spinel,
        BuildTarget::Ruby,
        BuildTarget::Jruby,
        BuildTarget::Crystal,
        BuildTarget::Elixir,
        BuildTarget::Go,
        BuildTarget::Python,
        BuildTarget::Rust,
        BuildTarget::Typescript,
        BuildTarget::TypescriptWorker,
    ];

    /// Targets valid for `--target LANG` (transpile to directory).
    /// Excludes `Blog` (source-only) — `--target blog` would mean
    /// "copy the input to the output," which is a `cp -r`, not a
    /// transpile.
    pub const TRANSPILE: &'static [BuildTarget] = &[
        BuildTarget::Spinel,
        BuildTarget::Ruby,
        BuildTarget::Jruby,
        BuildTarget::Crystal,
        BuildTarget::Elixir,
        BuildTarget::Go,
        BuildTarget::Python,
        BuildTarget::Rust,
        BuildTarget::Typescript,
        BuildTarget::TypescriptWorker,
    ];

    /// CLI name. Stable — used in `--target X` and in
    /// `_site/browse/<name>.{json,tgz,zip}` archive filenames.
    pub fn as_str(self) -> &'static str {
        match self {
            BuildTarget::Blog => "blog",
            BuildTarget::Spinel => "spinel",
            BuildTarget::Ruby => "ruby",
            BuildTarget::Jruby => "jruby",
            BuildTarget::Crystal => "crystal",
            BuildTarget::Elixir => "elixir",
            BuildTarget::Go => "go",
            BuildTarget::Python => "python",
            BuildTarget::Rust => "rust",
            BuildTarget::Typescript => "typescript",
            BuildTarget::TypescriptWorker => "typescript-worker",
        }
    }

    /// Parse a CLI string. Returns `None` for unknown names.
    pub fn from_str(s: &str) -> Option<BuildTarget> {
        for t in BuildTarget::ALL {
            if t.as_str() == s {
                return Some(*t);
            }
        }
        None
    }
}

/// Quick-start README for a transpile target. Written into the
/// output directory by the `--target LANG` mode of the `roundhouse`
/// binary, unless the file set already contains a `README.md` (the
/// spinel and ruby targets ship a comprehensive scaffold README that
/// must not be overwritten).
///
/// Content is intentionally short: prerequisites, build, run, test,
/// and the regenerate command. Target-specific specifics (port
/// numbers, exact file paths) are best-effort — the README is a
/// pointer, not a contract.
pub fn target_readme(target: BuildTarget) -> String {
    let name = target.as_str();
    let body = match target {
        BuildTarget::Blog => {
            "Source fixture, walked verbatim. Not a transpile output — no \
             build commands apply. This archive exists so consumers can \
             download the input that Roundhouse transpiles.\n"
        }
        BuildTarget::Spinel | BuildTarget::Ruby | BuildTarget::Jruby => {
            // Should not reach: these targets ship a scaffold README
            // and `--target` mode skips generation when one is present.
            "See the scaffold-provided README.md for build/run/test \
             instructions.\n"
        }
        BuildTarget::Crystal => {
            "## Prerequisites\n\
             - Crystal 1.10+\n\
             - SQLite (system library)\n\n\
             ## Install dependencies\n\
             ```sh\n\
             shards install\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             crystal run main.cr\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             crystal spec\n\
             ```\n"
        }
        BuildTarget::Elixir => {
            "## Prerequisites\n\
             - Elixir 1.15+ (Mix)\n\n\
             ## Install dependencies\n\
             ```sh\n\
             mix deps.get\n\
             mix compile\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             mix run --no-halt\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             mix test\n\
             ```\n"
        }
        BuildTarget::Go => {
            "## Prerequisites\n\
             - Go 1.21+\n\n\
             ## Build\n\
             ```sh\n\
             go build ./...\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             go run .\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             go test ./...\n\
             ```\n"
        }
        BuildTarget::Python => {
            "## Prerequisites\n\
             - Python 3.11+\n\
             - `uv` (or pip)\n\n\
             ## Install dependencies\n\
             ```sh\n\
             uv sync     # or: pip install -r requirements.txt\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             uv run python main.py\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             uv run pytest\n\
             ```\n"
        }
        BuildTarget::Rust => {
            "## Prerequisites\n\
             - Rust 1.85+ (`cargo`)\n\
             - SQLite (system library)\n\n\
             ## Build\n\
             ```sh\n\
             cargo build --release\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             cargo run --release --bin app\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             cargo test\n\
             ```\n"
        }
        BuildTarget::Typescript => {
            "## Prerequisites\n\
             - Node.js 18+\n\n\
             ## Install dependencies\n\
             ```sh\n\
             npm install\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             npm start\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             npm test\n\
             ```\n"
        }
        BuildTarget::TypescriptWorker => {
            "Browser deployment as a SharedWorker. The emitted bundle \
             is loaded by a host HTML page — there's no standalone \
             server.\n\n\
             ## Prerequisites\n\
             - Node.js 18+ (for bundling)\n\n\
             ## Install + build\n\
             ```sh\n\
             npm install\n\
             npm run build\n\
             ```\n\n\
             ## Run\n\
             Open the host HTML page in a browser. The worker bundle \
             runs in a `SharedWorker` context.\n"
        }
    };
    format!(
        "# Roundhouse → {name}\n\n\
         Transpiled from a Rails source app by [Roundhouse]\
         (https://rubys.github.io/roundhouse/).\n\n\
         {body}\n\
         ## Regenerate\n\
         ```sh\n\
         roundhouse --target {name} -o <output-dir> <input-app>\n\
         ```\n"
    )
}

/// Produce the file set for `target`. `app` must already be ingested
/// and analyzed. `fixture` is the source-app path on disk — needed
/// by `Blog` (walks the fixture) and `Ruby` (copies `app/javascript`
/// and `public`).
///
/// Returned entries are `(relative_path, file_content)`, sorted by
/// path. Binary files (anything containing a NUL byte, or files that
/// don't decode as UTF-8) are silently skipped — the archive payload
/// is text-only by construction.
pub fn target_files(
    app: &App,
    fixture: &Path,
    target: BuildTarget,
) -> Result<Vec<(String, String)>, String> {
    match target {
        BuildTarget::Blog => blog_files(fixture),
        BuildTarget::Spinel => spinel_files(app, fixture),
        BuildTarget::Ruby => ruby_runtime_files(app, fixture),
        BuildTarget::Jruby => jruby_runtime_files(app, fixture),
        BuildTarget::Crystal => Ok(sort_files(emit::crystal::emit(app))),
        BuildTarget::Elixir => Ok(sort_files(emit::elixir::emit(app))),
        BuildTarget::Go => Ok(sort_files(emit::go::emit(app))),
        BuildTarget::Python => Ok(sort_files(emit::python::emit(app))),
        BuildTarget::Rust => Ok(sort_files(emit::rust::emit(app))),
        BuildTarget::Typescript => Ok(sort_files(emit::typescript::emit(app))),
        BuildTarget::TypescriptWorker => Ok(sort_files(emit::typescript::emit_with_profile(
            app,
            &crate::profile::DeploymentProfile::worker(),
        ))),
    }
}

/// Write `files` to `dest` — each entry's path is taken relative to
/// `dest`, parent dirs created as needed. Used by the `--target LANG`
/// mode of the `roundhouse` binary.
pub fn write_to_dir(files: &[(String, String)], dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
    for (path, content) in files {
        let full = dest.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        fs::write(&full, content)
            .map_err(|e| format!("write {}: {e}", full.display()))?;
    }
    Ok(())
}

/// Sort the emit output (`Vec<EmittedFile>`) into the `(path, content)`
/// shape this module uses. Stable by path so the archive matrix is
/// deterministic.
pub fn sort_files(files: Vec<EmittedFile>) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = files
        .into_iter()
        .map(|f| (f.path.to_string_lossy().into_owned(), f.content))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// "blog" archive: the original Rails source fixture, walked
/// verbatim. The archive structure mirrors the fixture directory.
fn blog_files(fixture: &Path) -> Result<Vec<(String, String)>, String> {
    let mut files: Vec<(String, String)> = Vec::new();
    walk_ruby(fixture, fixture, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// "ruby" archive: emitted CRuby-runnable tree. Starts from the
/// spinel-target file set and applies three CRuby-specific overlays
/// — same layering as the outer Makefile's `ruby-transpile` rule:
///
///   1. Db shim swap: drop the FFI `runtime/db.rb`, rename
///      `runtime/db_cruby.rb` into its place.
///   2. ruby_overlay: CGI-shaped main.rb, Rakefile, config.ru,
///      config/puma.rb, cable.rb at root.
///   3. Source-app static assets: `app/javascript/` and `public/`
///      from the fixture verbatim. Binary files are silently
///      skipped (text-only archive).
///
/// The seeded `tmp/blog.sqlite3` that the Makefile copies in is NOT
/// included — `Schema.load!` is idempotent so a fresh DB still boots.
fn ruby_runtime_files(
    app: &App,
    fixture: &Path,
) -> Result<Vec<(String, String)>, String> {
    let mut files = spinel_files(app, fixture)?;

    files.retain(|(p, _)| p != "runtime/db.rb");
    for (path, _) in files.iter_mut() {
        if path == "runtime/db_cruby.rb" {
            *path = "runtime/db.rb".to_string();
        }
    }

    // Tep is a spinel-only transport (FFI HTTP server). The CRuby
    // target uses Puma + Rack via the ruby_overlay; nothing in its
    // boot path requires Tep, and the unsubstituted @TEP_SPHTTP_O@
    // placeholder in net.rb would confuse anyone exploring the tree.
    files.retain(|(p, _)| !p.starts_with("runtime/tep/"));

    walk_dir_into(
        Path::new("runtime/spinel/scaffold/ruby_overlay"),
        "",
        &mut files,
    )?;

    // The source app's `app/javascript/` + `public/` static assets are
    // already folded in by `spinel_files` (both targets need them — the
    // spinel binary now serves `/assets/*` too). Nothing CRuby-specific
    // to add here beyond the overlay above.
    Ok(dedupe_last_wins(files))
}

/// "jruby" archive: byte-identical to the "ruby" tree except the SQLite
/// backend. Same layering as `ruby_runtime_files` — spinel files +
/// ruby_overlay (Puma + Rack `config.ru`, all of which run unchanged on
/// the JVM) — but the Db shim swap installs the JDBC-backed
/// `runtime/db_jruby.rb` as `runtime/db.rb` instead of the CRuby
/// gem-backed `db_cruby.rb`. The `sqlite3` gem is a C extension with no
/// JRuby build, so JRuby reaches SQLite over JDBC. The emitted app/,
/// config/, and framework runtime are identical to the CRuby target —
/// JRuby is a deployment (VM) variant, not a source variant.
fn jruby_runtime_files(
    app: &App,
    fixture: &Path,
) -> Result<Vec<(String, String)>, String> {
    let mut files = spinel_files(app, fixture)?;

    // Db shim swap: drop the FFI `runtime/db.rb` and the CRuby gem
    // backend, then promote the JDBC backend into `runtime/db.rb`.
    // `db_jruby.rb` is excluded from `spinel_files`' base set, so read
    // it from disk and inject it here (mirrors the gem swap the CRuby
    // target does to `db_cruby.rb`).
    files.retain(|(p, _)| p != "runtime/db.rb" && p != "runtime/db_cruby.rb");
    let db_jruby = fs::read_to_string("runtime/spinel/db_jruby.rb")
        .map_err(|e| format!("read runtime/spinel/db_jruby.rb: {e}"))?;
    files.push(("runtime/db.rb".to_string(), db_jruby));

    // Tep is a spinel-only transport (FFI HTTP server); JRuby uses Puma
    // + Rack via the ruby_overlay, same as the CRuby target.
    files.retain(|(p, _)| !p.starts_with("runtime/tep/"));

    walk_dir_into(
        Path::new("runtime/spinel/scaffold/ruby_overlay"),
        "",
        &mut files,
    )?;

    Ok(dedupe_last_wins(files))
}

/// Spinel-target files: lowered emit (app/, config/, test/) plus
/// scaffold + runtime overlays. Order matches `make spinel-transpile`
/// — scaffold first, runtime test/lib next, lowered emit on top.
/// `dedupe_last_wins` resolves overlap (e.g. emit_spinel's
/// `test/test_helper.rb` supersedes the scaffold's canonical version).
///
/// The source app's `app/javascript/` (the importmap JS entry +
/// Stimulus controllers) and `public/` icons are walked in verbatim:
/// `make assets` copies them under `static/assets/`, and the spinel
/// binary's `Main.dispatch` serves them at `/assets/*`. Binary files
/// (e.g. `icon.png`) are silently skipped — the archive is text-only.
fn spinel_files(app: &App, fixture: &Path) -> Result<Vec<(String, String)>, String> {
    let mut files: Vec<(String, String)> = Vec::new();

    walk_dir_into(Path::new("runtime/spinel/scaffold"), "", &mut files)?;

    walk_dir_partitioned(
        Path::new("runtime/spinel/test"),
        "test/",
        "sig/test/",
        &mut files,
    )?;

    walk_dir_flat(Path::new("runtime/spinel"), &["rb"], "runtime/", &mut files)?;

    // `db_jruby.rb` is the JRuby/JDBC Db backend — it uses Java interop
    // (`java_import`, `Java::`) that the CRuby and Spinel toolchains (and
    // the spinel-subset compliance gate) must never see. It is injected
    // only by `jruby_runtime_files`, so keep it out of the shared base.
    files.retain(|(p, _)| p != "runtime/db_jruby.rb");

    // Vendored Tep transport (FFI HTTP server). Both .rb files and
    // sphttp.c (precompiled to sphttp.o at transpile-post time).
    // Recursive walk picks the whole subtree.
    walk_dir_into(Path::new("runtime/spinel/tep"), "runtime/tep/", &mut files)?;

    for sub in [
        "active_record",
        "action_view",
        "action_controller",
        "action_dispatch",
    ] {
        walk_dir_partitioned(
            &Path::new("runtime/ruby").join(sub),
            &format!("runtime/{sub}/"),
            &format!("sig/runtime/{sub}/"),
            &mut files,
        )?;
    }
    for stem in [
        "active_record",
        "action_view",
        "action_controller",
        "action_dispatch",
        "inflector",
        "json_builder",
    ] {
        let rb = Path::new("runtime/ruby").join(format!("{stem}.rb"));
        let content = fs::read_to_string(&rb)
            .map_err(|e| format!("read {}: {e}", rb.display()))?;
        files.push((format!("runtime/{stem}.rb"), content));
        let rbs = Path::new("runtime/ruby").join(format!("{stem}.rbs"));
        if rbs.exists() {
            let rbs_content = fs::read_to_string(&rbs)
                .map_err(|e| format!("read {}: {e}", rbs.display()))?;
            files.push((format!("sig/runtime/{stem}.rbs"), rbs_content));
        }
    }

    files.extend(sort_files(emit::ruby::emit_spinel(app)));

    let js = fixture.join("app/javascript");
    if js.exists() {
        walk_dir_into(&js, "app/javascript/", &mut files)?;
    }
    let public = fixture.join("public");
    if public.exists() {
        walk_dir_into(&public, "public/", &mut files)?;
    }

    Ok(dedupe_last_wins(files))
}

/// Resolve duplicate paths by keeping the last-inserted entry, then
/// sort alphabetically. Matches the Makefile's sequential-cp
/// semantics where later copies overwrite earlier ones.
fn dedupe_last_wins(files: Vec<(String, String)>) -> Vec<(String, String)> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for (path, content) in files {
        map.insert(path, content);
    }
    map.into_iter().collect()
}

/// Directory names that are dev/build-only and must not appear in
/// the emitted output. Matches the scaffold's `.gitignore`-shape
/// plus `vendor/`/`coverage/` (CI's bundler-cache populates them
/// with read-only gem trees that EACCES the walk).
///
/// `ruby_overlay` is the CRuby-target-specific scaffold overlay; the
/// build walker must NOT include the subdir verbatim or the manifest
/// re-creates it inside the emit on every transpile.
const SKIP_DIRS: &[&str] = &[
    "vendor", "node_modules", "build", "static", "tmp", "coverage", "log", ".bundle",
    "ruby_overlay",
];

/// Walk `src` recursively, collecting every readable text file as
/// `(prefix + relative_path, content)`. Skips dotfiles, unreadable
/// (binary) files, and well-known dev/build directories.
fn walk_dir_into(
    src: &Path,
    prefix: &str,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    if !src.exists() {
        return Err(format!("missing {}/", src.display()));
    }
    let mut stack = vec![(src.to_path_buf(), String::from(prefix))];
    while let Some((dir, sub_prefix)) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
            let entry = entry.map_err(|e| format!("read entry: {e}"))?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let ty = entry.file_type().map_err(|e| format!("stat: {e}"))?;
            if ty.is_dir() && SKIP_DIRS.contains(&name_str.as_ref()) {
                continue;
            }
            let nested = format!("{sub_prefix}{name_str}");
            if ty.is_dir() {
                stack.push((path, format!("{nested}/")));
            } else {
                let content = match fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                out.push((nested, content));
            }
        }
    }
    Ok(())
}

/// Walk `src` recursively, routing `.rb` files under `rb_prefix` and
/// `.rbs` files under `rbs_prefix`. Other extensions and dotfiles are
/// skipped. Splits `runtime/ruby/<sub>/` between the load-path tree
/// (`runtime/`) and the typed sidecar tree (`sig/runtime/`) in one pass.
fn walk_dir_partitioned(
    src: &Path,
    rb_prefix: &str,
    rbs_prefix: &str,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    if !src.exists() {
        return Err(format!("missing {}/", src.display()));
    }
    let mut stack: Vec<(PathBuf, String)> = vec![(src.to_path_buf(), String::new())];
    while let Some((dir, sub)) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
            let entry = entry.map_err(|e| format!("read entry: {e}"))?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let ty = entry.file_type().map_err(|e| format!("stat: {e}"))?;
            if ty.is_dir() && SKIP_DIRS.contains(&name_str.as_ref()) {
                continue;
            }
            let nested = format!("{sub}{name_str}");
            if ty.is_dir() {
                stack.push((path, format!("{nested}/")));
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let prefix = match ext {
                "rb" => rb_prefix,
                "rbs" => rbs_prefix,
                _ => continue,
            };
            let content = match fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            out.push((format!("{prefix}{nested}"), content));
        }
    }
    Ok(())
}

/// Walk `src` non-recursively, collecting only files whose extension
/// is in `exts`. Used to gather `runtime/spinel/*.rb` without
/// recursing into `runtime/spinel/{scaffold,test}` (those are walked
/// separately into different output prefixes).
fn walk_dir_flat(
    src: &Path,
    exts: &[&str],
    prefix: &str,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    for entry in fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("read entry: {e}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext_match = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| exts.contains(&e))
            .unwrap_or(false);
        if !ext_match {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("non-utf8 filename: {}", path.display()))?;
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        out.push((format!("{prefix}{name}"), content));
    }
    Ok(())
}

/// Orchestrates the `--site` mode of the `roundhouse` binary: for
/// every `BuildTarget`, produce `_site/browse/<lang>.{json,tgz,zip}`,
/// and copy the static landing-page assets (`site/`) plus the
/// `scripts/create-blog` standalone download to the output root.
///
/// `fixture` is the source-app path; `out` is the site output dir
/// (typically `_site/`). The output dir is removed and recreated if
/// it exists, so callers should pick a dedicated path.
pub fn build_site(fixture: &Path, out: &Path) -> Result<(), String> {
    if out.exists() {
        fs::remove_dir_all(out).map_err(|e| format!("clean {}: {e}", out.display()))?;
    }
    fs::create_dir_all(out.join("browse"))
        .map_err(|e| format!("mkdir {}: {e}", out.display()))?;

    copy_site_assets(out)?;
    copy_create_blog(out)?;

    let mut app =
        ingest_app(fixture).map_err(|e| format!("ingest {}: {e}", fixture.display()))?;
    Analyzer::new(&app).analyze(&mut app);

    for target in BuildTarget::ALL {
        let files = target_files(&app, fixture, *target)?;
        let name = target.as_str();

        let json_path = out.join("browse").join(format!("{name}.json"));
        fs::write(&json_path, write_manifest_json(name, &files))
            .map_err(|e| format!("write {}: {e}", json_path.display()))?;
        eprintln!("wrote {}", json_path.display());

        let tgz_path = out.join("browse").join(format!("{name}.tgz"));
        write_tgz(&tgz_path, name, &files)?;
        eprintln!("wrote {}", tgz_path.display());

        let zip_path = out.join("browse").join(format!("{name}.zip"));
        write_zip(&zip_path, name, &files)?;
        eprintln!("wrote {}", zip_path.display());
    }

    Ok(())
}

fn copy_site_assets(out: &Path) -> Result<(), String> {
    let site = PathBuf::from("site");
    if !site.exists() {
        return Err(format!("missing {}/ (static assets)", site.display()));
    }
    copy_tree(&site, out)
}

/// Copy `scripts/create-blog` to `_site/create-blog`. fs::copy
/// preserves the executable bit on Unix.
fn copy_create_blog(out: &Path) -> Result<(), String> {
    let src = Path::new("scripts/create-blog");
    if !src.exists() {
        return Err(format!("missing {}", src.display()));
    }
    let dst = out.join("create-blog");
    fs::copy(src, &dst).map_err(|e| format!("copy {} → {}: {e}", src.display(), dst.display()))?;
    eprintln!("wrote {}", dst.display());
    Ok(())
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    for entry in fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("read entry: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let ty = entry.file_type().map_err(|e| format!("stat: {e}"))?;
        if ty.is_dir() {
            fs::create_dir_all(&dst_path)
                .map_err(|e| format!("mkdir {}: {e}", dst_path.display()))?;
            copy_tree(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("copy {} → {}: {e}", src_path.display(), dst_path.display()))?;
        }
    }
    Ok(())
}

fn write_manifest_json(language: &str, files: &[(String, String)]) -> String {
    #[derive(serde::Serialize)]
    struct File<'a> {
        path: &'a str,
        content: &'a str,
    }
    #[derive(serde::Serialize)]
    struct Manifest<'a> {
        language: &'a str,
        files: Vec<File<'a>>,
    }
    let manifest = Manifest {
        language,
        files: files
            .iter()
            .map(|(p, c)| File { path: p, content: c })
            .collect(),
    };
    serde_json::to_string(&manifest).expect("serialize manifest")
}

/// Write a gzipped tar with each emitted file at `<language>/<path>`.
/// The leading `<language>/` means `tar -xzf rust.tgz` extracts into
/// `rust/` rather than scattering files into cwd. Mode 0644, mtime 0
/// for reproducible builds.
fn write_tgz(out: &Path, language: &str, files: &[(String, String)]) -> Result<(), String> {
    let f = fs::File::create(out).map_err(|e| format!("create {}: {e}", out.display()))?;
    let gz = GzEncoder::new(f, Compression::default());
    let mut tar = tar::Builder::new(gz);
    for (path, content) in files {
        let mut header = tar::Header::new_gnu();
        let bytes = content.as_bytes();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_cksum();
        let archive_path = format!("{language}/{path}");
        tar.append_data(&mut header, &archive_path, bytes)
            .map_err(|e| format!("append {archive_path}: {e}"))?;
    }
    tar.into_inner()
        .and_then(|gz| gz.finish())
        .map_err(|e| format!("finalize {}: {e}", out.display()))?;
    Ok(())
}

fn write_zip(out: &Path, language: &str, files: &[(String, String)]) -> Result<(), String> {
    let f = fs::File::create(out).map_err(|e| format!("create {}: {e}", out.display()))?;
    let mut zip = zip::ZipWriter::new(f);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for (path, content) in files {
        let archive_path = format!("{language}/{path}");
        zip.start_file(&archive_path, opts)
            .map_err(|e| format!("zip start {archive_path}: {e}"))?;
        zip.write_all(content.as_bytes())
            .map_err(|e| format!("zip write {archive_path}: {e}"))?;
    }
    zip.finish()
        .map_err(|e| format!("zip finalize {}: {e}", out.display()))?;
    Ok(())
}

fn walk_ruby(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(String, String)>,
) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("read entry: {e}"))?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        let ty = entry.file_type().map_err(|e| format!("stat: {e}"))?;
        if ty.is_dir() {
            walk_ruby(root, &path, files)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| format!("strip prefix: {e}"))?;
            let content = match fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if content.contains('\0') {
                continue;
            }
            files.push((rel.to_string_lossy().into_owned(), content));
        }
    }
    Ok(())
}
