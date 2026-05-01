//! Whole-app orchestrator: walks a Rails app directory, calls the
//! per-domain ingesters, and assembles an `App`. Also owns the small
//! DSLs that don't warrant their own submodule — `config/importmap.rb`
//! and the `.rb` / `.yml` / `.erb` file walkers.
//!
//! All filesystem access goes through the [`Vfs`] trait so that the
//! ingest pipeline drives both the on-disk Rails app (CLI) and an
//! in-memory tree (wasm transpile entry point). [`ingest_app`] is the
//! convenience wrapper for the disk case.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ruby_prism::{Node, parse};

use crate::App;
use crate::vfs::{FsVfs, MapVfs, Vfs};

use super::controller::ingest_controller;
use super::expr::ingest_ruby_program;
use super::fixture::ingest_fixture_file;
use super::library_class::{ClassKind, classify_class_file, ingest_library_class};
use super::model::ingest_model;
use super::routes::ingest_routes;
use super::schema::ingest_schema;
use super::test::ingest_test_file;
use super::view::ingest_view;
use super::{IngestError, IngestResult};

/// Ingest an entire Rails app directory from disk.
pub fn ingest_app(dir: &Path) -> IngestResult<App> {
    ingest_app_with_vfs(&FsVfs::new(), dir)
}

/// Ingest a Rails app from an in-memory `path → bytes` tree. Path keys
/// are interpreted relative to a virtual root (typically a single
/// segment like `app/`); the tree itself defines the root layout, so
/// callers usually pass `Path::new("")` for `root`.
pub fn ingest_app_from_tree(tree: HashMap<PathBuf, Vec<u8>>) -> IngestResult<App> {
    ingest_app_with_vfs(&MapVfs::new(tree), Path::new(""))
}

