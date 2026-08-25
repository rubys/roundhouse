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
use crate::Symbol;
use crate::dialect::{LibraryClass, MethodReceiver, TestModule};
use crate::vfs::{FsVfs, MapVfs, Vfs};

use super::controller::ingest_controller;
use super::expr::ingest_ruby_program;
use super::fixture::ingest_fixture_file;
use super::jbuilder::ingest_jbuilder;
use super::library_class::{
    ClassKind, classify_class_file, ingest_concern_class_method_names,
    ingest_concern_filters, ingest_concern_model_items, ingest_helper_method_names,
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
    // Front-end dispatch: a rack app with no config/routes.rb whose
    // app.rb subclasses Roda takes the Roda + Sequel walker (issue
    // #67); everything else is a Rails-convention tree.
    if super::roda_app::is_roda_app(vfs, dir) {
        return super::roda_app::ingest_roda_app_with_vfs(vfs, dir);
    }
    super::sources::reset();
    let mut app = App::new();
    // `enum` columns declared inside a concern's `included do`, keyed by
    // the module. Local rather than a field on `App`: they exist only
    // until the splice folds them into each including model's own
    // `enums` table, and nothing downstream reads them by module.
    let mut concern_enums: Vec<(
        crate::ident::ClassId,
        Vec<(crate::ident::Symbol, Vec<(String, crate::expr::Literal)>)>,
    )> = Vec::new();
    // Which of a concern's class-side methods came from its
    // `ClassMethods` carrier — the only ones an includer inherits. Local
    // for the same reason as `concern_enums`: read once, by the splice
    // that copies them onto each including model.
    let mut concern_class_method_names: Vec<(
        crate::ident::ClassId,
        Vec<crate::ident::Symbol>,
    )> = Vec::new();

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
    // A namespace's `table_name_prefix` has to be known BEFORE the model
    // it prefixes is ingested, and file order does not guarantee that
    // (`push/subscription.rb` may be read before `push.rb`). One cheap
    // pre-pass over the same files, so the fact is complete when the
    // models loop starts.
    let mut table_prefixes = super::model::TablePrefixes::new();
    if vfs.is_dir(&models_dir) {
        for entry in read_rb_files(vfs, &models_dir)? {
            let source = vfs.read(&entry)?;
            table_prefixes
                .extend(super::model::ingest_table_name_prefixes(&source, &entry.display().to_string()));
        }
    }
    if vfs.is_dir(&models_dir) {
        for entry in read_rb_files(vfs, &models_dir)? {
            let source = vfs.read(&entry)?;
            let path_str = entry.display().to_string();
            match classify_class_file(&source) {
                Some(ClassKind::Model) | None => {
                    if let Some(maybe_model) =
                        unwrap_or_record(ingest_model(&source, &path_str, &app.schema, &table_prefixes))?
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
                        let (concern_items, concern_enum_decls) =
                            ingest_concern_model_items(&source, &path_str);
                        app.concern_model_items.extend(concern_items);
                        concern_class_method_names
                            .extend(ingest_concern_class_method_names(&source));
                        app.view_visible_controller_methods
                            .extend(ingest_helper_method_names(&source));
                        concern_enums.extend(concern_enum_decls);
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
    // `config.autoload_lib(ignore: %w[…])` removes a directory from the
    // AUTOLOAD paths, which is NOT the same as removing it from the app.
    // campfire ignores `rails_ext` precisely BECAUSE it loads those
    // files itself, from an initializer — and dropping them lost
    // `String#all_emoji?`, which every message row calls. A subdir some
    // initializer explicitly requires is app code after all.
    let lib_ignores: Vec<String> = vfs
        .read(&dir.join("config/application.rb"))
        .ok()
        .map(|s| extract_autoload_lib_ignores(&s))
        .unwrap_or_default()
        .into_iter()
        .filter(|ignored| !lib_dir_is_explicitly_required(vfs, dir, ignored))
        .collect();
    for sub in [
        "extras",
        "lib",
        "app/lib",
        "app/jobs",
        // `app/channels` is here for the half of a channel that needs no
        // socket. `UnreadRoomsChannel.stream_name_for(user_id)` is a
        // plain class method, and the MODEL doing the broadcasting is
        // what calls it — so leaving channels uningested left a live
        // call site pointing at nothing. The subscription half
        // (`subscribed` / `stream_from`) is not dispatched yet; its
        // bases live in `runtime/action_cable.rb` so these files load.
        "app/channels",
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
                // Rails mixes in only the files whose NAME says helper:
                // `all_helpers_from_path` globs `**/*_helper.rb` and
                // nothing else. Everything else under app/helpers is
                // ordinary autoloaded app code that happens to live
                // there — campfire keeps `Messages::AttachmentPresentation`
                // (a PORO) and its `ContentFilters` classes here.
                //
                // Registering those cost twice over: the PORO's methods
                // entered the view surface, so a `render "messages/…"`
                // in a HELPER body bound to `AttachmentPresentation
                // .render` (arity 0, and not a partial render at all),
                // and index membership flattened the class into module
                // functions — `def self.initialize`, for a class whose
                // whole job is to hold two ivars.
                let is_helper_module = entry
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|stem| stem.ends_with("_helper"));
                match ingest_library_classes(&source, &path_str) {
                    Ok(classes) => {
                        for lc in classes.iter().filter(|_| is_helper_module) {
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
        // `config.session_store :cookie_store, key: "..."` — the second
        // config-DSL line the runtime is required to honor, and it lives
        // in `config/initializers/session_store.rb` rather than here
        // (Rails' own generator puts it there). The dispatch round-trips
        // the session under this cookie name, so it has to be known
        // before any app code runs; synthesized as `session_cookie_key`
        // on the Application reopen, overriding the framework default in
        // runtime/ruby/rails.rb. Apps that declare no session_store keep
        // that default. Every other initializer stays un-ingested.
        if let Ok(init) = vfs.read(&dir.join("config/initializers/session_store.rb")) {
            if let Some(key) = extract_session_cookie_key(&init) {
                if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                    "def session_cookie_key\n  {key:?}\nend\n"
                )) {
                    methods.append(&mut synth);
                }
            }
        }
        // App-defined config keys — `config.app_version = …` in
        // application.rb or an initializer, read back as
        // `Rails.application.config.app_version`. Rails' config object
        // takes arbitrary keys, so the assignment IS the definition;
        // each becomes a reader on this reopen and `lower::config_reader`
        // rewrites the reads. Framework keys are skipped: they either
        // already have a synthesized reader above (`time_zone`) or are
        // the railtie noise ingest deliberately does not model.
        {
            let mut sources: Vec<(String, Vec<u8>)> = vec![(
                dir.join("config/application.rb").display().to_string(),
                source.clone(),
            )];
            let init_dir = dir.join("config/initializers");
            if vfs.is_dir(&init_dir) {
                for entry in read_rb_files(vfs, &init_dir)? {
                    if let Ok(bytes) = vfs.read(&entry) {
                        sources.push((entry.display().to_string(), bytes));
                    }
                }
            }
            for (path, bytes) in sources {
                for (name, value) in extract_config_assignments(&bytes, &path) {
                    if FRAMEWORK_CONFIG_KEYS.contains(&name.as_str())
                        || methods.iter().any(|m| m.name.as_str() == name)
                    {
                        continue;
                    }
                    // The value rides verbatim: it is app code, and
                    // campfire's reads `ENV` — which nothing here can
                    // fold and nothing needs to.
                    if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                        "def {name}\n  {value}\nend\n"
                    )) {
                        methods.append(&mut synth);
                    }
                }
            }
        }
        if !methods.is_empty() {
            app.rails_application = Some(crate::dialect::LibraryClass {
                name: crate::ident::ClassId(crate::ident::Symbol::from("Rails::Application")),
                is_module: false,
                parent: None,
                includes: Vec::new(),
                methods,
                nullable_columns: Vec::new(),
                origin: None,
                constants: Vec::new(),
                unknown_calls: Vec::new(),
            });
        }
    }

    // `Time::DATE_FORMATS[:name] = ->(t) { … }` in an initializer —
    // read independently of config/application.rb above, since an app
    // can define a format without any of that file's config surface.
    // The lambda becomes a one-parameter method so its body arrives as
    // ordinary ingested IR; `lower::time_current` inlines it at each
    // `to_fs(:name)` site.
    {
        let init_dir = dir.join("config/initializers");
        if vfs.is_dir(&init_dir) {
            for entry in read_rb_files(vfs, &init_dir)? {
                let Ok(bytes) = vfs.read(&entry) else { continue };
                let path_str = entry.display().to_string();
                for (name, source) in extract_time_formats(&bytes, &path_str) {
                    let format = match source {
                        TimeFormatSource::Strftime(format) => {
                            crate::app::TimeFormat::Strftime { format }
                        }
                        TimeFormatSource::Lambda { param, body } => {
                            let Ok(methods) = crate::runtime_src::parse_methods(&format!(
                                "def __time_format_{name}({param})\n  {body}\nend\n"
                            )) else {
                                continue;
                            };
                            let Some(method) = methods.into_iter().next() else { continue };
                            crate::app::TimeFormat::Lambda { method }
                        }
                    };
                    app.time_formats
                        .insert(crate::ident::Symbol::from(name.as_str()), format);
                }
            }
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
                    // `helper_method :platform` written directly in a
                    // controller class body — the concern spelling is
                    // picked up at the module branch below.
                    app.view_visible_controller_methods
                        .extend(ingest_helper_method_names(&source));
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
                        let (concern_items, concern_enum_decls) =
                            ingest_concern_model_items(&source, &path_str);
                        app.concern_model_items.extend(concern_items);
                        concern_class_method_names
                            .extend(ingest_concern_class_method_names(&source));
                        app.view_visible_controller_methods
                            .extend(ingest_helper_method_names(&source));
                        concern_enums.extend(concern_enum_decls);
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

    // Shared test-support modules — `test/test_helpers/*.rb`, mixed
    // into every test case by the app's own `test/test_helper.rb`
    // (`include SessionTestHelper, MentionTestHelper, TurboTestHelper`).
    // Read BEFORE the test files so the splice below has them.
    //
    // Spliced rather than `include`d, the same call the model side made
    // (`splice_concerns_into_models`): a test class's `helpers` already
    // lower to ordinary instance methods on it, which is exactly what
    // the mixin means, and it needs nothing from a target's mixin
    // semantics.
    let shared_test_helpers = ingest_test_helper_modules(vfs, dir)?;

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
                    if let Some(mut tm) = maybe_tm {
                        splice_test_helpers(&mut tm, &shared_test_helpers);
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
            // ERB tags are lifted out and carried as expressions rather
            // than dropped — see `ingest::fixture`. A file whose ERB we
            // genuinely can't ingest still records a ledger line and is
            // skipped, via `unwrap_or_record`.
            if let Some(fixture) =
                unwrap_or_record(ingest_fixture_file(&source, &entry, &fixtures_dir))?
            {
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

    // Façade typing contracts, merged the same way an app's own `sig/`
    // sidecars are. A façade's `.rbs` is written to describe the REAL
    // shapes consumers chain on, and until now it was applied only at
    // emit time — after inference had already run — so analysis never
    // saw it. That cost lobsters two C errors ten links downstream:
    // `OpenSSL::Random.random_bytes` typed untyped, which widened `str`
    // in `Utils.random_str`, which made `CandidateId#to_s` untyped,
    // which put a `def to_s: () -> untyped` in the program — and one of
    // those widens every poly `.to_s` (matz/spinel#4090).
    //
    // Signatures merge for EVERY target, not just the strict ones that
    // swap the bodies. The contract describes the true API (`to_html`
    // really does answer a String; `random_bytes` really does answer a
    // String), so it is correct where the real gem runs too — and
    // target-conditional inference would let the CRuby and spinel lanes
    // disagree about types, which is what dual-runtime parity forbids.
    for (class_id, methods) in crate::facades::signatures_for(&app) {
        app.rbs_signatures
            .entry(class_id)
            .or_default()
            .extend(methods);
    }

    app.sources = super::sources::drain();
    // Registered source paths are prefixed with this (the fs walk
    // joins `dir`); map-VFS trees pass `""` and register app-relative.
    app.root = dir.display().to_string().trim_end_matches('/').to_string();

    resolve_polymorphic_targets(&mut app);
    // Before the splice: it (and every later consumer) looks concerns up
    // by ClassId, so the lexical-scope resolution has to have happened.
    qualify_relative_model_includes(&mut app);
    // Before the concern splices: they read `library_classes`, and this
    // turns `Current`'s metaprogrammed surface into real methods first.
    super::current_attributes::lower_current_attributes(&mut app);
    // After it, not before: `Current`'s own `delegate` reads an
    // ATTRIBUTE's ivar, which that pass has the declarations for. What
    // reaches here is the general shape, whose target is a method.
    super::delegate::lower_delegates(&mut app);
    splice_concerns_into_models(&mut app);
    splice_concern_class_methods_into_models(&mut app, &concern_class_method_names);
    // After the splice, so a class method a concern contributed gets
    // the same treatment as one written in the model.
    qualify_model_class_method_ar_calls(&mut app);
    splice_concerns_into_controllers(&mut app);
    // After the splice: a macro has to resolve against the concern's
    // class-side methods, and its expansion joins the same filter chain.
    expand_class_body_macros(&mut app);
    fold_concern_enums_into_models(&mut app, &concern_enums);
    // Last: needs every model's complete `enums` table, including the
    // columns an included concern declared.
    map_enum_labels(&mut app);
    // Last of all: `has_rich_text` can arrive through a concern's
    // `included do`, so the declaration scan has to run after the
    // splices — and `ActionText::RichText` has to be in `app.models`
    // before anything downstream enumerates models.
    crate::lower::rich_text::synthesize_record_model(&mut app);

    collect_binary_assets(vfs, dir, &mut app);

    Ok(app)
}

/// Source subtrees whose binary files are copied into the emitted tree.
///
/// Scoped rather than whole-tree: a Rails checkout also contains
/// `node_modules`, `.git` and `vendor`, none of which an emitted app
/// needs, and walking them would cost more than everything else the
/// ingester does. These three are where an app keeps files it reads at
/// RUN time — images and fonts it serves, and the fixture files its
/// tests open.
const BINARY_ASSET_ROOTS: [&str; 3] = ["app/assets", "public", "test/fixtures/files"];

/// Gather files the text pipeline cannot carry.
///
/// The rule is exactly "not valid UTF-8", which is the same condition
/// that makes a file unrepresentable as an `EmittedFile` (its `content`
/// is a `String`). Text assets under these roots already reach the emit
/// through the normal emitters — `public/404.html` and
/// `app/assets/tailwind.css` both do — so restricting to binary avoids
/// emitting a second copy and keeps the rule one sentence long.
///
/// A TEXT asset under these roots that no emitter produces would still
/// be dropped. That is a different gap and is deliberately not widened
/// into here: this closes the one where the pipeline is structurally
/// incapable, not the one where an emitter simply has no rule yet.
fn collect_binary_assets<V: Vfs + ?Sized>(vfs: &V, dir: &Path, app: &mut App) {
    for root in BINARY_ASSET_ROOTS {
        let start = dir.join(root);
        if vfs.is_dir(&start) {
            walk_binary_assets(vfs, dir, &start, app);
        }
    }
    // Deterministic order: the emit is byte-compared across runs, and
    // `read_dir` order is explicitly unspecified by the trait.
    app.binary_assets.sort_by(|a, b| a.0.cmp(&b.0));
}

fn walk_binary_assets<V: Vfs + ?Sized>(vfs: &V, root: &Path, dir: &Path, app: &mut App) {
    let Ok(entries) = vfs.read_dir(dir) else { return };
    for entry in entries {
        if vfs.is_dir(&entry) {
            walk_binary_assets(vfs, root, &entry, app);
            continue;
        }
        // Valid UTF-8 means an emitter can carry it; only what cannot be
        // a `String` is our business here.
        if vfs.read_to_string(&entry).is_ok() {
            continue;
        }
        let Ok(bytes) = vfs.read(&entry) else { continue };
        let rel = entry.strip_prefix(root).unwrap_or(&entry);
        app.binary_assets
            .push((rel.to_string_lossy().replace('\\', "/"), bytes));
    }
}

/// Splice each concern's `included do` DSL items (already parsed into
/// `App::concern_model_items` — validations, callbacks, associations,
/// scopes, block-form lifecycle callbacks) into every model whose body
/// has the matching `include <Concern>` line, right AFTER that line —
/// so the model lowerer emits them exactly like the model's own
/// declarations (lobsters' Token concern: `after_initialize` token
/// generation + `validates :token`).
///
/// The `include` line itself is KEPT: the ruby-family emit re-emits it
/// verbatim and emits the concern module as a real file, so Ruby's own
/// include provides the module's constants (`User::VALID_USERNAME`)
/// and instance methods at runtime — only the `included do` block is
/// inert there (the emitted module has no ActiveSupport::Concern, so
/// its DSL never ran; that's exactly the half this splice supplies).
/// Strict targets get the DSL items the same way; module
/// methods-via-include remain their separate, ledger-visible gap.
fn splice_concerns_into_models(app: &mut App) {
    use crate::dialect::ModelBodyItem;
    use crate::expr::ExprNode;

    for model in &mut app.models {
        let mut i = 0;
        while i < model.body.len() {
            // `include Attachment, Broadcasts, Mentionee` is one
            // statement mixing in three modules, and Rails runs their
            // `included do` blocks left to right — so collect every
            // arg's items in that order and splice them as one run.
            // Matching only single-arg includes skipped campfire's
            // models entirely: every one of them writes the list form.
            let concern_ids: Vec<crate::ident::ClassId> = match &model.body[i] {
                ModelBodyItem::Unknown { expr, .. } => match &*expr.node {
                    ExprNode::Send { recv: None, method, args, block: None, .. }
                        if method.as_str() == "include" =>
                    {
                        args.iter()
                            .filter_map(|arg| match &*arg.node {
                                ExprNode::Const { path } => {
                                    Some(crate::ident::ClassId(crate::ident::Symbol::from(
                                        path.iter()
                                            .map(|s| s.as_str())
                                            .collect::<Vec<_>>()
                                            .join("::"),
                                    )))
                                }
                                _ => None,
                            })
                            .collect()
                    }
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            };
            let model_name = model.name.clone();
            let items: Vec<ModelBodyItem> = concern_ids
                .iter()
                .filter_map(|id| app.concern_model_items.get(id).map(|items| (id, items)))
                .flat_map(|(id, items)| {
                    items.iter().map(|item| rehome_default_fk(item, id, &model_name))
                })
                .collect();
            if items.is_empty() {
                i += 1;
                continue;
            }
            let n = items.len();
            model.body.splice(i + 1..i + 1, items);
            i += n + 1;
        }
    }
}

/// An owner-derived FOREIGN KEY, recomputed for the model the
/// association is being spliced into.
///
/// `has_one :webhook` inside `User::Bot`'s `included do` defaults its
/// key from the declaring scope — and the declaring scope at ingest is
/// the CONCERN, so the key came out `user::bot_id`. Rails derives it
/// from the class the association ends up on, which is the includer:
/// `user_id`. The emitted query said `WHERE webhooks.user::bot_id = 5`
/// and sqlite answered "unrecognized token".
///
/// Only a key that still EQUALS the concern-derived default is moved.
/// An explicit `foreign_key:` differs from it and is left exactly as
/// written; if it happens to coincide, the two names are equal anyway.
/// `belongs_to` is untouched — its key derives from the TARGET, which
/// the splice does not change.
fn rehome_default_fk(
    item: &crate::dialect::ModelBodyItem,
    concern: &crate::ident::ClassId,
    model: &crate::ident::ClassId,
) -> crate::dialect::ModelBodyItem {
    use crate::dialect::{Association, ModelBodyItem};
    let mut out = item.clone();
    let ModelBodyItem::Association { assoc, .. } = &mut out else { return out };
    let concern_default =
        crate::ident::Symbol::from(format!("{}_id", crate::naming::snake_case(concern.0.as_str())));
    let model_default =
        crate::ident::Symbol::from(format!("{}_id", crate::naming::snake_case(model.0.as_str())));
    match assoc {
        Association::HasMany { foreign_key, .. } | Association::HasOne { foreign_key, .. } => {
            if *foreign_key == concern_default {
                *foreign_key = model_default;
            }
        }
        // `belongs_to`'s key derives from the TARGET, which the splice
        // does not change.
        _ => {}
    }
    out
}

/// Copy a model concern's CLASS-side methods onto every model that
/// includes it.
///
/// `include` never carries them. A concern writes its class side as
/// `class_methods do` / `module ClassMethods`, both of which
/// `ingest_library_classes` flattens into the module as `def self.…` —
/// and Ruby's `include` brings instance methods across, never singleton
/// ones (`C.respond_to?(:x)` is false; verified). Rails only gets away
/// with it because ActiveSupport::Concern's `append_features` runs
/// `base.extend ClassMethods`, and the emitted modules have no Concern.
///
/// So `Message.create_with_attachment!` — campfire's entire message
/// POST — resolved in analyze (the registry fold already copies the
/// class side onto includers) and NoMethodError'd at runtime. Analyze
/// agreeing with Rails while the emit disagreed is what kept it hidden.
///
/// COPY rather than emit `extend Message::Attachment::ClassMethods`:
/// the same call the model side already made for `included do` items and
/// the controller side made for filters, and for the same reason — a
/// mixin is a Ruby-family-only mechanism, and this lands once in the IR
/// for all thirteen targets.
///
/// Precedence follows Ruby's ancestor order for the LIST form campfire
/// writes (`include Attachment, Broadcasts, Mentionee`), where
/// `Module#include` inserts left to right so the EARLIER argument wins;
/// the model's own definition beats every concern. Transitive, so a
/// concern that includes another concern contributes both.
///
/// The module keeps its copy: it still emits as a real file, and the
/// copy is unreachable there rather than wrong (nothing calls
/// `Message::Attachment.create_with_attachment!`). Removing it would
/// mean rewriting library-class emit for no behavioural gain.
fn splice_concern_class_methods_into_models(
    app: &mut App,
    carriers: &[(crate::ident::ClassId, Vec<crate::ident::Symbol>)],
) {
    use crate::dialect::{MethodReceiver, ModelBodyItem};
    use crate::ident::{ClassId, Symbol};
    use std::collections::{HashMap, HashSet};

    // Class side + own constant names, per module. The constants come
    // along because a lifted body's bare `THUMBNAIL_MAX_WIDTH` resolves
    // against the module it was written in and would resolve against the
    // MODEL once moved — the same lexical trap the controller splice
    // hit with lobsters' `TIME_INTERVALS`.
    let carried: HashMap<&ClassId, HashSet<&Symbol>> = carriers
        .iter()
        .map(|(id, names)| (id, names.iter().collect()))
        .collect();
    if carried.is_empty() {
        return;
    }
    let mut class_side: HashMap<ClassId, (Vec<crate::dialect::MethodDef>, HashSet<Symbol>)> =
        HashMap::new();
    let mut module_includes: HashMap<ClassId, Vec<ClassId>> = HashMap::new();
    for lc in &app.library_classes {
        module_includes.insert(lc.name.clone(), lc.includes.clone());
        let Some(names) = carried.get(&lc.name) else { continue };
        let methods: Vec<crate::dialect::MethodDef> = lc
            .methods
            .iter()
            .filter(|m| m.receiver == MethodReceiver::Class && names.contains(&m.name))
            .cloned()
            .collect();
        if methods.is_empty() {
            continue;
        }
        let consts: HashSet<Symbol> = lc.constants.iter().map(|(n, _)| n.clone()).collect();
        class_side.insert(lc.name.clone(), (methods, consts));
    }
    if class_side.is_empty() {
        return;
    }

    for model in &mut app.models {
        // Transitive closure of the model's includes, in ancestor order.
        let mut order: Vec<ClassId> = Vec::new();
        let mut seen: HashSet<ClassId> = HashSet::new();
        let mut queue: Vec<ClassId> = crate::analyze::model_includes(model);
        while !queue.is_empty() {
            let id = queue.remove(0);
            if !seen.insert(id.clone()) {
                continue;
            }
            order.push(id.clone());
            if let Some(nested) = module_includes.get(&id) {
                queue.extend(nested.iter().cloned());
            }
        }
        if order.is_empty() {
            continue;
        }

        let mut taken: HashSet<Symbol> = model
            .body
            .iter()
            .filter_map(|i| match i {
                ModelBodyItem::Method { method, .. }
                    if method.receiver == MethodReceiver::Class =>
                {
                    Some(method.name.clone())
                }
                _ => None,
            })
            .collect();

        let mut added: Vec<ModelBodyItem> = Vec::new();
        for concern in &order {
            let Some((methods, consts)) = class_side.get(concern) else { continue };
            for m in methods {
                if !taken.insert(m.name.clone()) {
                    continue;
                }
                let mut m = m.clone();
                if !consts.is_empty() {
                    qualify_lexical_consts(&mut m.body, concern, consts);
                }
                added.push(ModelBodyItem::Method {
                    method: m,
                    leading_comments: Vec::new(),
                    leading_blank_line: true,
                });
            }
        }
        model.body.extend(added);
    }
}

/// Splice a controller concern's surface into every controller that
/// includes it: the `included do` filters join the filter chain, and the
/// module's instance methods become private methods of the controller.
///
/// Rails does this with `include` at class-definition time. Nothing in
/// the emitted trees can: the ruby-family targets would need Ruby's own
/// mixin semantics (which strict targets have no equivalent for), and
/// the filter chain is built at LOWERING time from `Controller::filters`
/// — a concern's filters were invisible to it. campfire's
/// ApplicationController is nothing BUT
/// `include AllowBrowser, Authentication, …`, so it emitted as an empty
/// class: no `before_action :require_authentication`, no
/// `restore_authentication` to call, every action running
/// unauthenticated.
///
/// Splicing (rather than emitting `include`) is the same choice the
/// model side already made, and for the same reason: it lands once, in
/// the IR, for all thirteen targets.
///
/// Closes transitively — `Authentication` includes `SessionLookup`, and
/// `find_session_by_cookie` has to arrive with it. A name the controller
/// (or an earlier concern) already defines wins, matching Ruby's
/// ancestor order.
///
/// A copied body carries its module's lexical scope with it, so a bare
/// constant reference is qualified on the way in: lobsters'
/// `IntervalHelper#time_interval` reads `TIME_INTERVALS`, which under
/// Ruby resolves against the module the `def` was written in and, once
/// spliced, resolves against the CONTROLLER — `uninitialized constant
/// HomeController::TIME_INTERVALS`, ten of the twenty-six benchmark
/// routes. The constant stays where it was defined (the module still
/// emits) and the reference becomes `IntervalHelper::TIME_INTERVALS`,
/// which is what Ruby's lexical lookup means and what every strict
/// target can resolve.
fn splice_concerns_into_controllers(app: &mut App) {
    use crate::dialect::{Action, ControllerBodyItem, MethodReceiver, RenderTarget};
    use crate::ty::{Row, Ty};

    // Instance methods per concern module, the constants those bodies
    // resolve lexically, and the modules it includes.
    let mut module_methods: HashMap<crate::ident::ClassId, Vec<crate::dialect::MethodDef>> =
        HashMap::new();
    let mut module_constants: HashMap<
        crate::ident::ClassId,
        std::collections::HashSet<crate::ident::Symbol>,
    > = HashMap::new();
    let mut module_includes: HashMap<crate::ident::ClassId, Vec<crate::ident::ClassId>> =
        HashMap::new();
    for lc in &app.library_classes {
        module_methods.insert(
            lc.name.clone(),
            lc.methods
                .iter()
                .filter(|m| matches!(m.receiver, MethodReceiver::Instance))
                .cloned()
                .collect(),
        );
        module_constants
            .insert(lc.name.clone(), lc.constants.iter().map(|(n, _)| n.clone()).collect());
        module_includes.insert(lc.name.clone(), lc.includes.clone());
    }

    for controller in &mut app.controllers {
        let includes = crate::analyze::controller_includes(controller);
        if includes.is_empty() {
            continue;
        }
        // Transitive closure, in include order.
        let mut queue = includes;
        let mut seen: std::collections::BTreeSet<crate::ident::ClassId> =
            queue.iter().cloned().collect();
        let mut qi = 0;
        while qi < queue.len() {
            let m = queue[qi].clone();
            qi += 1;
            for nested in module_includes.get(&m).into_iter().flatten() {
                if seen.insert(nested.clone()) {
                    queue.push(nested.clone());
                }
            }
        }

        let mut defined: std::collections::HashSet<crate::ident::Symbol> = controller
            .body
            .iter()
            .filter_map(|item| match item {
                ControllerBodyItem::Action { action, .. } => Some(action.name.clone()),
                _ => None,
            })
            .collect();

        let mut filters: Vec<ControllerBodyItem> = Vec::new();
        let mut methods: Vec<ControllerBodyItem> = Vec::new();
        for module in &queue {
            for filter in app.concern_filters.get(module).into_iter().flatten() {
                let mut filter = filter.clone();
                // Provenance for the chain view: `defined_in` is the
                // module, not the controller that included it.
                filter.from_concern = Some(module.clone());
                filters.push(ControllerBodyItem::Filter {
                    filter,
                    leading_comments: Vec::new(),
                    leading_blank_line: false,
                });
            }
            for method in module_methods.get(module).into_iter().flatten() {
                if !defined.insert(method.name.clone()) {
                    continue;
                }
                let mut body = method.body.clone();
                if let Some(consts) = module_constants.get(module) {
                    qualify_lexical_consts(&mut body, module, consts);
                }
                let mut params = Row::closed();
                let mut opt_params = Vec::new();
                for p in &method.params {
                    match &p.default {
                        Some(d) => opt_params.push((p.name.clone(), d.clone())),
                        None => {
                            params.fields.insert(p.name.clone(), Ty::Untyped);
                        }
                    }
                }
                methods.push(ControllerBodyItem::Action {
                    action: Action {
                        name: method.name.clone(),
                        params,
                        opt_params,
                        block_param: method.block_param.as_ref().map(|p| p.name.clone()),
                        body,
                        renders: RenderTarget::Inferred,
                        effects: crate::effect::EffectSet::pure(),
                    },
                    leading_comments: Vec::new(),
                    leading_blank_line: false,
                });
            }
        }
        if filters.is_empty() && methods.is_empty() {
            continue;
        }
        // Filters first (Rails runs an included filter ahead of the
        // includer's own), then the methods behind a private marker —
        // they're helpers, never routable actions.
        let has_private_marker = controller
            .body
            .iter()
            .any(|i| matches!(i, ControllerBodyItem::PrivateMarker { .. }));
        let mut body = std::mem::take(&mut controller.body);
        filters.extend(body.drain(..));
        if !methods.is_empty() && !has_private_marker {
            filters.push(ControllerBodyItem::PrivateMarker {
                leading_comments: Vec::new(),
                leading_blank_line: true,
            });
        }
        filters.extend(methods);
        controller.body = filters;
    }
}

/// Qualify the bare constant references in a body being lifted out of
/// `owner`'s lexical scope: `TIME_INTERVALS` -> `IntervalHelper::TIME_INTERVALS`.
///
/// Only names `owner` actually defines are touched, so a concern method
/// reading one of the INCLUDER's constants — or any global one — is left
/// alone to resolve where it always did. Already-qualified paths are
/// left alone too: a single segment is the only form whose meaning
/// changes when the `def` moves.
fn qualify_lexical_consts(
    expr: &mut crate::expr::Expr,
    owner: &crate::ident::ClassId,
    consts: &std::collections::HashSet<crate::ident::Symbol>,
) {
    use crate::expr::ExprNode;

    expr.node.for_each_child_mut(&mut |child| qualify_lexical_consts(child, owner, consts));
    if let ExprNode::Const { path } = &mut *expr.node {
        if let [segment] = &path[..] {
            if consts.contains(segment) {
                *path = owner
                    .0
                    .as_str()
                    .split("::")
                    .map(crate::ident::Symbol::from)
                    .chain(std::iter::once(segment.clone()))
                    .collect();
            }
        }
    }
}

/// Give a model's own class methods their implicit receiver.
///
/// `def self.banned?(ip) exists?(ip_address: ip) end` means
/// `Ban.exists?(…)` — inside a class method, self IS the model. Written
/// bare, the arel builder never sees it: its base arm needs a Const
/// receiver to resolve a table from, so the call fell through to the
/// runtime's `Base.exists?`, which takes an ID and got a Hash
/// (`Db.escape_int: undefined method 'to_i' for an instance of Hash`).
///
/// Naming the receiver here rather than teaching arel about an
/// enclosing class keeps the knowledge where it is certain — the walk
/// already knows which model owns the method — and pays off for every
/// consumer: the analyzer types the call through the model, and the
/// scope/relation machinery downstream sees the same shape a
/// `Model.where(…)` site has.
///
/// Only the base AR class methods, and only when the model does not
/// define that name itself: a model with its own `count` means its own.
///
/// The receiver it names is the model CONSTANT, which is exactly right
/// for a call on the class and loses one distinction Rails keeps: a
/// method reached through an association runs against the caller's
/// scope, so bare `count` means "this room's messages" where
/// `Message.count` means the whole table. The Ruby-family emit seam
/// re-roots those on the threaded relation
/// (`scope_chain::AssocClassMethods`), reading the model constant back
/// as the implicit self it stands for; the strict targets, which have
/// no relation to thread, keep the class-level reading.
fn qualify_model_class_method_ar_calls(app: &mut App) {
    use crate::dialect::{MethodReceiver, ModelBodyItem};
    use crate::expr::{Expr, ExprNode};

    /// The base-arm shapes `lower::arel::build` resolves from a Const
    /// receiver. `find`/`first`/`last` are deliberately absent: they
    /// take an id or no argument and already work receiverless through
    /// the runtime.
    const AR_CLASS_METHODS: &[&str] = &["all", "count", "where", "find_by", "exists?"];

    for model in &mut app.models {
        let own: std::collections::HashSet<String> = model
            .methods()
            .map(|m| m.name.as_str().to_string())
            .collect();
        let recv = Expr::new(
            crate::span::Span::synthetic(),
            ExprNode::Const { path: vec![model.name.0.clone()] },
        );
        let qualify = |expr: &mut Expr| {
            fn walk(
                expr: &mut Expr,
                recv: &Expr,
                own: &std::collections::HashSet<String>,
                ar: &[&str],
            ) {
                expr.node.for_each_child_mut(&mut |c| walk(c, recv, own, ar));
                let ExprNode::Send { recv: r @ None, method, .. } = &mut *expr.node else {
                    return;
                };
                if !ar.contains(&method.as_str()) || own.contains(method.as_str()) {
                    return;
                }
                *r = Some(recv.clone());
            }
            walk(expr, &recv, &own, AR_CLASS_METHODS);
        };
        for item in &mut model.body {
            let ModelBodyItem::Method { method, .. } = item else { continue };
            if !matches!(method.receiver, MethodReceiver::Class) {
                continue;
            }
            qualify(&mut method.body);
        }
    }
}

/// Run a controller's class-body macros at compile time.
///
/// A concern that exports a filter macro is ordinary modern Rails —
/// Rails 8 ships `allow_browser` that way, and campfire's Authentication
/// concern exports three (`allow_unauthenticated_access`,
/// `allow_bot_access`, `require_unauthenticated_access`). Each is a
/// `class_methods do` method whose whole body is filter DSL:
///
/// ```text
/// def self.allow_unauthenticated_access(options)
///   skip_before_action :require_authentication, options
/// end
/// ```
///
/// Rails runs that at class-definition time with literal arguments, so
/// the compiler can run it symbolically: bind the call's arguments to
/// the macro's parameters, substitute, and recognize what comes out as
/// the filters it is. `allow_unauthenticated_access only: %i[new create]`
/// folds to `skip_before_action :require_authentication, only: [:new,
/// :create]`, which every consumer of the chain already understands.
/// Left unexpanded, campfire's sign-in page demanded sign-in.
///
/// ALL-OR-NOTHING, and the reason is the failure direction, not
/// tidiness. Dropping the whole macro fails CLOSED — a page asks for
/// authentication it shouldn't. Expanding half of
/// `require_unauthenticated_access` — taking its `skip_before_action`
/// and losing the `before_action :restore_authentication,
/// :redirect_signed_in_user_to_root` behind it — fails OPEN. So a macro
/// whose body holds one statement this can't read stays Unknown, whole,
/// and is recorded as a gap.
fn expand_class_body_macros(app: &mut App) {
    use crate::dialect::{ControllerBodyItem, MethodReceiver};
    use crate::expr::ExprNode;

    // Class-side methods of every module, by name — the macro table.
    // Populated from library classes because that is where a concern's
    // `class_methods do` / `module ClassMethods` bodies land.
    let mut macros: HashMap<crate::ident::ClassId, Vec<crate::dialect::MethodDef>> = HashMap::new();
    for lc in &app.library_classes {
        let class_side: Vec<crate::dialect::MethodDef> = lc
            .methods
            .iter()
            .filter(|m| matches!(m.receiver, MethodReceiver::Class))
            .cloned()
            .collect();
        if !class_side.is_empty() {
            macros.insert(lc.name.clone(), class_side);
        }
    }
    if macros.is_empty() {
        return;
    }

    // Includes reachable from each controller, ITS ANCESTORS INCLUDED:
    // campfire's SessionsController includes nothing itself and calls a
    // macro its parent's Authentication concern exports, which is the
    // normal arrangement — the base controller mixes the concern in and
    // the subclasses use what it gave them.
    let mut reachable: HashMap<crate::ident::ClassId, Vec<crate::ident::ClassId>> = HashMap::new();
    for controller in &app.controllers {
        let mut acc: Vec<crate::ident::ClassId> = Vec::new();
        let mut cur = Some(controller);
        let mut seen: std::collections::BTreeSet<crate::ident::ClassId> =
            std::collections::BTreeSet::new();
        while let Some(c) = cur {
            if !seen.insert(c.name.clone()) {
                break;
            }
            for inc in crate::analyze::controller_includes(c) {
                if !acc.contains(&inc) {
                    acc.push(inc);
                }
            }
            cur = c
                .parent
                .as_ref()
                .and_then(|p| app.controllers.iter().find(|o| &o.name == p));
        }
        reachable.insert(controller.name.clone(), acc);
    }

    for controller in &mut app.controllers {
        let includes = reachable.get(&controller.name).cloned().unwrap_or_default();
        if includes.is_empty() {
            continue;
        }
        let mut expanded: Vec<ControllerBodyItem> = Vec::new();
        for item in std::mem::take(&mut controller.body) {
            let ControllerBodyItem::Unknown { expr, leading_comments, leading_blank_line } = &item
            else {
                expanded.push(item);
                continue;
            };
            let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
                expanded.push(item);
                continue;
            };
            // The macro has to come from a module this controller
            // includes; a same-named method elsewhere is not it.
            let found = includes.iter().find_map(|inc| {
                macros
                    .get(inc)
                    .and_then(|ms| ms.iter().find(|m| &m.name == method))
                    .map(|m| (inc.clone(), m.clone()))
            });
            let Some((module, macro_def)) = found else {
                expanded.push(item);
                continue;
            };
            let body = substitute_params(&macro_def, args);
            match filters_from_macro_body(&body, &module) {
                Some(filters) => {
                    let mut comments = leading_comments.clone();
                    let mut blank = *leading_blank_line;
                    for filter in filters {
                        expanded.push(ControllerBodyItem::Filter {
                            filter,
                            leading_comments: std::mem::take(&mut comments),
                            leading_blank_line: std::mem::take(&mut blank),
                        });
                    }
                }
                None => {
                    survey::record(&IngestError::Unsupported {
                        file: format!("{}", controller.name.0.as_str()),
                        message: format!(
                            "class-body macro not expanded: `{}` from {} holds a statement that is not filter DSL",
                            method.as_str(),
                            module.0.as_str()
                        ),
                    });
                    expanded.push(item);
                }
            }
        }
        controller.body = expanded;
    }
}

/// The macro's body with its parameters replaced by the call's
/// arguments. Positional binding, which is all these macros need: the
/// `**options` a concern macro forwards arrives here as a trailing
/// positional (see `ingest_hash_literal`), so the substitution is a
/// straight variable replacement.
///
/// A parameter the call site does NOT supply still has to bind, or the
/// body keeps a free variable and `filters_from_macro_body` rejects the
/// whole macro — which is how the bare `allow_unauthenticated_access`
/// (no arguments at all, campfire's FirstRunsController) silently kept
/// `require_authentication` and made `/first_run` redirect to
/// `/session/new`, which redirects back.
///
/// What an unsupplied parameter binds to is DERIVED, not guessed. In
/// valid Ruby a trailing parameter the caller may omit is exactly one of
/// three things, and the IR distinguishes all three:
///   * it declares a default        → bind the default
///   * `*rest` (`Param::rest`)      → bind an empty Array
///   * otherwise it can only be `**rest`, which ingest models as a plain
///     trailing positional          → bind an empty Hash
///
/// The empty Hash is what makes the bare call mean what Ruby means:
/// `skip_before_action :require_authentication, **{}` is an UNSCOPED
/// skip, so the filter comes off every action rather than none.
fn substitute_params(
    macro_def: &crate::dialect::MethodDef,
    args: &[crate::expr::Expr],
) -> crate::expr::Expr {
    use crate::expr::ExprNode;

    fn replace(expr: &mut crate::expr::Expr, bindings: &[(crate::ident::Symbol, crate::expr::Expr)]) {
        if let ExprNode::Var { name, .. } = &*expr.node {
            if let Some((_, value)) = bindings.iter().find(|(n, _)| n == name) {
                *expr = value.clone();
                return;
            }
        }
        expr.node.for_each_child_mut(&mut |child| replace(child, bindings));
    }

    let span = macro_def.body.span;
    let bindings: Vec<(crate::ident::Symbol, crate::expr::Expr)> = macro_def
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let value = match args.get(i) {
                Some(a) => a.clone(),
                None if p.default.is_some() => p.default.clone().expect("checked"),
                None if p.rest => crate::expr::Expr::new(
                    span,
                    ExprNode::Array { elements: vec![], style: Default::default() },
                ),
                None => crate::expr::Expr::new(
                    span,
                    ExprNode::Hash { entries: vec![], kwargs: false },
                ),
            };
            (p.name.clone(), value)
        })
        .collect();
    let mut body = macro_def.body.clone();
    replace(&mut body, &bindings);
    body
}

/// Every filter the macro body declares, or None if any statement in it
/// is something else. The IR twin of `parse_filter_call`, which reads
/// prism nodes — by this point the concern's body is already lowered.
fn filters_from_macro_body(
    body: &crate::expr::Expr,
    module: &crate::ident::ClassId,
) -> Option<Vec<crate::dialect::Filter>> {
    use crate::expr::ExprNode;

    let mut out = Vec::new();
    let statements: Vec<&crate::expr::Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };
    for stmt in statements {
        out.extend(filter_from_send(stmt, module)?);
    }
    if out.is_empty() { None } else { Some(out) }
}

fn filter_from_send(
    expr: &crate::expr::Expr,
    module: &crate::ident::ClassId,
) -> Option<Vec<crate::dialect::Filter>> {
    use crate::dialect::{Filter, FilterKind};
    use crate::expr::{ExprNode, Literal};

    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
        return None;
    };
    let kind = match method.as_str() {
        "before_action" => FilterKind::Before,
        "around_action" => FilterKind::Around,
        "after_action" => FilterKind::After,
        "skip_before_action" => FilterKind::Skip,
        _ => return None,
    };

    let sym_of = |e: &crate::expr::Expr| match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        _ => None,
    };
    let sym_list = |e: &crate::expr::Expr| -> Vec<crate::ident::Symbol> {
        match &*e.node {
            ExprNode::Array { elements, .. } => elements.iter().filter_map(&sym_of).collect(),
            _ => sym_of(e).into_iter().collect(),
        }
    };

    let mut targets: Vec<crate::ident::Symbol> = Vec::new();
    let mut only: Vec<crate::ident::Symbol> = Vec::new();
    let mut except: Vec<crate::ident::Symbol> = Vec::new();
    for arg in args {
        if let Some(sym) = sym_of(arg) {
            targets.push(sym);
            continue;
        }
        let ExprNode::Hash { entries, .. } = &*arg.node else {
            // An argument that is neither a target nor an options hash
            // (a forwarded parameter the call site never supplied, say)
            // means this macro was written for a shape not modeled here.
            return None;
        };
        for (key, value) in entries {
            match sym_of(key).as_ref().map(|k| k.as_str().to_string()).as_deref() {
                Some("only") => only = sym_list(value),
                Some("except") => except = sym_list(value),
                // if:/unless: guards on a macro-expanded filter would
                // need the predicate to resolve in the INCLUDER; not
                // modeled, and silently dropping a guard changes when a
                // filter fires.
                Some("if") | Some("unless") => return None,
                _ => {}
            }
        }
    }
    if targets.is_empty() {
        return None;
    }
    Some(
        targets
            .into_iter()
            .map(|target| Filter {
                kind: kind.clone(),
                target,
                from_concern: Some(module.clone()),
                only: only.clone(),
                except: except.clone(),
                only_style: crate::expr::ArrayStyle::default(),
                except_style: crate::expr::ArrayStyle::default(),
                if_cond: None,
                unless_cond: None,
                if_cond_expr: None,
                unless_cond_expr: None,
            })
            .collect(),
    )
}

/// Copy each concern's `enum` columns onto the models that include it,
/// the same way the DSL splice copies its `included do` items. Campfire
/// declares `enum :role` in `User::Role`, and `User::Bot` — a different
/// concern — queries it with `where(role: :bot)`, so the table has to be
/// whole before any label can be mapped.
fn fold_concern_enums_into_models(
    app: &mut App,
    concern_enums: &[(
        crate::ident::ClassId,
        Vec<(crate::ident::Symbol, Vec<(String, crate::expr::Literal)>)>,
    )],
) {
    if concern_enums.is_empty() {
        return;
    }
    for model in &mut app.models {
        let includes = crate::analyze::model_includes(model);
        for (module, decls) in concern_enums {
            if !includes.contains(module) {
                continue;
            }
            for (column, mapping) in decls {
                model.enums.entry(column.clone()).or_insert_with(|| mapping.clone());
            }
        }
    }
}

/// Replace enum LABELS with the values their columns store, at the
/// hand-written sites Rails' own enum type would have mapped:
/// `where(role: :bot)` → `where(role: 2)`, `update!(status:
/// :deactivated)` → `update!(status: 1)`.
///
/// The `enum` declaration itself expands into scopes and predicates
/// that already carry stored values (see `expand_enum_decl`); what's
/// left is code the app wrote by hand. Two rules decide which model a
/// hash belongs to, neither needing type inference:
///
///   * inside a model's own body — including everything a concern
///     spliced in — a key naming one of THAT model's enum columns is
///     that column (campfire: `active.where(role: :bot)`, and
///     `update! status: :deactivated` on self);
///   * anywhere at all, an explicit `Model.…` receiver names the model
///     (`User.create!(attributes.merge(role: :bot))`).
///
/// Both walk the whole argument subtree rather than just a top-level
/// hash argument, because the double-splat desugar buries the literal
/// pairs inside a `merge` chain.
///
/// A value that isn't a literal (`involvement: params[:involvement]`)
/// stays put: it's a runtime string that already holds what the column
/// holds. A label with no matching enum entry also stays put — the
/// mapping only fires on an exact label match, so a same-named column
/// on another model can't be caught by rule one.
fn map_enum_labels(app: &mut App) {
    use crate::dialect::ModelBodyItem;
    use crate::expr::{Expr, ExprNode, Literal};
    use crate::ident::{ClassId, Symbol};

    type EnumTable = indexmap::IndexMap<Symbol, Vec<(String, Literal)>>;

    /// Rewrite every hash entry in this subtree whose key names a column
    /// in `table` and whose value is a label of that column.
    fn map_in_subtree(expr: &mut Expr, table: &EnumTable) {
        expr.node.for_each_child_mut(&mut |child| map_in_subtree(child, table));
        let ExprNode::Hash { entries, .. } = &mut *expr.node else { return };
        for (key, value) in entries.iter_mut() {
            let ExprNode::Lit { value: Literal::Sym { value: column } } = &*key.node else {
                continue;
            };
            let Some(mapping) = table.get(column) else { continue };
            let label = match &*value.node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                _ => continue,
            };
            let Some((_, stored)) = mapping.iter().find(|(l, _)| *l == label) else { continue };
            *value.node = ExprNode::Lit { value: stored.clone() };
        }
    }

    /// Rule two: `Model.method(…)` anywhere in the app.
    fn map_const_receiver_sites(expr: &mut Expr, tables: &HashMap<ClassId, EnumTable>) {
        expr.node.for_each_child_mut(&mut |child| map_const_receiver_sites(child, tables));
        let ExprNode::Send { recv: Some(recv), args, .. } = &mut *expr.node else { return };
        let ExprNode::Const { path } = &*recv.node else { return };
        let id = ClassId(Symbol::from(
            path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
        ));
        let Some(table) = tables.get(&id) else { return };
        for arg in args.iter_mut() {
            map_in_subtree(arg, table);
        }
    }

    let tables: HashMap<ClassId, EnumTable> = app
        .models
        .iter()
        .filter(|m| !m.enums.is_empty())
        .map(|m| (m.name.clone(), m.enums.clone()))
        .collect();
    if tables.is_empty() {
        return;
    }

    for model in &mut app.models {
        let Some(table) = tables.get(&model.name).cloned() else { continue };
        for item in &mut model.body {
            match item {
                ModelBodyItem::Method { method, .. } => map_in_subtree(&mut method.body, &table),
                ModelBodyItem::Scope { scope, .. } => map_in_subtree(&mut scope.body, &table),
                ModelBodyItem::Unknown { expr, .. } => map_in_subtree(expr, &table),
                _ => {}
            }
        }
    }
    crate::lower::for_each_hook_body(app, &mut |expr| map_const_receiver_sites(expr, &tables));
}

