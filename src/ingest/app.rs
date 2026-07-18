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

use ruby_prism::Node;

use crate::App;
use crate::vfs::{FsVfs, MapVfs, Vfs};

use super::controller::ingest_controller;
use super::expr::ingest_ruby_program;
use super::fixture::ingest_fixture_file;
use super::jbuilder::ingest_jbuilder;
use super::library_class::{
    ClassKind, classify_class_file, ingest_concern_filters, ingest_concern_model_items,
    ingest_library_classes, ingest_rails_application_singleton_methods,
};
use super::model::ingest_model;
use super::routes::ingest_routes_with_draws;
use super::schema::{ingest_migration, ingest_schema};
use super::test::ingest_test_file;
use super::view::{ViewEngine, ingest_template};
use super::survey::{self, unwrap_or_record};
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
    super::sources::reset();
    let mut app = App::new();

    let schema_path = dir.join("db/schema.rb");
    if vfs.exists(&schema_path) {
        let source = vfs.read(&schema_path)?;
        if let Some(schema) =
            unwrap_or_record(ingest_schema(&source, &schema_path.display().to_string()))?
        {
            app.schema = schema;
        }
    } else {
        // No schema.rb (never migrated locally, gitignored, or a
        // migrations-only app) — recover the same column facts by
        // folding db/migrate/*.rb in filename order (timestamp
        // prefixes sort chronologically). schema.rb stays canonical
        // when both exist: it's the already-folded form.
        let migrate_dir = dir.join("db/migrate");
        if vfs.is_dir(&migrate_dir) {
            let mut schema = crate::schema::Schema::default();
            for entry in read_rb_files(vfs, &migrate_dir)? {
                let source = vfs.read(&entry)?;
                unwrap_or_record(ingest_migration(
                    &source,
                    &entry.display().to_string(),
                    &mut schema,
                ))?;
            }
            app.schema = schema;
        }
    }

    let models_dir = dir.join("app/models");
    if vfs.is_dir(&models_dir) {
        for entry in read_rb_files(vfs, &models_dir)? {
            let source = vfs.read(&entry)?;
            let path_str = entry.display().to_string();
            match classify_class_file(&source) {
                Some(ClassKind::Model) | None => {
                    if let Some(maybe_model) =
                        unwrap_or_record(ingest_model(&source, &path_str, &app.schema))?
                    {
                        if let Some(model) = maybe_model {
                            app.models.push(model);
                        }
                    }
                }
                Some(ClassKind::LibraryClass) => {
                    // Plural ingest so a bare `module Foo` under
                    // app/models/ (e.g. InactiveUser — a namespace of
                    // `def self.x`) registers as a library class, not
                    // just PORO classes. The singular path uses
                    // find_first_class and would drop a module.
                    if let Some(classes) =
                        unwrap_or_record(ingest_library_classes(&source, &path_str))?
                    {
                        app.library_classes.extend(classes);
                        // Concern modules (app/models/concerns/…) also
                        // carry `included do` declarations that belong
                        // to every includer: filters (controller-side)
                        // and model DSL (associations/scopes).
                        app.concern_filters
                            .extend(ingest_concern_filters(&source, &path_str));
                        app.concern_model_items
                            .extend(ingest_concern_model_items(&source, &path_str));
                    }
                }
            }
        }
    }

    // Vendored / support classes under extras/ and lib/ (Markdowner,
    // Sponge, Utils, monkey-patches, …) plus helper modules under
    // app/helpers/ and mailers under app/mailers/. Ingest each as a
    // library class so dotted calls like `Markdowner.to_html`,
    // `TrafficHelper.novelty_logo`, or `PasswordReset.password_reset_link`
    // resolve instead of dispatching to "no known method". Helpers are
    // conventionally mixed into views as instance methods
    // (`include`-resolution into a view's self-type is a separate gap),
    // but the ones called as bare singletons declare `def self.x` /
    // `module_function`, which `ingest_library_classes` records as class
    // methods — exactly the call surface we need here. Mailers declare
    // their actions as plain instance `def`s but are *invoked* on the
    // class (`Mailer.action(...).deliver_now`); analyze re-exposes those
    // as class methods (see `with_adapter`'s mailer pass), using the
    // `ActionMailer::Base` parent link captured here.
    // extras/lib are the least Rails-conventional files in the tree (HTTP
    // clients, monkey-patches, refinements), so isolate per file: a parse or
    // unsupported-construct failure degrades that one file to "class not
    // registered" (references stay unknown, same as before) rather than
    // aborting the whole app ingest. We never propagate; in survey mode the
    // error is still recorded for scope estimation.
    // `app/lib` is Rails-autoloaded app code (Mastodon keeps ~100
    // service/lib classes there — ActivityPub::TagManager etc.);
    // without it every `SomeService.instance.method` chain dispatches
    // into nothing. The service-object layer (services/workers/
    // serializers/policies/validators/presenters) is the same deal at
    // larger scale: Mastodon keeps ~450 plain-Ruby classes across those
    // six dirs, and every `FooService.new.call(…)` in a controller
    // dispatches into nothing until they register.
    // Rails loads lib/ subtrees per the app's declared
    // `config.autoload_lib(ignore: %w[...])` list — lobsters ignores
    // assets/custom_cops/tasks (the custom_cops are RuboCop cop classes
    // subclassing an unmodeled dev gem, never loaded at app runtime).
    // Honor the ignore list when walking lib/ so dev-tooling classes
    // don't register as app library classes (and don't end up in the
    // `app/models.rb` aggregator's eager-load set).
    let lib_ignores: Vec<String> = vfs
        .read(&dir.join("config/application.rb"))
        .ok()
        .map(|s| extract_autoload_lib_ignores(&s))
        .unwrap_or_default();
    for sub in [
        "extras",
        "lib",
        "app/lib",
        "app/jobs",
        "app/mailers",
        "app/services",
        "app/workers",
        "app/serializers",
        "app/policies",
        "app/validators",
        "app/presenters",
    ] {
        let support_dir = dir.join(sub);
        if !vfs.is_dir(&support_dir) {
            continue;
        }
        let Ok(entries) = read_rb_files(vfs, &support_dir) else { continue };
        for entry in entries {
            if sub == "lib"
                && entry.strip_prefix(&support_dir).is_ok_and(|rel| {
                    rel.components().next().is_some_and(|c| {
                        lib_ignores.iter().any(|ig| c.as_os_str() == ig.as_str())
                    })
                })
            {
                continue;
            }
            let Ok(source) = vfs.read(&entry) else { continue };
            let path_str = entry.display().to_string();
            match ingest_library_classes(&source, &path_str) {
                Ok(classes) => app.library_classes.extend(classes),
                Err(err) => {
                    if survey::is_active() {
                        survey::record(&err);
                    }
                }
            }
        }
    }

    // `app/helpers/*.rb` — ingested as library classes like the support
    // dirs above, but ALSO registered in `helper_method_index` so the
    // ruby emit-path helper-lowering pass can resolve a bare `avatar_img(…)`
    // in a template to `ApplicationHelper.avatar_img(…)`. Rails mixes every
    // helper module into every view, so the index is the flat union of all
    // helper method names → their defining module (last-writer-wins, as
    // Rails' include order would resolve). Empty-module helpers (the blog's
    // `module ApplicationHelper; end`) contribute nothing, keeping the
    // registry — and every downstream consumer — a no-op for them.
    let helpers_dir = dir.join("app/helpers");
    if vfs.is_dir(&helpers_dir) {
        if let Ok(entries) = read_rb_files(vfs, &helpers_dir) {
            for entry in entries {
                let Ok(source) = vfs.read(&entry) else { continue };
                let path_str = entry.display().to_string();
                match ingest_library_classes(&source, &path_str) {
                    Ok(classes) => {
                        for lc in &classes {
                            // Rails resolves a helper's `include`d
                            // modules into the same view surface —
                            // lobsters' ApplicationHelper includes
                            // TimeAgoInWords (lib/), whose
                            // time_ago_in_words SHADOWS the framework
                            // helper of the same name. Register the
                            // included module's methods under ITS id
                            // (the registry consult precedes the
                            // framework fallback in
                            // rewrite_helper_calls, preserving Rails'
                            // shadowing; index membership also puts
                            // the module in apply_helper_lowering's
                            // instance→module-function flip set).
                            // Include-target methods first so the
                            // helper's own defs win their names.
                            // lib/extras are ingested before helpers,
                            // so targets are already registered.
                            for inc in &lc.includes {
                                let Some(target) = app
                                    .library_classes
                                    .iter()
                                    .find(|c| c.name == *inc)
                                else {
                                    continue;
                                };
                                for m in &target.methods {
                                    app.helper_method_index
                                        .insert(m.name.clone(), target.name.clone());
                                }
                            }
                            for m in &lc.methods {
                                app.helper_method_index
                                    .insert(m.name.clone(), lc.name.clone());
                            }
                        }
                        app.library_classes.extend(classes);
                    }
                    Err(err) => {
                        if survey::is_active() {
                            survey::record(&err);
                        }
                    }
                }
            }
        }
    }

    // `config/application.rb` — the app's `Rails::Application` subclass
    // (`class Application < Rails::Application` inside the app module).
    // Its instance methods are app config (`read_only?`, `name`,
    // `domain`) reached at runtime as `Rails.application.<m>`. Reparent
    // onto `Rails::Application` itself: the runtime shim memoizes
    // `Rails::Application.new`, so a reopen makes the methods reachable
    // regardless of require order, and the app namespace (never
    // referenced at runtime) drops out. Same isolate-per-file tolerance
    // as extras/lib — the file carries Bundler/railtie noise that must
    // not abort ingest.
    let app_config_path = dir.join("config/application.rb");
    if let Ok(source) = vfs.read(&app_config_path) {
        let file = app_config_path.display().to_string();
        // Two capture points: methods in the Application class body, and
        // the "site-wide settings" idiom — a top-level
        // `class << Rails.application ... end` block whose defs are the
        // real config surface (lobsters keeps read_only?/name/domain
        // there, outside the class body).
        let class_methods = match ingest_library_classes(&source, &file) {
            Ok(classes) => classes
                .into_iter()
                .find(|lc| {
                    lc.parent
                        .as_ref()
                        .map(|p| p.0.as_str() == "Rails::Application")
                        .unwrap_or(false)
                })
                .map(|lc| lc.methods)
                .unwrap_or_default(),
            Err(err) => {
                if survey::is_active() {
                    survey::record(&err);
                }
                Vec::new()
            }
        };
        let singleton_methods =
            match ingest_rails_application_singleton_methods(&source, &file) {
                Ok(methods) => methods,
                Err(err) => {
                    if survey::is_active() {
                        survey::record(&err);
                    }
                    Vec::new()
                }
            };
        let mut methods = class_methods;
        methods.extend(singleton_methods);
        // `config.time_zone = "..."` — the one config-DSL assignment
        // the render layer is required to honor: Rails presents every
        // AR temporal value in this zone (lobsters runs Central).
        // Synthesized as a `config_time_zone` method on the
        // Application reopen; the CRuby overlay maps it to an IANA TZ
        // at boot (main.rb pins ENV["TZ"]). Every other config.* line
        // remains railtie noise ingest deliberately does not model.
        if let Some(zone) = extract_config_time_zone(&source) {
            if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                "def config_time_zone\n  {zone:?}\nend\n"
            )) {
                methods.append(&mut synth);
            }
        }
        if !methods.is_empty() {
            app.rails_application = Some(crate::dialect::LibraryClass {
                name: crate::ident::ClassId(crate::ident::Symbol::from("Rails::Application")),
                is_module: false,
                parent: None,
                includes: Vec::new(),
                methods,
                origin: None,
                constants: Vec::new(),
            });
        }
    }

    let controllers_dir = dir.join("app/controllers");
    if vfs.is_dir(&controllers_dir) {
        for entry in read_rb_files(vfs, &controllers_dir)? {
            let source = vfs.read(&entry)?;
            let path_str = entry.display().to_string();
            if let Some(maybe_controller) =
                unwrap_or_record(ingest_controller(&source, &path_str))?
            {
                if let Some(controller) = maybe_controller {
                    // `helper_method :x` exposes controller methods to
                    // templates. The ARG-PURE ones (no ivar reads)
                    // register like app-helper functions — the bare
                    // view call rewrites to `<Controller>.x(args)`
                    // against a class-side clone the controller
                    // lowering synthesizes. Registered before the
                    // app/helpers pass below, so a same-named helper-
                    // module function wins (its insert overwrites).
                    for name in crate::lower::controller_to_library::controller_helper_method_names(
                        &controller,
                    ) {
                        app.helper_method_index.insert(name, controller.name.clone());
                    }
                    app.controllers.push(controller);
                } else {
                    // No class in the file — a module: a concern under
                    // app/controllers/concerns/ (`AccountOwnedConcern`)
                    // or a mixin like `Authorization`. Ingest as a
                    // library class so its methods register and
                    // `include X` dispatch (ClassInfo.includes) can
                    // resolve into it, and capture its `included do`
                    // filter declarations for every includer's chain.
                    if let Some(classes) =
                        unwrap_or_record(ingest_library_classes(&source, &path_str))?
                    {
                        app.library_classes.extend(classes);
                        app.concern_filters
                            .extend(ingest_concern_filters(&source, &path_str));
                        app.concern_model_items
                            .extend(ingest_concern_model_items(&source, &path_str));
                    }
                }
            }
        }
    }

    let routes_path = dir.join("config/routes.rb");
    if vfs.exists(&routes_path) {
        let source = vfs.read(&routes_path)?;
        // `draw(:name)` split files — Rails loads
        // `config/routes/<name>.rb` into the same DSL context, and
        // Mastodon-class apps keep most of their route table there.
        let mut draw_files: HashMap<String, (Vec<u8>, String)> = HashMap::new();
        let routes_dir = dir.join("config/routes");
        if vfs.is_dir(&routes_dir) {
            for entry in read_rb_files(vfs, &routes_dir)? {
                let Some(stem) = entry.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let split_source = vfs.read(&entry)?;
                draw_files
                    .insert(stem.to_string(), (split_source, entry.display().to_string()));
            }
        }
        if let Some(routes) = unwrap_or_record(ingest_routes_with_draws(
            &source,
            &routes_path.display().to_string(),
            &draw_files,
        ))? {
            app.routes = routes;
        }
    }

    let views_dir = dir.join("app/views");
    if vfs.is_dir(&views_dir) {
        let erb_files = read_erb_files(vfs, &views_dir)?;
        for (erb_path, engine) in erb_files {
            let source = vfs.read_to_string(&erb_path)?;
            let rel = erb_path
                .strip_prefix(&views_dir)
                .map_err(|_| IngestError::Unsupported {
                    file: erb_path.display().to_string(),
                    message: "view path outside views dir".into(),
                })?;
            if let Some(view) = unwrap_or_record(ingest_template(
                &source,
                rel,
                &erb_path.display().to_string(),
                engine.compile_fn(),
            ))? {
                app.views.push(view);
            }
        }

        let jbuilder_files = read_jbuilder_files(vfs, &views_dir)?;
        for jb_path in jbuilder_files {
            let source = vfs.read_to_string(&jb_path)?;
            let rel = jb_path
                .strip_prefix(&views_dir)
                .map_err(|_| IngestError::Unsupported {
                    file: jb_path.display().to_string(),
                    message: "view path outside views dir".into(),
                })?;
            if let Some(view) = unwrap_or_record(ingest_jbuilder(
                &source,
                rel,
                &jb_path.display().to_string(),
            ))? {
                app.views.push(view);
            }
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
                if let Some(maybe_tm) =
                    unwrap_or_record(ingest_test_file(&source, &entry.display().to_string()))?
                {
                    if let Some(tm) = maybe_tm {
                        app.test_modules.push(tm);
                    }
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
            if let Some(fixture) = unwrap_or_record(ingest_fixture_file(&source, &entry))? {
                app.fixtures.push(fixture);
            }
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
        if let Some(expr) =
            unwrap_or_record(ingest_ruby_program(&source, &seeds_path.display().to_string()))?
        {
            app.seeds = Some(expr);
        }
    }

    // `config/importmap.rb` — tiny DSL of `pin` + `pin_all_from`
    // calls. Evaluated at ingest time to build an explicit
    // name→path list; `pin_all_from` expands by walking the
    // referenced directory. Feeds the emitted
    // `javascript_importmap_tags` helper.
    let importmap_path = dir.join("config/importmap.rb");
    if vfs.exists(&importmap_path) {
        let source = vfs.read_to_string(&importmap_path)?;
        if let Some(importmap) = unwrap_or_record(ingest_importmap(
            vfs,
            &source,
            dir,
            &importmap_path.display().to_string(),
        ))? {
            if !importmap.pins.is_empty() {
                app.importmap = Some(importmap);
            }
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
                let parsed = crate::rbs::parse_app_signatures(&source).map_err(|message| {
                    IngestError::Parse {
                        file: path_str.clone(),
                        message,
                    }
                });
                if let Some(sigs) = unwrap_or_record(parsed)? {
                    for (class_id, methods) in sigs {
                        app.rbs_signatures
                            .entry(class_id)
                            .or_default()
                            .extend(methods);
                    }
                }
            }
        }
    }

    app.sources = super::sources::drain();
    // Registered source paths are prefixed with this (the fs walk
    // joins `dir`); map-VFS trees pass `""` and register app-relative.
    app.root = dir.display().to_string().trim_end_matches('/').to_string();

    resolve_polymorphic_targets(&mut app);

    Ok(app)
}

