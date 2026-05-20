//! `build-site` — assemble the GitHub Pages site.
//!
//! For each target language, ingest the real-blog fixture, run the
//! analyzer, emit the project into memory, and write three artifacts
//! to `_site/browse/<lang>.{json,tgz,zip}`:
//!
//!   - `.json`: `{ language, files: [{ path, content }] }` — drives
//!     the interactive browse tab on the landing page.
//!   - `.tgz`: gzipped tar containing each emitted file at its
//!     canonical path; `tar -xzf <lang>.tgz` reproduces the
//!     transpile output as a self-contained tree.
//!   - `.zip`: same payload, deflate-compressed; for users on
//!     platforms without convenient tar (Windows Explorer can
//!     extract zip natively).
//!
//! For the "ruby" tab, walk the fixture itself and include its source
//! files; the archive structure mirrors the fixture directory.
//!
//! Static assets (landing page + browse subpath) are copied from
//! `site/` into `_site/`.
//!
//! Usage:
//!
//!     cargo run --bin build-site -- [FIXTURE] [OUTDIR]
//!
//! Defaults: `fixtures/real-blog` and `_site`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use flate2::Compression;
use flate2::write::GzEncoder;
use roundhouse::analyze::Analyzer;
use roundhouse::emit::{self, EmittedFile};
use roundhouse::ingest::ingest_app;
use zip::write::SimpleFileOptions;