/// Resolve a model's `include <Const>` against Ruby's lexical scope:
/// inside `class User`, `include Avatar` names `User::Avatar` when such
/// a module exists, and only falls back to a top-level `Avatar`.
///
/// Campfire keeps every model concern that way —
/// `app/models/user/{avatar,bannable,bot,mentionable,role,transferable}.rb`
/// each declare `module User::Avatar` and friends — so the unqualified
/// ClassId matched no ingested module and the whole mixed-in surface
/// (`ban`, `create_bot!`, `active_bots`, `from_avatar_token`) dispatched
/// into nothing.
///
/// Rewrites the IR node rather than resolving at each consumer:
/// `model_includes` (analyze), `splice_concerns_into_models` above, and
/// every emitter that re-emits the line then read one qualified path.
/// Narrow trigger — only when `<Model>::<Const>` actually names an
/// ingested module, so apps whose concerns live at the top level
/// (`app/models/concerns/…`) are untouched.
fn qualify_relative_model_includes(app: &mut App) {
    use crate::dialect::ModelBodyItem;
    use crate::expr::ExprNode;

    let known: std::collections::HashSet<crate::ident::ClassId> = app
        .library_classes
        .iter()
        .map(|lc| lc.name.clone())
        .chain(app.concern_model_items.keys().cloned())
        .collect();

    for model in &mut app.models {
        let model_name = model.name.0.as_str().to_string();
        for item in &mut model.body {
            let ModelBodyItem::Unknown { expr, .. } = item else { continue };
            let ExprNode::Send { recv: None, method, args, .. } = &mut *expr.node else {
                continue;
            };
            if method.as_str() != "include" {
                continue;
            }
            for arg in args.iter_mut() {
                let ExprNode::Const { path } = &mut *arg.node else { continue };
                let [segment] = &path[..] else { continue };
                let qualified = crate::ident::ClassId(crate::ident::Symbol::from(format!(
                    "{model_name}::{}",
                    segment.as_str()
                )));
                if known.contains(&qualified) {
                    *path = vec![crate::ident::Symbol::from(model_name.as_str()), segment.clone()];
                }
            }
        }
    }

    // Same rule for a module's own includes. campfire's
    // `Authentication` concern opens with `include SessionLookup`,
    // which is `Authentication::SessionLookup` — under Ruby's lexical
    // lookup, and on disk at
    // app/controllers/concerns/authentication/session_lookup.rb.
    // Unqualified, the emitted `include SessionLookup` raises NameError
    // at load time (and nothing pulls the file into the require graph).
    for lc in &mut app.library_classes {
        let owner = lc.name.0.as_str().to_string();
        for inc in &mut lc.includes {
            if inc.0.as_str().contains("::") {
                continue;
            }
            let qualified = crate::ident::ClassId(crate::ident::Symbol::from(format!(
                "{owner}::{}",
                inc.0.as_str()
            )));
            if known.contains(&qualified) {
                *inc = qualified;
            }
        }
    }
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
                // RECURSIVE. importmap-rails globs `**/*.js` and names
                // each pin by its path RELATIVE to the pinned root, so
                // `app/javascript/lib/autocomplete/helpers.js` pins as
                // `lib/autocomplete/helpers`. Reading one level deep
                // dropped seventeen of campfire's modules — every file
                // under `lib/autocomplete/` and `lib/rich_text/` — from
                // both the import map and the page's modulepreloads,
                // which is a page that loads and a composer whose
                // autocomplete never resolves its imports.
                let mut entries: Vec<PathBuf> = Vec::new();
                collect_js_tree(vfs, &walk_dir, &mut entries)?;
                entries.sort();
                for entry in entries {
                    let Ok(rel) = entry.strip_prefix(&walk_dir) else { continue };
                    let Some(rel) = rel.to_str() else { continue };
                    // MEASURED against the oracle's rendered import map:
                    // a trailing `index.js` names its DIRECTORY, at any
                    // depth (`controllers/index.js` → `controllers`),
                    // matching JS module resolution.
                    let stem = rel
                        .strip_suffix(".js")
                        .unwrap_or(rel)
                        .trim_end_matches("index")
                        .trim_end_matches('/')
                        .to_string();
                    let name = match (&under, stem.is_empty()) {
                        (Some(ns), true) => ns.clone(),
                        (Some(ns), false) => format!("{ns}/{stem}"),
                        (None, true) => continue,
                        (None, false) => stem.clone(),
                    };
                    let file = rel.strip_suffix(".js").unwrap_or(rel);
                    let path = match &under {
                        Some(ns) => format!("/assets/{ns}/{file}.js"),
                        None => format!("/assets/{file}.js"),
                    };
                    pins.push(ImportmapPin { name, path });
                }
            }
            _ => {}
        }
    }
    Ok(Importmap { pins })
}