/// The actual whole-app walker. Generic over [`Vfs`] so it can read
/// from disk or from an in-memory map without code duplication.
pub fn ingest_app_with_vfs<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<App> {
    let mut app = App::new();

    let schema_path = dir.join("db/schema.rb");
    if vfs.exists(&schema_path) {
        let source = vfs.read(&schema_path)?;
        app.schema = ingest_schema(&source, &schema_path.display().to_string())?;
    }

    let models_dir = dir.join("app/models");
    if vfs.is_dir(&models_dir) {
        for entry in read_rb_files(vfs, &models_dir)? {
            let source = vfs.read(&entry)?;
            let path_str = entry.display().to_string();
            match classify_class_file(&source) {
                Some(ClassKind::Model) | None => {
                    if let Some(model) = ingest_model(&source, &path_str, &app.schema)? {
                        app.models.push(model);
                    }
                }
                Some(ClassKind::LibraryClass) => {
                    if let Some(lc) = ingest_library_class(&source, &path_str)? {
                        app.library_classes.push(lc);
                    }
                }
            }
        }
    }

    let controllers_dir = dir.join("app/controllers");
    if vfs.is_dir(&controllers_dir) {
        for entry in read_rb_files(vfs, &controllers_dir)? {
            let source = vfs.read(&entry)?;
            if let Some(controller) = ingest_controller(&source, &entry.display().to_string())? {
                app.controllers.push(controller);
            }
        }
    }

    let routes_path = dir.join("config/routes.rb");
    if vfs.exists(&routes_path) {
        let source = vfs.read(&routes_path)?;
        app.routes = ingest_routes(&source, &routes_path.display().to_string())?;
    }

    let views_dir = dir.join("app/views");
    if vfs.is_dir(&views_dir) {
        let erb_files = read_erb_files(vfs, &views_dir)?;
        for erb_path in erb_files {
            let source = vfs.read_to_string(&erb_path)?;
            let rel = erb_path
                .strip_prefix(&views_dir)
                .map_err(|_| IngestError::Unsupported {
                    file: erb_path.display().to_string(),
                    message: "view path outside views dir".into(),
                })?;
            let view = ingest_view(&source, rel, &erb_path.display().to_string())?;
            app.views.push(view);
        }
    }

    // Test files — `test/models/*_test.rb` and
    // `test/controllers/*_test.rb`. System tests under `test/system/`
    // still need a browser-driver runtime and stay out of scope.
    // Ingesting controller tests early (Phase 4-compile stage) lets
    // the emitter surface the HTTP primitives the tests reference,
    // even if those tests all skip pending the HTTP runtime.
    for subdir in ["test/models", "test/controllers"] {
        let tests_dir = dir.join(subdir);
        if vfs.is_dir(&tests_dir) {
            for entry in read_rb_files(vfs, &tests_dir)? {
                let source = vfs.read(&entry)?;
                if let Some(tm) = ingest_test_file(&source, &entry.display().to_string())? {
                    app.test_modules.push(tm);
                }
            }
        }
    }

    // YAML fixtures — `test/fixtures/*.yml`. The file stem is conventionally
    // the table name (articles.yml → articles). Values are kept as strings;
    // emitters interpret per column type and resolve Rails fixture-reference
    // shorthand (`article: one` → id of the `one` fixture in articles).
    let fixtures_dir = dir.join("test/fixtures");
    if vfs.is_dir(&fixtures_dir) {
        for entry in read_yml_files(vfs, &fixtures_dir)? {
            let source = vfs.read(&entry)?;
            let fixture = ingest_fixture_file(&source, &entry)?;
            app.fixtures.push(fixture);
        }
    }

    // `db/seeds.rb` — sample data loaded at startup. Ingested as a
    // top-level Ruby program (Seq of AR-create statements, usually
    // with an early-return guard). Analyzer types the body against
    // the model registry; TS emitter wraps it in
    // `async function run()` and main.ts invokes it if the DB is
    // fresh.
    let seeds_path = dir.join("db/seeds.rb");
    if vfs.exists(&seeds_path) {
        let source = vfs.read_to_string(&seeds_path)?;
        let expr = ingest_ruby_program(&source, &seeds_path.display().to_string())?;
        app.seeds = Some(expr);
    }

    // `config/importmap.rb` — tiny DSL of `pin` + `pin_all_from`
    // calls. Evaluated at ingest time to build an explicit
    // name→path list; `pin_all_from` expands by walking the
    // referenced directory. Feeds the emitted
    // `javascript_importmap_tags` helper.
    let importmap_path = dir.join("config/importmap.rb");
    if vfs.exists(&importmap_path) {
        let source = vfs.read_to_string(&importmap_path)?;
        let importmap = ingest_importmap(vfs, &source, dir, &importmap_path.display().to_string())?;
        if !importmap.pins.is_empty() {
            app.importmap = Some(importmap);
        }
    }

    // Logical stylesheets — file stems of `.css` files found in
    // `app/assets/stylesheets/` and `app/assets/builds/`. Rails'
    // `stylesheet_link_tag :app` with Propshaft + tailwindcss-rails
    // emits one `<link>` per stylesheet in these dirs; we mirror
    // by emitting the name list here.
    let mut stylesheets: Vec<String> = Vec::new();
    for subdir in ["app/assets/stylesheets", "app/assets/builds"] {
        let css_dir = dir.join(subdir);
        if !vfs.is_dir(&css_dir) {
            continue;
        }
        let mut entries: Vec<PathBuf> = vfs
            .read_dir(&css_dir)?
            .into_iter()
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("css"))
            .collect();
        entries.sort();
        for entry in entries {
            if let Some(stem) = entry.file_stem().and_then(|s| s.to_str()) {
                if !stylesheets.iter().any(|s| s == stem) {
                    stylesheets.push(stem.to_string());
                }
            }
        }
    }
    app.stylesheets = stylesheets;

    // `sig/**/*.rbs` — user-authored RBS sidecars for app code the
    // Rails conventions can't fully type on their own. Recursively
    // walk the sig dir, parse each file, merge into app.rbs_signatures
    // keyed by the declared class/module's fully-qualified name.
    let sig_dir = dir.join("sig");
    if vfs.is_dir(&sig_dir) {
        let mut stack = vec![sig_dir];
        while let Some(current) = stack.pop() {
            let mut entries: Vec<PathBuf> = vfs.read_dir(&current)?;
            entries.sort();
            for entry in entries {
                if vfs.is_dir(&entry) {
                    stack.push(entry);
                    continue;
                }
                if entry.extension().and_then(|s| s.to_str()) != Some("rbs") {
                    continue;
                }
                let source = vfs.read_to_string(&entry)?;
                let path_str = entry.display().to_string();
                let sigs =
                    crate::rbs::parse_app_signatures(&source).map_err(|message| {
                        IngestError::Parse {
                            file: path_str.clone(),
                            message,
                        }
                    })?;
                for (class_id, methods) in sigs {
                    app.rbs_signatures
                        .entry(class_id)
                        .or_default()
                        .extend(methods);
                }
            }
        }
    }

    Ok(app)
}