// `blog` is the original Rails source fixture (verbatim app
// directory walk). `ruby` is the emitted CRuby-runnable tree
// (lowered emit + scaffold + ruby_overlay + static assets, with
// db_cruby.rb swapped in as the per-target Db shim). `spinel` is
// the FFI-shim variant of the same emit, targeting the future
// spinel-AOT runner. `typescript-worker` is the SharedWorker
// browser deployment of the TypeScript target — same emit pipeline,
// picked via `DeploymentProfile::worker()`. Listed alongside the
// language targets so it shows up in the browse archive matrix.
const TARGETS: &[&str] = &[
    "blog", "spinel", "ruby", "crystal", "elixir", "go", "python", "rust", "typescript",
    "typescript-worker",
];

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let fixture = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("fixtures/real-blog"));
    let out = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("_site"));

    match run(&fixture, &out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("build-site: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(fixture: &Path, out: &Path) -> Result<(), String> {
    if out.exists() {
        fs::remove_dir_all(out).map_err(|e| format!("clean {}: {e}", out.display()))?;
    }
    fs::create_dir_all(out.join("browse"))
        .map_err(|e| format!("mkdir {}: {e}", out.display()))?;

    copy_site_assets(out)?;
    copy_create_blog(out)?;

    let mut app = ingest_app(fixture)
        .map_err(|e| format!("ingest {}: {e}", fixture.display()))?;
    Analyzer::new(&app).analyze(&mut app);

    for target in TARGETS {
        let files = match *target {
            "blog" => blog_files(fixture)?,
            "spinel" => spinel_files(&app)?,
            "ruby" => ruby_runtime_files(&app, fixture)?,
            "crystal" => sort_files(emit::crystal::emit(&app)),
            "elixir" => sort_files(emit::elixir::emit(&app)),
            "go" => sort_files(emit::go::emit(&app)),
            "python" => sort_files(emit::python::emit(&app)),
            "rust" => sort_files(emit::rust::emit(&app)),
            "typescript" => sort_files(emit::typescript::emit(&app)),
            "typescript-worker" => sort_files(emit::typescript::emit_with_profile(
                &app,
                &roundhouse::profile::DeploymentProfile::worker(),
            )),
            _ => unreachable!(),
        };

        let json_path = out.join("browse").join(format!("{target}.json"));
        fs::write(&json_path, write_manifest_json(target, &files))
            .map_err(|e| format!("write {}: {e}", json_path.display()))?;
        eprintln!("wrote {}", json_path.display());

        let tgz_path = out.join("browse").join(format!("{target}.tgz"));
        write_tgz(&tgz_path, target, &files)?;
        eprintln!("wrote {}", tgz_path.display());

        let zip_path = out.join("browse").join(format!("{target}.zip"));
        write_zip(&zip_path, target, &files)?;
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

/// Copy `scripts/create-blog` to `_site/create-blog`. The fixture's
/// generator — running it produces the same Rails app that lives in
/// `fixtures/real-blog/` and that the `ruby` browse tab / archive
/// expose. Standalone download so consumers can regenerate the
/// fixture upstream without checking out Roundhouse. fs::copy
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

fn sort_files(files: Vec<EmittedFile>) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = files
        .into_iter()
        .map(|f| (f.path.to_string_lossy().into_owned(), f.content))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// "blog" archive: the original Rails source fixture, walked
/// verbatim. The archive structure mirrors the fixture directory
/// — `tar -xzf blog.tgz` reproduces the input that Roundhouse
/// transpiles, useful as a reference for "what does the input
/// look like" and as a downloadable starting point.
fn blog_files(fixture: &Path) -> Result<Vec<(String, String)>, String> {
    let mut files: Vec<(String, String)> = Vec::new();
    walk_ruby(fixture, fixture, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// "ruby" archive: the emitted CRuby-runnable tree. Starts from
/// the spinel-target file set (scaffold + runtime + tests + lowered
/// emit) and applies three CRuby-specific overlays — the same
/// layering the outer Makefile's `ruby-transpile` rule does. Stays
/// in lockstep with that rule; if you change one, change the other.
///
///   1. Db shim swap: drop the FFI variant (`runtime/db.rb`) and
///      rename the gem-backed variant (`runtime/db_cruby.rb`) into
///      its place. CRuby uses the gem; spinel-AOT uses the FFI.
///   2. ruby_overlay: CGI-shaped main.rb, Rakefile, config.ru,
///      config/puma.rb, cable.rb at root. Overrides the Tep-based
///      scaffold defaults that the spinel target keeps.
///   3. Source-app static assets: `app/javascript/` (importmap-
///      served JS) and `public/` (icons, error pages, robots.txt)
///      copied from the fixture verbatim. The lowered emit doesn't
///      produce these; they're verbatim assets, not transpilable
///      Ruby. Binary files (e.g. icon.png) are silently skipped
///      by `walk_dir_into` since `EmittedFile.content: String`
///      can't carry binary blobs — the archive is text-only.
///
/// The seeded `tmp/blog.sqlite3` that the Makefile copies in is
/// NOT included here for the same binary-content reason; consumers
/// of the archive get an empty DB on first boot. Schema.load! is
/// idempotent and creates tables on startup, so the archive is
/// still runnable.
fn ruby_runtime_files(
    app: &roundhouse::App,
    fixture: &Path,
) -> Result<Vec<(String, String)>, String> {
    let mut files = spinel_files(app)?;

    files.retain(|(p, _)| p != "runtime/db.rb");
    for (path, _) in files.iter_mut() {
        if path == "runtime/db_cruby.rb" {
            *path = "runtime/db.rb".to_string();
        }
    }

    walk_dir_into(
        Path::new("runtime/spinel/scaffold/ruby_overlay"),
        "",
        &mut files,
    )?;

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

/// Spinel-target files: the lowered emit (app/, config/, test/{models,
/// controllers,fixtures,test_helper}) plus the scaffold + runtime
/// overlays that `make spinel-transpile` adds. Mirrors the Makefile's
/// `cp -r runtime/spinel/scaffold/`, `cp -r runtime/spinel/test/`,
/// `cp -r runtime/ruby/{active_record,action_view,
/// action_controller,action_dispatch} runtime/ruby/*.rb runtime/spinel/*.rb`
/// steps so the archive is self-contained — `tar -xzf spinel.tgz &&
/// cd spinel && make spinel-test` works without a Roundhouse checkout.
fn spinel_files(app: &roundhouse::App) -> Result<Vec<(String, String)>, String> {
    // Order matches the Makefile's `make spinel-transpile`: scaffold
    // first, then runtime/spinel/test, then runtime files, then the
    // lowered emit on top. `dedupe_last_wins` resolves overlap (e.g.
    // emit_spinel's `test/test_helper.rb` supersedes the scaffold's
    // canonical version) — same precedence the Makefile achieves via
    // sequential cp.
    let mut files: Vec<(String, String)> = Vec::new();

    // Verbatim scaffold at the root: main.rb, Makefile, Gemfile,
    // server/, tools/, app/views.rb, app/assets/, README.md, etc.
    walk_dir_into(Path::new("runtime/spinel/scaffold"), "", &mut files)?;

    // Target-specific tests under test/. `.rb` files land under
    // test/, `.rbs` sidecars route to sig/test/ — same one-sig-root
    // policy as runtime/<sub>/ below. Without partitioning, hand-
    // maintained sidecars (e.g. test/test_helper.rbs) would land at
    // <out>/test/ where spinel's `--rbs sig` flag doesn't reach them.
    walk_dir_partitioned(
        Path::new("runtime/spinel/test"),
        "test/",
        "sig/test/",
        &mut files,
    )?;

    // Spinel-target primitives flat under runtime/.
    walk_dir_flat(Path::new("runtime/spinel"), &["rb"], "runtime/", &mut files)?;

    // Framework Ruby modules + bridge .rb files under runtime/. The
    // companion `.rbs` sidecars route to `sig/runtime/` (one sig/
    // root for `spinel --rbs DIR` + Steep — see
    // project_rbs_emit_landed.md).
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

    // Emit on top — overrides any path the scaffold/runtime walks
    // also produced (e.g. test/test_helper.rb).
    files.extend(sort_files(emit::ruby::emit_spinel(app)));

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
/// (build/static/node_modules/tmp/package.json) plus `vendor/` and
/// `coverage/`, which the scaffold doesn't gitignore (since git
/// ignores them via global rules) but CI's `bundler-cache: true`
/// populates with read-only gem trees that EACCES the explode step.
///
/// `ruby_overlay` is the CRuby-target-specific scaffold overlay
/// (Rakefile, config.ru, config/puma.rb) — the outer Makefile
/// copies its contents to the top of the Ruby emit tree; the
/// build-site walker must NOT include the subdir verbatim or the
/// manifest re-creates it inside the emit on every transpile.
const SKIP_DIRS: &[&str] = &[
    "vendor", "node_modules", "build", "static", "tmp", "coverage", "log", ".bundle",
    "ruby_overlay",
];

/// Walk `src` recursively, collecting every readable text file as
/// `(prefix + relative_path, content)`. Skips dotfiles, unreadable
/// (binary) files, and well-known dev/build directories listed in
/// `SKIP_DIRS`.
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

/// Walk `src` non-recursively, collecting only files whose extension
/// is in `exts`. Used to gather `runtime/spinel/*.rb` without
/// recursing into `runtime/spinel/{scaffold,test}` (those are walked
/// separately into different output prefixes).
/// Walk `src` recursively, routing `.rb` files under `rb_prefix` and
/// `.rbs` files under `rbs_prefix`. Other extensions and dotfiles are
/// skipped. Used to split runtime/ruby/<sub>/ between the load-path
/// tree (runtime/) and the typed sidecar tree (sig/runtime/) in one
/// pass.
fn walk_dir_partitioned(
    src: &Path,
    rb_prefix: &str,
    rbs_prefix: &str,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    if !src.exists() {
        return Err(format!("missing {}/", src.display()));
    }
    let mut stack: Vec<(std::path::PathBuf, String)> =
        vec![(src.to_path_buf(), String::new())];
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

/// Write a gzipped tar containing each emitted file at `<language>/<path>`
/// — the leading `<language>/` directory means `tar -xzf rust.tgz`
/// extracts into a `rust/` subdirectory rather than scattering files
/// into cwd. Mode 0644 for files, mtime 0 for reproducible builds.
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

/// Write a zip with the same structure as the tgz: each emitted file
/// at `<language>/<path>`, deflate-compressed.
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