/// Every `*.js` under `dir`, at any depth — importmap-rails'
/// `Dir[path.join("**/*.js")]`.
fn collect_js_tree<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    out: &mut Vec<PathBuf>,
) -> IngestResult<()> {
    for entry in vfs.read_dir(dir)? {
        if vfs.is_dir(&entry) {
            collect_js_tree(vfs, &entry, out)?;
        } else if entry.extension().and_then(|e| e.to_str()) == Some("js") {
            out.push(entry);
        }
    }
    Ok(())
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

/// Every `.yml`/`.yaml` under `dir`, RECURSIVELY. Rails fixture sets
/// nest — `test/fixtures/push/subscriptions.yml` is the fixture set
/// `push_subscriptions` loading `Push::Subscription` — and a flat
/// `read_dir` silently skipped them: the file was never ingested, so
/// `push_subscriptions(:david_chrome)` reached no method at all.
/// Non-YAML files in the tree (campfire's `test/fixtures/files/*.png`,
/// which `file_fixture` reads) are filtered out here as before.
fn read_yml_files<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = Vec::new();
    walk_yml(vfs, dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_yml<V: Vfs + ?Sized>(vfs: &V, dir: &Path, out: &mut Vec<PathBuf>) -> IngestResult<()> {
    for path in vfs.read_dir(dir)? {
        if vfs.is_dir(&path) {
            walk_yml(vfs, &path, out)?;
            continue;
        }
        if matches!(path.extension().and_then(|e| e.to_str()), Some("yml") | Some("yaml")) {
            out.push(path);
        }
    }
    Ok(())
}

pub(super) fn read_erb_files<V: Vfs + ?Sized>(
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
                // `.turbo_stream.erb` also renders through the view path.
                // The stem-collision worry above is answered by naming:
                // a non-html format's lowered method carries the format
                // suffix (`create_turbo_stream`), the same shape the
                // jbuilder `_json` variants already use, so it sits
                // beside `create` rather than on top of it.
                let format = stem.rsplit_once('.').map(|(_, f)| f);
                // `.svg.erb` joins them: campfire renders a user's
                // initials as an SVG avatar (`users/avatars/show.svg.erb`)
                // and reaches it with `render formats: :svg`. Same naming
                // answer to the stem-collision worry — the lowered method
                // carries the format suffix (`show_svg`) and sits beside
                // `show` rather than on top of it.
                if stem.ends_with(".html")
                    || !stem.contains('.')
                    || format == Some("turbo_stream")
                    || format == Some("svg")
                {
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
pub(super) fn read_rb_files<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Vec<PathBuf>> {
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
/// Does an initializer load this lib subdirectory itself?
///
/// The shape campfire writes, in `config/initializers/extensions.rb`:
///
/// ```ruby
/// %w[ rails_ext ].each do |extensions_dir|
///   Dir["#{Rails.root}/lib/#{extensions_dir}/*"].each { |path| require "#{extensions_dir}/#{File.basename(path)}" }
/// end
/// ```
///
/// The directory name and the `require` are both there, but neither is
/// reachable by matching a require's ARGUMENT — the path is built by
/// interpolation from a glob. So the test is per-STATEMENT and textual:
/// a top-level statement that both names the directory and calls
/// `require` is loading it.
///
/// Per-statement rather than per-file on purpose. `assets` and `tasks`
/// are named in the comment Rails' own generator writes directly above
/// the `autoload_lib` line, and a whole-file scan would read that as a
/// load. Statement scope also keeps an unrelated `require` elsewhere in
/// the same initializer from vouching for a directory it never mentions.
fn lib_dir_is_explicitly_required<V: Vfs + ?Sized>(vfs: &V, dir: &Path, subdir: &str) -> bool {
    let init_dir = dir.join("config/initializers");
    if !vfs.is_dir(&init_dir) {
        return false;
    }
    let Ok(entries) = read_rb_files(vfs, &init_dir) else { return false };
    for entry in entries {
        let Ok(bytes) = vfs.read(&entry) else { continue };
        let file = entry.display().to_string();
        let result = super::prism::parse(&bytes, &file);
        let src = String::from_utf8_lossy(&bytes).into_owned();
        let root = result.node();
        let stmts = root
            .as_program_node()
            .map(|p| p.statements().body().iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for stmt in stmts {
            let loc = stmt.location();
            let text = &src[loc.start_offset()..loc.end_offset()];
            if text.contains("require") && text.contains(subdir) {
                return true;
            }
        }
    }
    false
}

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
/// `config.session_store :cookie_store, key: "lobster_trap"` → the key.
///
/// Line-scanned like `extract_config_time_zone` above rather than parsed:
/// the initializer is Bundler/railtie territory that ingest deliberately
/// doesn't model, and the declaration is conventionally spread across
/// lines (`key:` usually sits on the line after `session_store`, at
/// whatever indentation the generator left). Scanning forward from the
/// `session_store` line for the first `key:` covers both the one-line and
/// wrapped forms, and both quote styles; anything more exotic simply
/// yields None and the framework default stands.
/// App-defined `config.<name> = <expr>` assignments, as (name, the
/// value's SOURCE TEXT).
///
/// Rails' config object takes arbitrary keys — campfire's
/// `config/initializers/version.rb` writes `Rails.application.config
/// .app_version = ENV["APP_VERSION"].presence || … || "0"` and two call
/// sites read it back. There is nothing to model structurally: the
/// assignment IS the definition, and the read is a method call on the
/// application. So each becomes a method on the `Rails::Application`
/// reopen, carrying the value expression verbatim — the config object
/// is a compile-time fiction, and `lower::config_reader` rewrites the
/// reads to match.
///
/// Only assignments whose receiver chain is `config` or
/// `Rails.application.config`, and only a leaf name (`config.i18n
/// .fallbacks` is a framework namespace, not an app key). Names Rails
/// itself defines are left to the railtie noise they are: the caller
/// filters against what it already synthesized.
/// Rails' own config surface. An assignment to one of these is
/// railtie configuration the emitted app has no use for — either a
/// reader is synthesized for it explicitly (`time_zone`) or it
/// configures machinery that does not exist here.
const FRAMEWORK_CONFIG_KEYS: &[&str] = &[
    "time_zone",
    "session_store",
    "load_defaults",
    "eager_load",
    "cache_classes",
    "autoload_lib",
    "active_record",
    "action_controller",
    "action_view",
    "action_mailer",
    "active_job",
    "active_storage",
    "action_cable",
    "active_support",
    "action_dispatch",
    "i18n",
    "assets",
    "generators",
    "hosts",
    "logger",
    "log_level",
    "force_ssl",
    "consider_all_requests_local",
];

/// An app-registered `to_fs` format, in either spelling Rails accepts:
///
/// ```ruby
/// ActiveSupport::TimeFormats.register(:month_and_year, "%B %Y")   # current
/// Time::DATE_FORMATS[:epoch] = ->(time) { … }                     # deprecated
/// ```
///
/// The bracket form is DEPRECATED as of Rails main (`Time` carries a
/// `deprecate_constant :DATE_FORMATS` pointing at
/// `ActiveSupport::TimeFormats.register`), and campfire — which tracks
/// Rails main — still writes it. Both are read, because an app moving
/// to the new spelling must not silently lose its format.
///
/// Worth ingesting at all because the alternative is SILENTLY WRONG:
/// `Time#to_fs` falls back to `to_s` for a format it does not know, so
/// an app that registers `:epoch` (campfire,
/// `(time.to_f * 1000).to_i` — the millisecond stamp three of its
/// `data-` attributes are sorted by) would otherwise render a
/// human-readable date into a JS number field, reported by nothing.
///
/// Top level of an initializer only, which is where Rails' own docs put
/// it — the same discipline `extract_config_assignments` draws below.
/// `ActiveSupport::DateFormats` is a SEPARATE registry whose strings
/// differ for the same names (`:number` is `"%Y%m%d"` there against
/// `"%Y%m%d%H%M%S"` here), so it is deliberately not folded in: our
/// `Ty::Time` covers Date and DateTime too, and sharing one table would
/// render a full timestamp where Rails renders eight digits.
fn extract_time_formats(source: &[u8], file: &str) -> Vec<(String, TimeFormatSource)> {
    let result = super::prism::parse(source, file);
    let root = result.node();
    let src = String::from_utf8_lossy(source).into_owned();
    let stmts = root
        .as_program_node()
        .map(|p| p.statements().body().iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut out = Vec::new();
    for stmt in stmts {
        let Some(call) = stmt.as_call_node() else { continue };
        let Some(recv) = call.receiver() else { continue };
        let Some(path) = recv.as_constant_path_node() else { continue };
        let recv_loc = path.location();
        let receiver = &src[recv_loc.start_offset()..recv_loc.end_offset()];
        // `a[k] = v` parses as a call named `[]=` taking (k, v), so both
        // spellings arrive as a two-argument call and differ only in the
        // receiver and method name.
        let recognized = match super::util::constant_id_str(&call.name()) {
            "[]=" => receiver == "Time::DATE_FORMATS",
            "register" => receiver == "ActiveSupport::TimeFormats",
            _ => false,
        };
        if !recognized {
            continue;
        }
        let Some(args) = call.arguments() else { continue };
        let mut args = args.arguments().iter();
        let (Some(key), Some(value)) = (args.next(), args.next()) else {
            continue;
        };
        let Some(key) = key.as_symbol_node() else { continue };
        let name = String::from_utf8_lossy(key.unescaped()).into_owned();

        // A String format is a strftime string; a lambda is inlined.
        // Rails picks between them at run time with `respond_to?(:call)`.
        if let Some(string) = value.as_string_node() {
            out.push((
                name,
                TimeFormatSource::Strftime(
                    String::from_utf8_lossy(string.unescaped()).into_owned(),
                ),
            ));
            continue;
        }
        let Some(lambda) = value.as_lambda_node() else { continue };
        let Some(params) = lambda
            .parameters()
            .and_then(|p| p.as_block_parameters_node())
            .and_then(|p| p.parameters())
        else {
            continue;
        };
        let requireds: Vec<_> = params.requireds().iter().collect();
        let [param] = requireds.as_slice() else { continue };
        let Some(param) = param.as_required_parameter_node() else { continue };
        let Some(body) = lambda.body() else { continue };
        let body_loc = body.location();
        out.push((
            name,
            TimeFormatSource::Lambda {
                param: super::util::constant_id_str(&param.name()).to_string(),
                body: src[body_loc.start_offset()..body_loc.end_offset()].to_string(),
            },
        ));
    }
    out
}

/// A registered format as its initializer spells it, before the lambda
/// form is parsed into IR.
enum TimeFormatSource {
    Strftime(String),
    Lambda { param: String, body: String },
}

fn extract_config_assignments(source: &[u8], file: &str) -> Vec<(String, String)> {
    fn walk(stmts: Vec<ruby_prism::Node<'_>>, src: &str, out: &mut Vec<(String, String)>) {
        for stmt in stmts {
            // Config lines sit at the top level of an initializer and
            // inside the Application class body; descend through both
            // rather than the whole expression tree, which is where
            // every other config reader in this file draws the line.
            if let Some(class) = stmt.as_class_node() {
                walk(class.body().map(super::util::flatten_statements).unwrap_or_default(), src, out);
                continue;
            }
            if let Some(module) = stmt.as_module_node() {
                walk(module.body().map(super::util::flatten_statements).unwrap_or_default(), src, out);
                continue;
            }
            // The third home, and the one Rails' own generator writes:
            // `Rails.application.configure do … end`. Descending into the
            // block keeps the same discipline as the two above — named
            // containers only, not the whole expression tree.
            if let Some(call) = stmt.as_call_node() {
                if super::util::constant_id_str(&call.name()) == "configure" {
                    if let Some(body) = call
                        .block()
                        .and_then(|b| b.as_block_node())
                        .and_then(|b| b.body())
                    {
                        walk(super::util::flatten_statements(body), src, out);
                        continue;
                    }
                }
            }
            let Some(call) = stmt.as_call_node() else { continue };
            // `x.y = v` parses as a call named `y=`.
            let name = super::util::constant_id_str(&call.name()).to_string();
            let Some(base) = name.strip_suffix('=') else { continue };
            let Some(prefix) = config_receiver_path(&call.receiver()) else {
                continue;
            };
            let Some(args) = call.arguments() else { continue };
            let Some(value) = args.arguments().iter().next() else { continue };
            let loc = value.location();
            let mut reader = prefix;
            reader.push(base.to_string());
            out.push((
                reader.join("_"),
                src[loc.start_offset()..loc.end_offset()].to_string(),
            ));
        }
    }

    let result = super::prism::parse(source, file);
    let root = result.node();
    let src = String::from_utf8_lossy(source).into_owned();
    let mut out = Vec::new();
    let stmts = root
        .as_program_node()
        .map(|p| p.statements().body().iter().collect::<Vec<_>>())
        .unwrap_or_default();
    walk(stmts, &src, &mut out);
    out
}

/// The receiver chain of a config assignment, as the segments BETWEEN
/// `config` and the assigned key — or `None` when this is not a config
/// assignment at all.
///
/// `config.app_version = …` and `Rails.application.config.app_version =
/// …` (the two spellings, depending on whether the line sits in the
/// Application class body or an initializer) both yield `[]`.
///
/// `config.x.vapid.public_key = …` yields `["x", "vapid"]`. Rails' `x`
/// is an open namespace of nested OrderedOptions — arbitrarily deep, and
/// every level springs into existence on read. Modelling that as objects
/// would mean a nested dynamic bag; FLATTENING the path into one reader
/// name (`x_vapid_public_key`) keeps the lift's whole premise intact —
/// an assignment IS the definition, and every application-level value
/// reads back the same way, as a plain method on `Rails::Application`.
/// `config_reader` flattens the read side identically, so the two halves
/// meet at the same name.
fn config_receiver_path(recv: &Option<ruby_prism::Node<'_>>) -> Option<Vec<String>> {
    let path = config_path_of(recv.as_ref()?, 0)?;
    // Only `config.<key>` and the `x` namespace. A deeper chain that is
    // NOT `x` is a framework subsection (`config.action_mailer.
    // delivery_method`, `config.active_record.*`), and lifting those
    // would contradict this lift's premise: `x` is the namespace Rails
    // documents as the app's own, where an assignment IS the definition.
    // Framework config stays unlifted so a read of it fails visibly
    // rather than resolving to a reader we invented — the same rule
    // `lower::config_reader` states for the read side.
    if path.first().is_some_and(|s| s != "x") {
        return None;
    }
    Some(path)
}

/// Recursive because prism's `Node` is not `Clone` — each hop's receiver
/// is a fresh owned node, so the walk borrows down the stack rather than
/// reassigning a cursor. Depth-bounded; Rails' `x` namespace is
/// arbitrarily deep in principle, two levels in practice.
fn config_path_of(node: &ruby_prism::Node<'_>, depth: usize) -> Option<Vec<String>> {
    if depth > 8 {
        return None;
    }
    let call = node.as_call_node()?;
    let name = super::util::constant_id_str(&call.name()).to_string();
    if name == "config" {
        // Anchored: bare `config`, or `<recv>.application.config`.
        let anchored = match call.receiver() {
            None => true,
            Some(inner) => inner
                .as_call_node()
                .is_some_and(|c| super::util::constant_id_str(&c.name()) == "application"),
        };
        return anchored.then(Vec::new);
    }
    // A non-`config` hop counts only if the chain below it reaches
    // `config` — and only as a bare reader, never a call with arguments.
    if call.arguments().is_some_and(|a| a.arguments().iter().count() > 0) {
        return None;
    }
    let inner = call.receiver()?;
    let mut segments = config_path_of(&inner, depth + 1)?;
    segments.push(name);
    Some(segments)
}

fn extract_session_cookie_key(source: &[u8]) -> Option<String> {
    let source = String::from_utf8_lossy(source);
    let mut seen_session_store = false;
    for line in source.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        if !seen_session_store {
            if t.contains("config.session_store") {
                seen_session_store = true;
                // `key:` may share the line with the declaration.
                if let Some(key) = quoted_after_key_label(t) {
                    return Some(key);
                }
            }
            continue;
        }
        if let Some(key) = quoted_after_key_label(t) {
            return Some(key);
        }
    }
    None
}

/// The quoted value of a `key:` label in `text`, if it has one. Anchored
/// on the label so a `key:` inside another option's value can't match.
fn quoted_after_key_label(text: &str) -> Option<String> {
    let idx = text.find("key:")?;
    // Reject `session_key:` / `secret_key:` — the label must start the
    // option, i.e. be preceded by nothing or a separator.
    let preceded_ok = text[..idx]
        .chars()
        .next_back()
        .map(|c| c == ',' || c == '(' || c.is_whitespace())
        .unwrap_or(true);
    if !preceded_ok {
        return None;
    }
    let rest = text[idx + "key:".len()..].trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let inner = &rest[1..];
    let end = inner.find(quote)?;
    Some(inner[..end].to_string())
}

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

/// Shared test-support modules under `test/test_helpers/`, keyed by
/// module name, filtered to the ones the app's own
/// `test/test_helper.rb` mixes into every test case.
///
/// The filter matters. campfire ships four such modules but includes
/// only three; the fourth, `SystemTestHelper`, is included by
/// `application_system_test_case.rb` and is Capybara all the way down
/// (`visit`, `find`, `fill_in`). Splicing it into every test class
/// would put a pile of permanently-unresolvable dispatch into the
/// emit for methods nothing calls — `test/system/` is out of scope
/// (see the test-file loop above), so nothing would ever reach them.
///
/// Reading the include list rather than globbing the directory is also
/// what makes this track the app: a module the app stops mixing in
/// stops being spliced.
fn ingest_test_helper_modules<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
) -> IngestResult<Vec<LibraryClass>> {
    let helpers_dir = dir.join("test/test_helpers");
    if !vfs.is_dir(&helpers_dir) {
        return Ok(Vec::new());
    }

    // Which modules get mixed into every test case. `test/test_helper.rb`
    // reopens `ActiveSupport::TestCase` and includes them there; ingest
    // the file as library classes and read that class's include list.
    // An app whose helper file we can't read (or that includes nothing)
    // splices nothing — the tests still ingest, they just don't gain
    // the helper methods, which is the pre-existing behavior.
    let mut wanted: Vec<Symbol> = Vec::new();
    let helper_rb = dir.join("test/test_helper.rb");
    if vfs.exists(&helper_rb) {
        let source = vfs.read(&helper_rb)?;
        if let Some(classes) = unwrap_or_record(ingest_library_classes(
            &source,
            &helper_rb.display().to_string(),
        ))? {
            for lc in classes {
                wanted.extend(lc.includes.iter().map(|c| c.0.clone()));
            }
        }
    }
    if wanted.is_empty() {
        return Ok(Vec::new());
    }

    let mut out: Vec<LibraryClass> = Vec::new();
    for entry in read_rb_files(vfs, &helpers_dir)? {
        let source = vfs.read(&entry)?;
        let Some(classes) =
            unwrap_or_record(ingest_library_classes(&source, &entry.display().to_string()))?
        else {
            continue;
        };
        for lc in classes {
            if wanted.iter().any(|w| w == &lc.name.0) {
                out.push(lc);
            }
        }
    }
    // Include order, so a name defined twice resolves the way Ruby's
    // `include A, B` would.
    out.sort_by_key(|lc| {
        wanted
            .iter()
            .position(|w| w == &lc.name.0)
            .unwrap_or(usize::MAX)
    });
    Ok(out)
}

/// Copy shared helper methods onto a test class, the test's own
/// definitions winning on a name collision (Ruby resolves the class
/// body ahead of an included module).
fn splice_test_helpers(tm: &mut TestModule, helpers: &[LibraryClass]) {
    for lc in helpers {
        for m in &lc.methods {
            if m.receiver != MethodReceiver::Instance {
                continue;
            }
            if tm.helpers.iter().any(|h| h.name == m.name) {
                continue;
            }
            if tm.tests.iter().any(|t| t.name == m.name.as_str()) {
                continue;
            }
            let mut m = m.clone();
            m.enclosing_class = Some(tm.name.0.clone());
            tm.helpers.push(m);
        }
    }
}