/// Fill each `belongs_to …, polymorphic: true` association's target
/// set from the inverse side: every model declaring a `has_many`/
/// `has_one` with `as: <name>` implements that polymorphic interface
/// (the Rails-canonical registration). Runs once at app assembly so
/// the IR is self-describing — lowerers and the analyzer read the
/// resolved set instead of re-scanning the app. Models are collected
/// in ingest order (alphabetical fs walk), so the set is stable.
fn resolve_polymorphic_targets(app: &mut App) {
    use crate::dialect::{Association, ModelBodyItem};

    let mut implementors: HashMap<crate::ident::Symbol, Vec<crate::ident::ClassId>> =
        HashMap::new();
    for model in &app.models {
        for assoc in model.associations() {
            let (Association::HasMany { as_interface: Some(intf), .. }
            | Association::HasOne { as_interface: Some(intf), .. }) = assoc
            else {
                continue;
            };
            let entry = implementors.entry(intf.clone()).or_default();
            if !entry.contains(&model.name) {
                entry.push(model.name.clone());
            }
        }
    }
    for model in &mut app.models {
        // Secondary source, resolved before the mutable borrow: the
        // owner model's own body may name implementors as literals —
        // `where(item_type: "Moderation")` hash conditions or raw-SQL
        // joins (`item_type = 'Moderation'`). Rails apps without
        // inverse `as:` declarations (lobsters' ModActivity) register
        // the set this way.
        let literal_sets: Vec<(crate::ident::Symbol, Vec<crate::ident::ClassId>)> = model
            .associations()
            .filter_map(|assoc| match assoc {
                Association::BelongsTo { name, polymorphic: true, .. }
                    if !implementors.contains_key(name) =>
                {
                    let found = scan_type_literals(model, name);
                    (!found.is_empty()).then(|| (name.clone(), found))
                }
                _ => None,
            })
            .collect();
        for item in &mut model.body {
            let ModelBodyItem::Association { assoc, .. } = item else { continue };
            let Association::BelongsTo {
                name, polymorphic: true, polymorphic_targets, ..
            } = assoc
            else {
                continue;
            };
            if let Some(targets) = implementors.get(name) {
                *polymorphic_targets = targets.clone();
            } else if let Some((_, found)) =
                literal_sets.iter().find(|(n, _)| n == name)
            {
                *polymorphic_targets = found.clone();
            }
        }
    }
}