/// Ingest `config/importmap.rb`. The DSL has three common shapes:
///
/// ```ruby
/// pin "name"                    # → name → /assets/<name>.js
/// pin "name", to: "path.js"     # → name → /assets/path.js
/// pin_all_from "app/javascript/controllers", under: "controllers"
/// # → walks the dir, for each `foo_controller.js` pins
/// #    "controllers/foo_controller" → /assets/controllers/foo_controller.js
/// ```
///
/// We parse the AST directly rather than evaluating the Ruby so
/// ingest stays deterministic across environments. `preload:` /
/// `ignore:` kwargs are accepted-and-skipped; they don't affect
/// the rendered importmap tags' name→path entries for our
/// current needs.
fn ingest_importmap<V: Vfs + ?Sized>(
    vfs: &V,
    source: &str,
    app_dir: &Path,
    file: &str,
) -> IngestResult<crate::app::Importmap> {
    use crate::app::{Importmap, ImportmapPin};
    let result = parse(source.as_bytes());
    let root = result.node();
    let program = root.as_program_node().ok_or_else(|| IngestError::Parse {
        file: file.into(),
        message: "importmap.rb is not a program".into(),
    })?;
    let stmts = program.statements();
    let mut pins: Vec<ImportmapPin> = Vec::new();
    for stmt in stmts.body().iter() {
        let Some(call) = stmt.as_call_node() else {
            continue;
        };
        // Skip receiver-qualified calls; we only recognize top-
        // level `pin` / `pin_all_from`.
        if call.receiver().is_some() {
            continue;
        }
        let name = call.name();
        let name_str = name.as_slice();
        let Ok(method) = std::str::from_utf8(name_str) else {
            continue;
        };
        let args: Vec<Node<'_>> = call
            .arguments()
            .map(|a| a.arguments().iter().collect())
            .unwrap_or_default();

        match method {
            "pin" => {
                // First positional arg is the name (Str literal);
                // optional `to:` kwarg overrides the derived path.
                let Some(name_arg) = args.first() else {
                    continue;
                };
                let Some(name) = string_literal_value(name_arg) else {
                    continue;
                };
                let to = args.iter().skip(1).find_map(|a| extract_kwarg_str(a, "to"));
                let path = match to {
                    Some(filename) => format!("/assets/{filename}"),
                    None => format!("/assets/{name}.js"),
                };
                pins.push(ImportmapPin { name, path });
            }
            "pin_all_from" => {
                // `pin_all_from "dir", under: "ns"` — walk dir and
                // add a pin per *.js file. Name is `ns/basename`;
                // path is `/assets/ns/basename.js`.
                let Some(dir_arg) = args.first() else {
                    continue;
                };
                let Some(dir_str) = string_literal_value(dir_arg) else {
                    continue;
                };
                let under = args
                    .iter()
                    .skip(1)
                    .find_map(|a| extract_kwarg_str(a, "under"));
                let walk_dir = app_dir.join(&dir_str);
                if !vfs.is_dir(&walk_dir) {
                    continue;
                }
                let mut entries: Vec<PathBuf> = vfs
                    .read_dir(&walk_dir)?
                    .into_iter()
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("js"))
                    .collect();
                entries.sort();
                for entry in entries {
                    let stem = entry.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    if stem.is_empty() {
                        continue;
                    }
                    // Rails' importmap-rails pins `index.js` as the
                    // namespace itself (`"controllers"` not
                    // `"controllers/index"`) — matches JS module
                    // resolution where `import "controllers"`
                    // resolves to the directory's index.
                    let name = match (&under, stem) {
                        (Some(ns), "index") => ns.clone(),
                        (Some(ns), _) => format!("{ns}/{stem}"),
                        (None, _) => stem.to_string(),
                    };
                    let path = match &under {
                        Some(ns) => format!("/assets/{ns}/{stem}.js"),
                        None => format!("/assets/{stem}.js"),
                    };
                    pins.push(ImportmapPin { name, path });
                }
            }
            _ => {}
        }
    }
    Ok(Importmap { pins })
}

fn string_literal_value(node: &Node<'_>) -> Option<String> {
    let s = node.as_string_node()?;
    Some(String::from_utf8_lossy(s.unescaped()).into_owned())
}

fn extract_kwarg_str(arg: &Node<'_>, key: &str) -> Option<String> {
    let hash = arg.as_keyword_hash_node()?;
    for element in hash.elements().iter() {
        let Some(pair) = element.as_assoc_node() else {
            continue;
        };
        let k = pair.key();
        let k_node = k.as_symbol_node()?;
        let k_str = String::from_utf8_lossy(k_node.unescaped()).into_owned();
        if k_str != key {
            continue;
        }
        return string_literal_value(&pair.value());
    }
    None
}

fn read_yml_files<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = vfs
        .read_dir(dir)?
        .into_iter()
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("yml") | Some("yaml")))
        .collect();
    out.sort();
    Ok(out)
}

fn read_erb_files<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk_erb(vfs, dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_erb<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    out: &mut Vec<PathBuf>,
) -> IngestResult<()> {
    for path in vfs.read_dir(dir)? {
        if vfs.is_dir(&path) {
            walk_erb(vfs, &path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("erb") {
            // Only HTML templates — `.html.erb`. Mailer plain-text
            // templates (`.text.erb`) aren't part of the scaffold
            // render path and would collide on emit (their stems
            // strip to the same name as the HTML template).
            let path_str = path.to_string_lossy();
            if path_str.ends_with(".html.erb") {
                out.push(path);
            }
        }
    }
    Ok(())
}

fn read_rb_files<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = vfs
        .read_dir(dir)?
        .into_iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("rb"))
        .collect();
    out.sort();
    Ok(out)
}