/// Scan a model's body expressions for literal mentions of
/// `<assoc>_type` paired with a class-name string: hash conditions
/// (`where(item_type: "Moderation")`) and raw-SQL fragments
/// (`… item_type = 'Moderation' …`). Returns the class names in
/// first-appearance order.
fn scan_type_literals(
    model: &crate::dialect::Model,
    assoc_name: &crate::ident::Symbol,
) -> Vec<crate::ident::ClassId> {
    use crate::dialect::{Association, ModelBodyItem};
    use crate::expr::{Expr, ExprNode, Literal};

    let type_col = format!("{assoc_name}_type");
    let mut found: Vec<crate::ident::ClassId> = Vec::new();
    let mut push = |s: &str| {
        // Class names only — reject anything that isn't a constant path.
        if !s.is_empty()
            && s.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
        {
            let id = crate::ident::ClassId(crate::ident::Symbol::from(s));
            if !found.contains(&id) {
                found.push(id);
            }
        }
    };

    fn walk(e: &Expr, f: &mut dyn FnMut(&Expr)) {
        f(e);
        e.node.for_each_child(&mut |c| walk(c, f));
    }
    let mut visit = |e: &Expr| {
        walk(e, &mut |e| {
            match &*e.node {
                // where(item_type: "Moderation") — hash entry keyed by
                // the type column with a string literal value.
                ExprNode::Hash { entries, .. } => {
                    for (k, v) in entries {
                        let key_matches = match &*k.node {
                            ExprNode::Lit { value: Literal::Sym { value } } => {
                                value.as_str() == type_col
                            }
                            ExprNode::Lit { value: Literal::Str { value } } => {
                                value == &type_col
                            }
                            _ => false,
                        };
                        if key_matches {
                            if let ExprNode::Lit { value: Literal::Str { value } } = &*v.node {
                                push(value);
                            }
                        }
                    }
                }
                // Raw SQL: every `<col> = '<Name>'` / `= "<Name>"`
                // occurrence inside one string literal.
                ExprNode::Lit { value: Literal::Str { value } } => {
                    let mut rest = value.as_str();
                    while let Some(pos) = rest.find(type_col.as_str()) {
                        rest = &rest[pos + type_col.len()..];
                        let tail = rest.trim_start();
                        let Some(tail) = tail.strip_prefix('=') else { continue };
                        let tail = tail.trim_start();
                        let Some(quote) = tail.chars().next().filter(|c| *c == '\'' || *c == '"')
                        else {
                            continue;
                        };
                        let inner = &tail[1..];
                        if let Some(end) = inner.find(quote) {
                            push(&inner[..end]);
                        }
                    }
                }
                _ => {}
            }
        });
    };

    for item in &model.body {
        match item {
            ModelBodyItem::Scope { scope, .. } => visit(&scope.body),
            ModelBodyItem::Method { method, .. } => visit(&method.body),
            ModelBodyItem::Unknown { expr, .. } => visit(expr),
            ModelBodyItem::Association { assoc, .. } => {
                if let Association::HasMany { scope: Some(s), .. } = assoc {
                    visit(s);
                }
            }
            _ => {}
        }
    }
    found
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
    super::sources::register(file, source);
    let result = super::prism::parse(source.as_bytes(), file);
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

fn read_erb_files<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
) -> IngestResult<Vec<(PathBuf, ViewEngine)>> {
    let mut out = Vec::new();
    walk_erb(vfs, dir, &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn walk_erb<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    out: &mut Vec<(PathBuf, ViewEngine)>,
) -> IngestResult<()> {
    for path in vfs.read_dir(dir)? {
        if vfs.is_dir(&path) {
            walk_erb(vfs, &path, out)?;
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str());
        match ext {
            // jbuilder is ingested by `walk_jbuilder`; leave it alone.
            Some("jbuilder") => {}
            // A supported text-template engine (ERB today; HAML/herb as
            // they land). Only HTML-format templates render through the
            // view path: mailer plain-text variants (`.text.erb` /
            // `.text.haml`) carry Ruby we don't type and would collide on
            // emit (their stems strip to the HTML template's name), so
            // surface them as a coverage gap rather than dropping silently.
            Some(e) if ViewEngine::from_extension(e).is_some() => {
                let engine = ViewEngine::from_extension(e).expect("checked is_some");
                // `.html.erb` renders through the view path. So does a
                // FORMAT-AGNOSTIC template (`rss.erb` — engine ext with
                // no inner format): Rails renders those for any request
                // format (lobsters' home/rss.erb backs its RSS feeds).
                // Explicit non-html formats (`.text.erb` mailer variants)
                // stay skipped: their stems collide with the html
                // template's on emit and their bodies aren't typed.
                let file_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let stem = file_name
                    .strip_suffix(&format!(".{e}"))
                    .unwrap_or(&file_name);
                if stem.ends_with(".html") || !stem.contains('.') {
                    out.push((path, engine));
                } else {
                    record_skipped_view(&path, &format!("{e} (non-html format)"));
                }
            }
            // Template engines we don't ingest yet — they hold Ruby (or are
            // pure Ruby, like `.json.ruby`) the analyzer never sees. Record
            // so the hole is visible to `--continue` and the LSP/MCP.
            // Moving one of these into `ViewEngine::from_extension` (above)
            // is the whole walker-side change to support a new engine.
            Some("slim" | "ruby" | "builder" | "rabl") => {
                record_skipped_view(&path, ext.expect("matched a Some arm"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Record an un-ingested view template as a survey gap. A no-op when
/// survey mode is off, so the strict/CI path is unchanged; under
/// `--continue` (and the LSP/MCP, which now ingest in survey mode) it
/// makes the HAML / `.text.erb` / `.ruby` coverage hole visible instead
/// of letting whole template files vanish without a trace.
fn record_skipped_view(path: &Path, engine: &str) {
    survey::record(&IngestError::Unsupported {
        file: path.display().to_string(),
        message: format!("view template not ingested: {engine}"),
    });
}

fn read_jbuilder_files<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk_jbuilder(vfs, dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_jbuilder<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    out: &mut Vec<PathBuf>,
) -> IngestResult<()> {
    for path in vfs.read_dir(dir)? {
        if vfs.is_dir(&path) {
            walk_jbuilder(vfs, &path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("jbuilder") {
            out.push(path);
        }
    }
    Ok(())
}

/// Every `.rb` file under `dir`, recursively, sorted for determinism.
/// Recursion matters on real apps: Rails autoloads nested directories
/// (`app/controllers/admin/…`, `app/models/concerns/…`), and a flat
/// listing silently ignored them — on Mastodon that dropped 306 of 337
/// controller files (admin/, api/, settings/, concerns/) with no gap
/// recorded anywhere. The textbook silent gap; never again.
fn read_rb_files<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Vec<PathBuf>> {
    fn collect<V: Vfs + ?Sized>(
        vfs: &V,
        dir: &Path,
        out: &mut Vec<PathBuf>,
    ) -> IngestResult<()> {
        for entry in vfs.read_dir(dir)? {
            if vfs.is_dir(&entry) {
                collect(vfs, &entry, out)?;
            } else if entry.extension().and_then(|e| e.to_str()) == Some("rb") {
                out.push(entry);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    collect(vfs, dir, &mut out)?;
    out.sort();
    Ok(out)
}

/// Extract the `ignore:` list from a `config.autoload_lib(ignore:
/// %w[assets tasks])` call in config/application.rb. Same textual
/// line-scan contract as `extract_config_time_zone` (railtie soup is
/// deliberately not parsed); commented lines don't match. Absent call
/// or unrecognized shape → empty list (walk everything, the prior
/// behavior).
fn extract_autoload_lib_ignores(source: &[u8]) -> Vec<String> {
    let source = String::from_utf8_lossy(source);
    for line in source.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        let Some(rest) = t.strip_prefix("config.autoload_lib") else {
            continue;
        };
        let Some(start) = rest.find("%w") else {
            return Vec::new();
        };
        let rest = &rest[start + 2..];
        let close = match rest.chars().next() {
            Some('[') => ']',
            Some('(') => ')',
            Some('{') => '}',
            _ => return Vec::new(),
        };
        let inner = &rest[1..];
        let Some(end) = inner.find(close) else {
            return Vec::new();
        };
        return inner[..end]
            .split_whitespace()
            .map(str::to_string)
            .collect();
    }
    Vec::new()
}

/// Extract the string value of a `config.time_zone = "..."` assignment
/// from config/application.rb. A textual line scan, not a parse: the
/// file is railtie soup ingest deliberately does not model, and this
/// one assignment is load-bearing for render parity (Rails presents
/// every ActiveRecord temporal value in this zone). Commented lines —
/// the `rails new` template ships `# config.time_zone = …` — don't
/// match.
fn extract_config_time_zone(source: &[u8]) -> Option<String> {
    let source = String::from_utf8_lossy(source);
    for line in source.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        let Some(rest) = t.strip_prefix("config.time_zone") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let quote = rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let inner = &rest[1..];
        if let Some(end) = inner.find(quote) {
            return Some(inner[..end].to_string());
        }
    }
    None
}
