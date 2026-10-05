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
use std::path::{Component, Path, PathBuf};

use ruby_prism::Node;

use crate::App;
use crate::Symbol;
use crate::dialect::{LibraryClass, MethodReceiver, TestModule};
use crate::vfs::{FsVfs, MapVfs, Vfs};

use super::controller::ingest_controller_with_nesting;
use super::expr::ingest_ruby_program;
use super::fixture::ingest_fixture_file;
use super::jbuilder::ingest_jbuilder;
use super::library_class::{
    ClassKind, ConcernClassMethodSpans, classify_class_file, ingest_concern_class_method_spans,
    ingest_concern_filters, ingest_concern_model_items, ingest_helper_method_names,
    ingest_library_classes, ingest_rails_application_singleton_methods,
};
use super::model::ingest_model_with_enum_constants;
use super::routes::ingest_routes_with_draws;
use super::schema::{ingest_migration, ingest_schema};
use super::structure_sql::ingest_structure_sql;
use super::test::ingest_test_files;
use super::view::{ViewEngine, ingest_template};
use super::survey::{self, unwrap_or_record};
use super::{IngestError, IngestResult};

/// Read a file's bytes for one of the per-file walks below (ERB,
/// jbuilder, models, controllers, migrations, `structure.sql`, routes
/// split files, …). An unreadable file — the shape that matters in
/// practice is a symlink whose target is absent (Procore's
/// `app/views/shared/_princess_footer.pdf.erb` points into a
/// `components/` package that can be missing from a given checkout) —
/// used to propagate through the walk's `?` and abort the ENTIRE app
/// ingest over one bad file. In survey mode this records a `file not
/// readable` gap and returns `None` so the caller skips just that
/// file, the same as any other per-file gap; in strict mode it still
/// propagates — that is what strict mode is for.
fn read_or_ledger<V: Vfs + ?Sized>(vfs: &V, path: &Path) -> IngestResult<Option<Vec<u8>>> {
    unwrap_or_record(vfs.read(path).map_err(|e| IngestError::Unsupported {
        file: path.display().to_string(),
        message: format!("file not readable: {e} ({})", path.display()),
    }))
}

/// String-reading twin of [`read_or_ledger`] — same ledger-or-propagate
/// behavior, for the ERB/jbuilder/rbs walks that read UTF-8 text
/// directly rather than raw bytes.
fn read_to_string_or_ledger<V: Vfs + ?Sized>(vfs: &V, path: &Path) -> IngestResult<Option<String>> {
    unwrap_or_record(vfs.read_to_string(path).map_err(|e| IngestError::Unsupported {
        file: path.display().to_string(),
        message: format!("file not readable: {e} ({})", path.display()),
    }))
}

/// Ingest an entire Rails app directory from disk.
///
/// A root that is not a directory is an error, not an empty app: the
/// walker reads whatever `read_dir` yields, and for a missing path that
/// is nothing — every model, controller and view "absent" with no
/// diagnostic to say why. The `check` binary guards its own argument;
/// this guard covers every other caller (the LSP and MCP servers, the
/// test suites against a fixture that has not been generated).
pub fn ingest_app(dir: &Path) -> IngestResult<App> {
    if !dir.is_dir() {
        return Err(IngestError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} is not a directory", dir.display()),
        )));
    }
    // Absolute lockfile remotes must resolve identically whether the CLI
    // names this app by a relative path or an absolute one.
    let dir = dir.canonicalize()?;
    ingest_app_with_vfs(&FsVfs::new(), &dir)
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
/// `config/initializers/inflections.rb` → the app's inflection
/// declarations: the `inflect.irregular` / `uncountable` / `acronym`
/// calls inside the `ActiveSupport::Inflector.inflections(:en) do
/// |inflect| … end` block, with literal arguments. `inflect.plural` /
/// `inflect.singular` take regexes the port cannot carry; they are
/// counted and, under survey mode, reported as a gap rather than
/// silently dropped. Any parse trouble yields the defaults — the
/// initializer is optional and often all comments.
pub fn ingest_inflections<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> crate::naming::AppInflections {
    use crate::expr::{Expr, ExprNode, Literal};
    let path = dir.join("config/initializers/inflections.rb");
    let mut out = crate::naming::AppInflections::default();
    let Ok(source) = vfs.read_to_string(&path) else { return out };
    let file = path.to_string_lossy().into_owned();
    let Ok(program) = super::expr::ingest_ruby_program(&source, &file) else { return out };

    fn strings(e: &Expr, out: &mut Vec<String>) {
        match &*e.node {
            ExprNode::Lit { value: Literal::Str { value } } => out.push(value.clone()),
            ExprNode::Lit { value: Literal::Sym { value } } => out.push(value.as_str().to_string()),
            ExprNode::Array { elements, .. } => elements.iter().for_each(|x| strings(x, out)),
            _ => {}
        }
    }
    fn walk(e: &Expr, out: &mut crate::naming::AppInflections) {
        if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node {
            let on_inflect = matches!(&*recv.node, ExprNode::Var { name, .. } if name.as_str() == "inflect");
            if on_inflect {
                let mut words = Vec::new();
                args.iter().for_each(|a| strings(a, &mut words));
                match method.as_str() {
                    "irregular" if words.len() == 2 => {
                        out.irregular.push((words[0].clone(), words[1].clone()));
                    }
                    "uncountable" => out.uncountable.extend(words),
                    "acronym" => out.acronym.extend(words),
                    // The string form carries; a regex rule cannot.
                    "singular" if words.len() == 2 && args.len() == 2 => {
                        out.singular.push((words[0].clone(), words[1].clone()));
                    }
                    "plural" if words.len() == 2 && args.len() == 2 => {
                        out.plural.push((words[0].clone(), words[1].clone()));
                    }
                    "plural" | "singular" => out.regex_rules += 1,
                    _ => {}
                }
            }
        }
        e.node.for_each_child(&mut |c| walk(c, out));
    }
    walk(&program, &mut out);
    if out.regex_rules > 0 {
        survey::record(&IngestError::Unsupported {
            file: file.clone(),
            message: format!(
                "{} inflect.plural/singular regex rule(s) not applied — only irregular, uncountable and acronym declarations are read",
                out.regex_rules
            ),
        });
    }
    out
}

/// A module or class an initializer defines at the top level, kept
/// when the app's own code names it and nothing else defines it.
///
/// Rails runs every initializer at boot, so a constant defined there
/// is as live as one in `app/`. Lobsters' `telebugs.rb` is the shape:
/// `module Telebugs` with no-op `user`/`context`/`message` (reopened to
/// forward to Sentry only when credentials exist — that reopen sits
/// inside an `if` and is not a top-level definition), called from
/// `authenticate_user` on every signed-in request. Dropping it made
/// each of those requests a NameError.
///
/// The reference test keeps what an initializer defines for the
/// framework's own use (lobsters' `SneakWrapperIntoPath`, prepended
/// into `rails dbconsole`) out of the tree. Core classes and framework
/// roots are never taken from here: a top-level definition of one is a
/// reopen of something the runtime already provides.
fn keep_initializer_defined(
    app: &mut App,
    dir: &Path,
    sources: &[crate::span::SourceFile],
    candidates: Vec<LibraryClass>,
) {
    const FRAMEWORK_ROOTS: &[&str] = &[
        "Rails", "ActiveRecord", "ActiveSupport", "ActiveModel", "ActiveJob",
        "ActiveStorage", "ActionController", "ActionDispatch", "ActionView",
        "ActionMailer", "ActionMailbox", "ActionCable", "ActionText", "Rack",
    ];
    const CORE: &[&str] = &[
        "Object", "BasicObject", "Kernel", "Module", "Class", "Comparable",
        "Enumerable", "Integer", "Float", "Numeric", "String", "Symbol",
        "Array", "Hash", "NilClass", "TrueClass", "FalseClass", "Time",
        "Date", "DateTime", "Range", "Regexp", "Proc", "Struct", "Exception",
        "StandardError",
    ];
    let root = dir.display().to_string();
    let root = root.trim_end_matches('/');
    let referenced = |name: &str| {
        sources.iter().any(|f| {
            let rel = f.path.strip_prefix(root).unwrap_or(&f.path).trim_start_matches('/');
            (rel.starts_with("app/") || rel.starts_with("lib/"))
                && f.text.match_indices(name).any(|(i, _)| {
                    let before = f.text[..i].chars().next_back();
                    let after = f.text[i + name.len()..].chars().next();
                    !before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':')
                        && matches!(after, Some('.') | Some(':'))
                })
        })
    };
    for lc in candidates {
        let name = lc.name.0.as_str().to_string();
        if name.contains("::")
            || FRAMEWORK_ROOTS.contains(&name.as_str())
            || CORE.contains(&name.as_str())
            || app.library_classes.iter().any(|c| c.name == lc.name)
            || app.models.iter().any(|m| m.name == lc.name)
            || app.controllers.iter().any(|c| c.name == lc.name)
            || !referenced(&name)
        {
            continue;
        }
        app.library_classes.push(lc);
    }
}

pub fn ingest_app_with_vfs<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<App> {
    // Front-end dispatch: a rack app with no config/routes.rb whose
    // app.rb subclasses Roda takes the Roda + Sequel walker (issue
    // #67); everything else is a Rails-convention tree.
    if super::roda_app::is_roda_app(vfs, dir) {
        return super::roda_app::ingest_roda_app_with_vfs(vfs, dir);
    }
    super::sources::reset();
    let _source_root = super::sources::set_root(dir);
    let path_gems = path_gem_dirs(vfs, dir);
    let source_vfs = PathGemVfs { inner: vfs, root: dir, dirs: &path_gems };
    let vfs = &source_vfs;
    let additional_test_paths = additional_test_paths(vfs, dir)?;
    validate_additional_test_paths(vfs, dir, &additional_test_paths)?;
    let mut app = App::new();
    // `enum` columns declared inside a concern's `included do`, keyed by
    // the module. Local rather than a field on `App`: they exist only
    // until the splice folds them into each including model's own
    // `enums` table, and nothing downstream reads them by module.
    let mut concern_enums: Vec<(
        crate::ident::ClassId,
        Vec<(crate::ident::Symbol, Vec<(String, crate::expr::Literal)>)>,
    )> = Vec::new();

    // The app's inflections come first: everything after this that
    // turns `:leaves` into a class name or `Leaf` into a table name
    // consults them. A missing initializer leaves Rails' defaults.
    crate::naming::install_app_inflections(ingest_inflections(vfs, dir));

    // The lockfile is not a source: it never enters the analysis. It
    // is carried so the census and the unknown-gem attribution can
    // read it beside the diagnostics.
    let lock_path = dir.join("Gemfile.lock");
    if vfs.exists(&lock_path) {
        if let Ok(text) = vfs.read_to_string(&lock_path) {
            app.gem_lock = Some(crate::gems::Lockfile::parse(&text));
        }
    }

    let schema_path = dir.join("db/schema.rb");
    let structure_path = dir.join("db/structure.sql");
    if vfs.exists(&schema_path) {
        if let Some(source) = read_or_ledger(vfs, &schema_path)? {
            if let Some(schema) =
                unwrap_or_record(ingest_schema(&source, &schema_path.display().to_string()))?
            {
                app.schema = schema;
            }
        }
    } else if vfs.exists(&structure_path) {
        // `config.active_record.schema_format = :sql` apps (Postgres,
        // typically) never write schema.rb — `rails db:schema:dump`
        // writes a raw `pg_dump` DDL dump instead. Same canonical-
        // snapshot role, just SQL instead of the Rails DSL.
        if let Some(source) = read_or_ledger(vfs, &structure_path)? {
            if let Some(schema) = unwrap_or_record(ingest_structure_sql(
                &source,
                &structure_path.display().to_string(),
            ))? {
                app.schema = schema;
            }
        }
    } else {
        // No schema.rb or structure.sql (never migrated locally,
        // gitignored, or a migrations-only app) — recover the same
        // column facts by folding every `db/migrate*/*.rb` in
        // filename order across every migrate directory. A long-lived
        // app sometimes splits old migrations into `db/migrate-YYYY`
        // siblings of `db/migrate` (Procore's `db/migrate-2010` …
        // `db/migrate-2023`); folding only `db/migrate` would silently
        // miss every table those older migrations created. Sorted by
        // filename alone (not full path) since Rails' timestamp prefix
        // is what makes the order chronological — the directory a file
        // happens to live in isn't. schema.rb / structure.sql stay
        // canonical when either exists: they're the already-folded form.
        let mut migrate_dirs: Vec<PathBuf> = Vec::new();
        let default_migrate_dir = dir.join("db/migrate");
        if vfs.is_dir(&default_migrate_dir) {
            migrate_dirs.push(default_migrate_dir);
        }
        let db_dir = dir.join("db");
        if vfs.is_dir(&db_dir) {
            for entry in vfs.read_dir(&db_dir)? {
                let is_sibling_migrate_dir = vfs.is_dir(&entry)
                    && entry
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("migrate-"));
                if is_sibling_migrate_dir {
                    migrate_dirs.push(entry);
                }
            }
        }
        if !migrate_dirs.is_empty() {
            let mut files: Vec<PathBuf> = Vec::new();
            for migrate_dir in &migrate_dirs {
                files.extend(read_rb_files(vfs, migrate_dir)?);
            }
            files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
            let mut schema = crate::schema::Schema::default();
            for entry in files {
                let Some(source) = read_or_ledger(vfs, &entry)? else { continue };
                unwrap_or_record(ingest_migration(
                    &source,
                    &entry.display().to_string(),
                    &mut schema,
                ))?;
            }
            app.schema = schema;
        }
    }

    // Packwerk packages and in-repository engines share the root app's passes.
    let roots = app_roots(vfs, dir, &path_gems);
    app.app_roots = roots.iter().map(|r| r.display().to_string()).collect();
    // A namespace's `table_name_prefix` has to be known BEFORE the model
    // it prefixes is ingested, and file order does not guarantee that
    // (`push/subscription.rb` may be read before `push.rb`). One cheap
    // pre-pass over the same files, so the fact is complete when the
    // models loop starts.
    // Hoisted above the models pre-pass: the base set below has to
    // cover BOTH trees before either is classified, and the support
    // roots need this to be enumerated.
    let lib_ignores: Vec<String> = vfs
        .read(&dir.join("config/application.rb"))
        .ok()
        .map(|s| extract_autoload_lib_ignores(&s))
        .unwrap_or_default()
        .into_iter()
        .filter(|ignored| !lib_dir_is_explicitly_required(vfs, dir, ignored))
        .collect();
    let ignored_lib_file = |entry: &Path| {
        entry.strip_prefix(dir.join("lib")).is_ok_and(|rel| {
            rel.components().next().is_some_and(|c| {
                lib_ignores.iter().any(|ig| c.as_os_str() == ig.as_str())
            })
        })
    };

    let mut table_prefixes = super::model::TablePrefixes::new();
    // Action Text engine `isolate_namespace` → `action_text_` prefix.
    // Writebook's Markdown model lives under `module ActionText` without
    // an app-declared `table_name_prefix`, but its schema table is
    // `action_text_markdowns` (not `markdowns`). Seed the framework
    // prefix so ordinary model ingest matches the gem.
    table_prefixes.insert("ActionText".to_string(), "action_text_".to_string());
    // Qualified enum arrays can live in a later file (e.g. a service
    // module). Collect literal inputs before expanding any model DSL.
    let mut enum_constants = super::model::EnumConstants::default();
    let mut enum_input_files = std::collections::HashSet::new();
    // The same pre-pass answers a second question: which classes are
    // ActiveRecord bases. A model descending through the app's own
    // abstract base was classified a library class and lost its DSL,
    // and one file's AST cannot resolve that — the base is declared in
    // another file, possibly later.
    let mut model_bases = super::library_class::ModelBases::new();
    let mut base_pairs: Vec<(String, String)> = Vec::new();
    for root in &roots {
        let models_dir = dir.join(root).join("models");
        if !vfs.is_dir(&models_dir) {
            continue;
        }
        for entry in read_rb_files(vfs, &models_dir)? {
            let Some(source) = read_or_ledger(vfs, &entry)? else { continue };
            table_prefixes
                .extend(super::model::ingest_table_name_prefixes(&source, &entry.display().to_string()));
            model_bases.record(&source, &mut base_pairs);
            enum_constants.record(&source, &entry.display().to_string());
            enum_input_files.insert(entry);
        }
    }
    // An abstract base can live outside `app/models` too — in a
    // package under `lib/`, or in whatever the app adds to its
    // autoload paths. Collected before anything is classified, so a
    // model in either tree resolves against a base in either tree.
    for sub in support_roots(vfs, dir, &roots, &path_gems, &lib_ignores) {
        let support_dir = dir.join(sub.as_str());
        if !vfs.is_dir(&support_dir) {
            continue;
        }
        let Ok(entries) = read_rb_files(vfs, &support_dir) else { continue };
        for entry in entries {
            let Ok(source) = vfs.read(&entry) else { continue };
            table_prefixes.extend(super::model::ingest_table_name_prefixes(
                &source,
                &entry.display().to_string(),
            ));
            model_bases.record(&source, &mut base_pairs);
            if sub != "lib" || !ignored_lib_file(&entry) {
                enum_constants.record(&source, &entry.display().to_string());
                enum_input_files.insert(entry);
            }
        }
    }
    enum_constants.finish();
    model_bases.close_over(&base_pairs);
    for root in &roots {
        let models_dir = dir.join(root).join("models");
        if !vfs.is_dir(&models_dir) {
            continue;
        }
        for entry in read_rb_files(vfs, &models_dir)? {
            let Some(source) = read_or_ledger(vfs, &entry)? else { continue };
            let path_str = entry.display().to_string();
            match classify_class_file(&source, &model_bases) {
                Some(ClassKind::Model) | None => {
                    if let Some(maybe_model) =
                        unwrap_or_record(ingest_model_with_enum_constants(
                            &source, &path_str, &app.schema, &table_prefixes, &enum_constants,
                            &model_bases,
                        ))?
                    {
                        if let Some(model) = maybe_model {
                            // Classes nested in the model's body are
                            // classes of their own — the model walk
                            // skips them, and the same library ingest
                            // that serves `app/services` registers them
                            // here, under the qualified name Ruby gives
                            // them.
                            let outer = model.name.clone();
                            app.models.push(model);
                            if let Some(classes) =
                                unwrap_or_record(ingest_library_classes(&source, &path_str))?
                            {
                                let nested = nested_under(&outer, classes);
                                let (concern_items, concern_enum_decls) =
                                    ingest_concern_model_items(&source, &path_str);
                                app.concern_model_items.extend(concern_items.into_iter().filter(
                                    |(id, _)| nested.iter().any(|class| class.name == *id),
                                ));
                                concern_enums.extend(concern_enum_decls.into_iter().filter(
                                    |(id, _)| nested.iter().any(|class| class.name == *id),
                                ));
                                app.library_classes.extend(nested);
                            }
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
    // Support roots can nest: an engine at `lib/billing` puts
    // `lib/billing/app` and `lib/billing/lib` under the root `lib`.
    // A file in a layer of another app root belongs to that root's own
    // passes (its `models`/`controllers`/… walks, and one support root
    // per remaining layer), and a file two support roots both reach is
    // ingested by the first. A file directly in `lib/billing/app`, or
    // under its `assets`/`javascript`, has no pass of its own there and
    // stays with the walk that reached it.
    let in_nested_layer = |entry: &Path, root: &Path| {
        entry.strip_prefix(root).is_ok_and(|rel| {
            let mut components = rel.components();
            let layer = components.next();
            components.next().is_some()
                && !layer.is_some_and(|c| c.as_os_str() == "assets" || c.as_os_str() == "javascript")
        })
    };
    let nested_app_roots: Vec<PathBuf> = roots.iter().skip(1).map(|root| dir.join(root)).collect();
    let mut support_seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for sub in support_roots(vfs, dir, &roots, &path_gems, &lib_ignores) {
        let sub = sub.as_str();
        let support_dir = dir.join(sub);
        if !vfs.is_dir(&support_dir) {
            continue;
        }
        let Ok(entries) = read_rb_files(vfs, &support_dir) else { continue };
        for entry in entries {
            if sub == "lib" && ignored_lib_file(&entry) {
                continue;
            }
            if nested_app_roots
                .iter()
                .any(|root| in_nested_layer(&entry, root) && !support_dir.starts_with(root))
            {
                continue;
            }
            if !support_seen.insert(entry.clone()) {
                continue;
            }
            let Ok(source) = vfs.read(&entry) else { continue };
            let path_str = entry.display().to_string();
            // An ActiveRecord class is one wherever it lives. A
            // packwerk package under `lib/`, or an engine's models
            // reached through an autoload path, used to land here as
            // plain library classes and lose their associations,
            // validations and scopes — replayed by the ruby emitter,
            // dropped by a strict target.
            //
            // Only an explicit `Model` classification routes this way.
            // `None` — a file with no class, a bare module — stays a
            // library class here, unlike under `app/models` where the
            // directory itself is the app saying what the file is.
            if super::library_class::has_active_record_base(&source, &model_bases) {
                match ingest_model_with_enum_constants(
                    &source, &path_str, &app.schema, &table_prefixes, &enum_constants,
                    &model_bases,
                ) {
                    Ok(Some(model)) => {
                        let outer = model.name.clone();
                        app.models.push(model);
                        // Classes nested in the model's body are classes
                        // of their own, exactly as under `app/models`.
                        if let Ok(classes) = ingest_library_classes(&source, &path_str) {
                            let nested = nested_under(&outer, classes);
                            let (concern_items, concern_enum_decls) =
                                ingest_concern_model_items(&source, &path_str);
                            app.concern_model_items.extend(concern_items.into_iter().filter(
                                |(id, _)| nested.iter().any(|class| class.name == *id),
                            ));
                            concern_enums.extend(concern_enum_decls.into_iter().filter(
                                |(id, _)| nested.iter().any(|class| class.name == *id),
                            ));
                            app.library_classes.extend(nested);
                        }
                        super::on_load_reopen::ingest_on_load_reopens(&source, &path_str, &mut app);
                        continue;
                    }
                    Ok(None) => {}
                    Err(err) => {
                        if survey::is_active() {
                            survey::record(&err);
                        }
                    }
                }
            }
            match ingest_library_classes(&source, &path_str) {
                Ok(classes) => app.library_classes.extend(classes),
                Err(err) => {
                    if survey::is_active() {
                        survey::record(&err);
                    }
                }
            }
            // A reopen deferred behind `ActiveSupport.on_load` is
            // invisible to the class finder above (it skips blocks);
            // this reads the one shape that is carried and says so
            // for the rest.
            super::on_load_reopen::ingest_on_load_reopens(&source, &path_str, &mut app);
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
    for root in &roots {
        let helpers_dir = dir.join(root).join("helpers");
        if !vfs.is_dir(&helpers_dir) {
            continue;
        }
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
        // The per-key slots the lifted config readers memoize into —
        // see the config-assignment lift below.
        let mut constants: Vec<(crate::ident::Symbol, crate::expr::Expr)> = Vec::new();
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
        // `GlobalID.app` — the first segment of every `gid://<app>/
        // <Model>/<id>` this runtime mints, and half of every turbo
        // stream name that names a record. Rails takes it from the
        // application's railtie name, which is the underscored module
        // wrapping `class Application < Rails::Application`
        // (`Campfire` -> "campfire"). Synthesized as an override on the
        // Application reopen, the same shape as `session_cookie_key`
        // above; the namespace itself is dropped at ingest, so without
        // this the name is gone by emit time.
        if let Some(name) = extract_app_namespace(&source) {
            let app_name = crate::naming::underscore(&name);
            if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                "def global_id_app\n  {app_name:?}\nend\n"
            )) {
                methods.append(&mut synth);
            }
        }
        // `config.active_storage.variable_content_types -= %w[…]` — an
        // initializer trimming the image types a variant may be made
        // from (campfire drops bmp/ico/psd: loaders it does not trust).
        // The runtime answers `variable?` from Rails' default list
        // minus this one, so a bmp avatar falls back to initials here
        // exactly as it does there. Synthesized as
        // `active_storage_excluded_content_types` on the reopen, over
        // the framework default (`[]`) in runtime/ruby/rails.rb.
        {
            let init_dir = dir.join("config/initializers");
            let mut excluded: Vec<String> = Vec::new();
            if vfs.is_dir(&init_dir) {
                for entry in read_rb_files(vfs, &init_dir)? {
                    if let Ok(bytes) = vfs.read(&entry) {
                        excluded.extend(extract_variable_content_type_exclusions(&bytes));
                    }
                }
            }
            if !excluded.is_empty() {
                let literal = excluded
                    .iter()
                    .map(|t| format!("{t:?}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                    "def active_storage_excluded_content_types
  [{literal}]
end
"
                )) {
                    methods.append(&mut synth);
                }
            }
        }
        // `Vips.block_untrusted(true)` / `Vips.block("<op>", true)` —
        // an initializer setting libvips' loader policy before any
        // upload is decoded (any app that stores user uploads; campfire
        // refuses every unfuzzed loader, and openslide by name). The
        // image processor applies it at load and wraps find_load so a
        // blocked loader is not selected (runtime/spinel/facades/
        // active_storage_processor_vips.rb). Synthesized as
        // `vips_block_untrusted` / `vips_blocked_operations` on the
        // reopen, over the framework defaults (false / []) in
        // runtime/ruby/rails.rb.
        {
            let init_dir = dir.join("config/initializers");
            let mut untrusted = false;
            let mut blocked: Vec<String> = Vec::new();
            if vfs.is_dir(&init_dir) {
                for entry in read_rb_files(vfs, &init_dir)? {
                    if let Ok(bytes) = vfs.read(&entry) {
                        let (u, mut b) = extract_vips_loader_policy(&bytes);
                        untrusted = untrusted || u;
                        blocked.append(&mut b);
                    }
                }
            }
            if untrusted {
                if let Ok(mut synth) =
                    crate::runtime_src::parse_methods("def vips_block_untrusted\n  true\nend\n")
                {
                    methods.append(&mut synth);
                }
            }
            if !blocked.is_empty() {
                let literal = blocked
                    .iter()
                    .map(|t| format!("{t:?}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                    "def vips_blocked_operations\n  [{literal}]\nend\n"
                )) {
                    methods.append(&mut synth);
                }
            }
        }
        // `Kaminari.configure { |config| config.default_per_page = N }`
        // — the page size `Relation#page` applies. Synthesized as
        // `kaminari_default_per_page` on the reopen, over Kaminari's
        // own default (25) in runtime/ruby/rails.rb.
        {
            let init_dir = dir.join("config/initializers");
            let mut per_page: Option<u64> = None;
            if vfs.is_dir(&init_dir) {
                for entry in read_rb_files(vfs, &init_dir)? {
                    if let Ok(bytes) = vfs.read(&entry) {
                        let file = entry.display().to_string();
                        per_page = extract_kaminari_default_per_page(&bytes, &file).or(per_page);
                    }
                }
            }
            if let Some(n) = per_page {
                if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                    "def kaminari_default_per_page\n  {n}\nend\n"
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
            let mut assignments: Vec<(Vec<String>, String)> = Vec::new();
            for (path, bytes) in sources {
                assignments.extend(extract_config_assignments(&bytes, &path));
            }
            // ONCE PER PROCESS. An initializer runs at boot, so the
            // value an assignment writes is built one time and every
            // read afterwards answers the same object. A reader that
            // re-evaluated the expression per call was faithful for the
            // `ENV.fetch` leaves that first drove this lift and wrong
            // for an object: campfire's `config.x.web_push_pool =
            // WebPush::Pool.new(…)` handed `Room::MessagePusher` a fresh
            // pool — and a fresh pair of thread pools — on every push,
            // and its suite, waiting on `completed_task_count` of the
            // pool it could see, never saw the tasks.
            //
            // Three methods per leaf. `<key>__build` is the expression
            // verbatim (app code; the analyzer types it, and
            // `lower::config_reader` reads the reader's type off it).
            // `<key>` builds once under the lock into a per-key slot and
            // answers it. `<key>=` is the write a test makes
            // (campfire's `test_helper` replaces the pool before every
            // test). The slot is a one-element Array constant, not an
            // ivar: `Rails.application` constructs its object per call,
            // so instance state would not survive two reads — and the
            // single-element-Array holder is the shape every stub slot
            // in the runtime already uses, typed by its push on spinel.
            let mut slots: Vec<(String, String)> = Vec::new();
            for (segments, value) in &assignments {
                let name = segments.join("_");
                if FRAMEWORK_CONFIG_KEYS.contains(&name.as_str())
                    || methods.iter().any(|m| m.name.as_str() == name)
                {
                    continue;
                }
                let slot = format!("{}_SLOT", name.to_uppercase());
                if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                    "def {name}__build\n  {value}\nend\n\
                     def {name}\n  slot = {slot}\n  CONFIG_LOCK.synchronize do\n    slot.push({name}__build) if slot.empty?\n  end\n  slot[0]\nend\n\
                     def {name}=(value)\n  CONFIG_LOCK.synchronize do\n    {slot}.clear\n    {slot}.push(value)\n  end\n  value\nend\n"
                )) {
                    methods.append(&mut synth);
                    slots.push((name, slot));
                }
            }
            if !slots.is_empty() {
                let mut src = String::from("CONFIG_LOCK = Mutex.new\n");
                for (_, slot) in &slots {
                    src.push_str(&format!("{slot} = []\n"));
                }
                if let Ok(mut synth) = crate::runtime_src::parse_module_constant_exprs(&src) {
                    constants.append(&mut synth);
                }
            }
            // AN INTERMEDIATE NODE IS READ AS A WHOLE, and until now
            // nothing answered it. `config.x.vapid.private_key = …`
            // lifted a reader for the LEAF; campfire then writes
            // `Rails.configuration.x.vapid.symbolize_keys` and asks for
            // the group. In Rails that is an `OrderedOptions` whose keys
            // are its leaves, so the honest reader is a symbol-keyed
            // Hash of exactly those.
            //
            // Only a prefix whose children are ALL leaves gets one. `x`
            // itself has both a leaf (`x.web_push_pool`) and a subtree
            // (`x.vapid.*`) under it, and a Hash naming half of what is
            // there would be worse than the gap — a read of `config.x`
            // stays unlifted and fails visibly, which is the rule the
            // framework keys already follow.
            {
                use std::collections::BTreeMap;
                let mut groups: BTreeMap<Vec<String>, Vec<String>> = BTreeMap::new();
                for (segments, _) in &assignments {
                    if segments.len() >= 2 {
                        groups
                            .entry(segments[..segments.len() - 1].to_vec())
                            .or_default()
                            .push(segments[segments.len() - 1].clone());
                    }
                }
                let prefixes: Vec<Vec<String>> = groups.keys().cloned().collect();
                for (prefix, leaves) in &groups {
                    let has_subgroup = prefixes
                        .iter()
                        .any(|p| p.len() > prefix.len() && p.starts_with(prefix));
                    if has_subgroup {
                        continue;
                    }
                    let name = prefix.join("_");
                    if FRAMEWORK_CONFIG_KEYS.contains(&name.as_str())
                        || methods.iter().any(|m| m.name.as_str() == name)
                    {
                        continue;
                    }
                    let pairs: Vec<String> = leaves
                        .iter()
                        .map(|leaf| format!("{leaf}: {name}_{leaf}"))
                        .collect();
                    if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                        "def {name}\n  {{ {} }}\nend\n",
                        pairs.join(", ")
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
                constants,
                unknown_calls: Vec::new(),
                class_ivar_initializers: Vec::new(),
            });
        }
    }

    // Top-level constants an initializer defines that are not mixins —
    // kept or dropped after every app file is read (see
    // `keep_initializer_defined`).
    let mut initializer_defined: Vec<LibraryClass> = Vec::new();

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
                // Same directory, same read: `X.prepend Y` / `X.include
                // Y`. Recorded whether or not either constant is in the
                // tree — `lower::module_mixins` decides that, because it
                // runs after every class the tree will have exists.
                let mixins = extract_module_mixins(&bytes, &path_str);
                // A module the initializer DEFINES and mixes in, in the
                // same file: campfire's `WebPush::PersistentRequest`,
                // the SSRF guard its `web_push.rb` prepends onto the
                // gem's Request. No autoload path holds it, so without
                // this the mixin named a module nothing ingested and
                // was dropped with the guard. Only the mixed-in names
                // are kept here; the rest wait in `initializer_defined`
                // for the referenced-by-app test below.
                if let Ok(classes) = ingest_library_classes(&bytes, &path_str) {
                    for lc in classes {
                        if mixins.iter().any(|m| m.module.as_str() == lc.name.0.as_str()) {
                            app.library_classes.push(lc);
                        } else {
                            initializer_defined.push(lc);
                        }
                    }
                }
                app.module_mixins.extend(mixins);
                app.initializer_filters.extend(extract_initializer_filters(&bytes, &path_str));
                app.sql_functions
                    .extend(super::sql_functions::extract_sql_functions(&bytes, &path_str));
            }
        }
    }

    // Each controller's lexical nesting, for resolving its superclass
    // once every controller is known (see
    // `qualify_relative_controller_superclasses`).
    let mut controller_nesting = std::collections::HashMap::new();
    for root in &roots {
        let controllers_dir = dir.join(root).join("controllers");
        if !vfs.is_dir(&controllers_dir) {
            continue;
        }
        for entry in read_rb_files(vfs, &controllers_dir)? {
            let Some(source) = read_or_ledger(vfs, &entry)? else { continue };
            let path_str = entry.display().to_string();
            if let Some(maybe_controller) =
                unwrap_or_record(ingest_controller_with_nesting(&source, &path_str))?
            {
                if let Some((controller, nesting)) = maybe_controller {
                    controller_nesting.insert(controller.name.clone(), nesting);
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
                    let outer = controller.name.clone();
                    app.controllers.push(controller);
                    // Same as the models above: a class nested in the
                    // controller's body is its own class, registered
                    // from this file by the library ingest.
                    if let Some(classes) =
                        unwrap_or_record(ingest_library_classes(&source, &path_str))?
                    {
                        app.library_classes.extend(nested_under(&outer, classes));
                    }
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
        if let Some(source) = read_or_ledger(vfs, &routes_path)? {
            // `draw(:name)` split files — Rails loads
            // `config/routes/<name>.rb` into the same DSL context, and
            // Mastodon-class apps keep most of their route table there.
            // Keyed by the path RELATIVE TO `config/routes/`, without the
            // `.rb` extension — `draw('financials/financials_erp_routes')`
            // passes that whole relative path as the name, and keying by
            // bare file stem alone (dropping the `financials/` prefix) left
            // every subdirectory-nested draw unresolved (Procore has 34).
            // `ingest_draw_route`'s `resolve_draw_name` still falls back to
            // the bare stem when it is unambiguous, for `draw(:name)` calls
            // written against a flat `config/routes/` layout.
            let mut draw_files: HashMap<String, (Vec<u8>, String)> = HashMap::new();
            let routes_dir = dir.join("config/routes");
            if vfs.is_dir(&routes_dir) {
                for entry in read_rb_files(vfs, &routes_dir)? {
                    let Some(rel) = entry.strip_prefix(&routes_dir).ok().and_then(|p| p.to_str())
                    else {
                        continue;
                    };
                    let key = rel.trim_end_matches(".rb").replace('\\', "/");
                    let Some(split_source) = read_or_ledger(vfs, &entry)? else { continue };
                    draw_files.insert(key, (split_source, entry.display().to_string()));
                }
            }
            if let Some(routes) = unwrap_or_record(ingest_routes_with_draws(
                &source,
                &routes_path.display().to_string(),
                &draw_files,
            ))? {
                // `to: redirect("/x")` routes point at actions nobody
                // wrote, so write them: one controller, one action per
                // redirect, each a `redirect_to <literal>, status: …`. It
                // is the shape an app uses by hand for the same thing, and
                // it keeps the redirect out of every emitter's route kind.
                if !routes.redirects.is_empty() {
                    app.controllers.push(synthesize_redirect_controller(&routes.redirects));
                }
                app.routes = routes;
                synthesize_rails_health_controller(&mut app);
            }
        }
    }

    // Host templates take precedence, as in Rails. Other roots are
    // sorted by path; their relative order is not Rails engine load order.
    // A template an earlier root has under the same name and format shadows a later
    // root's: `app/views/layouts/application.html.erb` is what renders,
    // and an engine's copy of it never does. Keyed on name and format,
    // not the file, so an `.erb` override shadows a `.haml` original;
    // another FORMAT of the same name is a different template and stays.
    // Only across roots — within one, nothing changes.
    let mut view_owner: HashMap<(Symbol, Symbol), usize> = HashMap::new();
    for (root_index, root) in roots.iter().enumerate() {
        let mut keep = |view: &crate::dialect::View| {
            let owner = *view_owner
                .entry((view.name.clone(), view.format.clone()))
                .or_insert(root_index);
            owner == root_index
        };
        let views_dir = dir.join(root).join("views");
        if !vfs.is_dir(&views_dir) {
            continue;
        }
        let erb_files = read_erb_files(vfs, &views_dir)?;
        for (erb_path, engine) in erb_files {
            let Some(source) = read_to_string_or_ledger(vfs, &erb_path)? else { continue };
            let rel = erb_path
                .strip_prefix(&views_dir)
                .map_err(|_| IngestError::Unsupported {
                    file: erb_path.display().to_string(),
                    message: "view path outside views dir".into(),
                })?;
            // A handler-less file is named as Rails' resolver sees it —
            // `pwa/service_worker.js` is `pwa/service_worker.js.raw` —
            // so name/format parse the same way every template's does.
            let raw_rel;
            let rel = if engine == ViewEngine::Raw && !rel.to_string_lossy().ends_with(".raw") {
                raw_rel = PathBuf::from(format!("{}.raw", rel.display()));
                raw_rel.as_path()
            } else {
                rel
            };
            if let Some(view) = unwrap_or_record(ingest_template(
                &source,
                rel,
                &erb_path.display().to_string(),
                engine.compile_fn(),
            ))? {
                if keep(&view) {
                    app.views.push(view);
                }
            }
        }

        let jbuilder_files = read_jbuilder_files(vfs, &views_dir)?;
        for jb_path in jbuilder_files {
            let Some(source) = read_to_string_or_ledger(vfs, &jb_path)? else { continue };
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
                if keep(&view) {
                    app.views.push(view);
                }
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
    // The app-wide `setup` the same file declares — see
    // `ingest_test_case_setup`. Prepended to every test module's own.
    let test_case_setup: Option<crate::expr::Expr> = {
        let helper_rb = dir.join("test/test_helper.rb");
        if vfs.exists(&helper_rb) {
            match read_or_ledger(vfs, &helper_rb)? {
                Some(source) => unwrap_or_record(super::test::ingest_test_case_setup(
                    &source,
                    &helper_rb.display().to_string(),
                ))?
                .flatten(),
                None => None,
            }
        } else {
            None
        }
    };

    // Test files under the Rails defaults and the additional roots from
    // `roundhouse.yml`. Select every Ruby file recursively.
    let mut test_roots: Vec<PathBuf> = [
        "test/models",
        "test/controllers",
        "test/helpers",
        "test/channels",
        "test/lib",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    test_roots.extend(additional_test_paths);
    test_roots.sort();
    test_roots.dedup();

    let mut test_files = Vec::new();
    for root in test_roots {
        let tests_dir = dir.join(root);
        if !path_has_symlink_component(vfs, dir, &tests_dir) && vfs.is_dir(&tests_dir) {
            test_files.extend(read_test_rb_files(vfs, dir, &tests_dir)?);
        }
    }
    test_files.sort();
    test_files.dedup();

    for entry in test_files {
        let Some(source) = read_or_ledger(vfs, &entry)? else { continue };
        if let Some(tms) =
            unwrap_or_record(ingest_test_files(&source, &entry.display().to_string()))?
        {
            for mut tm in tms {
                splice_test_helpers(&mut tm, &shared_test_helpers);
                if let Some(case_setup) = &test_case_setup {
                    splice_test_case_setup(&mut tm, case_setup);
                }
                app.test_modules.push(tm);
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
            let Some(source) = read_or_ledger(vfs, &entry)? else { continue };
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
        if let Some(source) = read_to_string_or_ledger(vfs, &seeds_path)? {
            if let Some(expr) =
                unwrap_or_record(ingest_ruby_program(&source, &seeds_path.display().to_string()))?
            {
                app.seeds = Some(expr);
            }
        }
    }

    // `config/importmap.rb` — tiny DSL of `pin` + `pin_all_from`
    // calls. Evaluated at ingest time to build an explicit
    // name→path list; `pin_all_from` expands by walking the
    // referenced directory. Feeds the emitted
    // `javascript_importmap_tags` helper.
    let importmap_path = dir.join("config/importmap.rb");
    if vfs.exists(&importmap_path) {
        if let Some(source) = read_to_string_or_ledger(vfs, &importmap_path)? {
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
    }

    // Every app source is registered by here. Rubydex resolves a
    // SNAPSHOT on another thread while the passes below run, and those
    // passes read the same snapshot. It is not the real `drain`: the
    // passes below re-ingest generated Ruby and look up the `FileId`s of
    // real files again, so the registry must stay live until they have
    // run. Draining here and letting the registry start over at
    // `FileId(1)` is how a synthesized parse failure once rendered
    // against an unrelated real file (see `ingest::sources`'s module
    // doc). The real drain runs after them, below.
    let sources = std::sync::Arc::new(super::sources::snapshot());
    let const_resolver =
        crate::analyze::ConstResolverTask::start(std::sync::Arc::clone(&sources));

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
    // Propshaft's load path is wider than the app's two dirs: a GEM can
    // ship stylesheets, and the `:all` expansion links those too — ONLY
    // `:all`: `:app` is the app's own stylesheets, which is why real-blog
    // (`stylesheet_link_tag :app`) links no `trix.css` although its
    // bundle has the gem. So a layout must write `:all` before any gem's
    // stems join the list.
    // `action_text-trix` ships `trix.css`, which Rails links on every
    // Action Text app's pages; `lexxy` ships four (its engine adds its
    // `app/assets/stylesheets` to the path), and Rails links those on
    // campfire's since the Lexxy merge — alongside `trix.css`, whose gem
    // Action Text still depends on. The lockfile is the evidence a gem is
    // in the bundle; a tree without one falls back to Trix's importmap
    // pin (campfire before Lexxy: `pin "trix"`). Inserted in sorted
    // position by FILENAME, because the expansion sorts paths —
    // `lexxy-content.css` before `lexxy.css`, where the bare stems would
    // sort the other way — and skipped when the app carries its own
    // copy. The Makefile generator emits the copy-from-gem rule for each
    // (`apply_makefile_asset_list`).
    let pins_trix = app
        .importmap
        .iter()
        .flat_map(|m| &m.pins)
        .any(|p| p.name == "trix");
    let links_all = layouts_link_all_stylesheets(vfs, dir, &roots);
    for (gem, stems) in crate::gems::GEM_STYLESHEETS {
        if !links_all {
            continue;
        }
        let bundled = match app.gem_lock.as_ref() {
            Some(lock) => lock.has(gem) || (*gem == "action_text-trix" && pins_trix),
            None => *gem == "action_text-trix" && pins_trix,
        };
        if !bundled {
            continue;
        }
        for stem in *stems {
            if stylesheets.iter().any(|s| s == stem) {
                continue;
            }
            let file = format!("{stem}.css");
            let pos = stylesheets
                .iter()
                .position(|s| format!("{s}.css") > file)
                .unwrap_or(stylesheets.len());
            stylesheets.insert(pos, stem.to_string());
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
                let Some(source) = read_to_string_or_ledger(vfs, &entry)? else { continue };
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
                // The same file's `include`s. A gem's base class is
                // opaque to the tree, so its ancestry can only come
                // from here — and ancestry is what decides whether a
                // class-body `const` is the `T::Props` macro or an
                // unknown call to replay.
                if let Ok(includes) = crate::rbs::parse_app_includes(&source) {
                    for (class_id, modules) in includes {
                        app.rbs_includes.entry(class_id).or_default().extend(modules);
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

    // The app's own sorbet-runtime `sig` blocks, read from the trees
    // that hold its classes. Same table, same consumers as the `sig/`
    // sidecar above — and the sidecar wins where both declare a method,
    // because it is written for this analyzer while a `sig` is written
    // for Sorbet. A `sig` outside the grammar the reader models is
    // dropped whole and the method is inferred as before.
    for root in ["app", "lib"] {
        let tree = dir.join(root);
        if !vfs.is_dir(&tree) {
            continue;
        }
        let Ok(entries) = read_rb_files(vfs, &tree) else { continue };
        for entry in entries {
            let Ok(source) = vfs.read(&entry) else { continue };
            for (class_id, methods) in super::sorbet_sig::ingest_sorbet_signatures(&source) {
                let declared = app.rbs_signatures.entry(class_id).or_default();
                for (name, ty) in methods {
                    declared.entry(name).or_insert(ty);
                }
            }
        }
    }

    enum_constants.validate_consumed_sources(&app, &sources, &enum_input_files)?;
    keep_initializer_defined(&mut app, dir, &sources, initializer_defined);
    // Carrier provenance must not depend on where a module lives:
    // models, services, helpers and lib all use the same splice.
    let mut concern_class_method_spans = Vec::new();
    let mut framework_shadow_scopes = std::collections::HashSet::new();
    for source in sources.iter().filter(|source| source.path.ends_with(".rb")) {
        let (carriers, shadows) = ingest_concern_class_method_spans(source.text.as_bytes(), &source.path);
        concern_class_method_spans.extend(carriers);
        framework_shadow_scopes.extend(shadows);
    }
    // Registered source paths are prefixed with this (the fs walk
    // joins `dir`); map-VFS trees pass `""` and register app-relative.
    app.root = dir.display().to_string().trim_end_matches('/').to_string();

    // A module-nested controller's relative superclass
    // (`module Ns; class XController < BaseController`) names
    // `Ns::BaseController` under Ruby's lexical lookup. Left bare, the
    // parent matched no controller, the ancestry walk came back empty,
    // and the whole filter chain (its own base's before_action AND
    // ApplicationController's) vanished from the synthesized dispatcher.
    qualify_relative_controller_superclasses(&mut app, &controller_nesting);

    // `app/models/post/summary.rb` often reopens `class Post` only to
    // hold `Post::Summary`. That reopen is a namespace, not a class of
    // its own: kept as a library class, it owns the file
    // `app/models/post.rb` and the emit writes it over the model. With
    // the reopen dropped, the nested class keeps its own file, as a
    // class nested in the model's own file does. This runs after every
    // walk, because `app/services` and `lib` can hold the same reopen,
    // and `lib` can hold the model.
    let model_names: std::collections::HashSet<&str> =
        app.models.iter().map(|m| m.name.0.as_str()).collect();
    app.library_classes.retain(|lc| {
        let bodiless = !lc.is_module
            && lc.parent.is_none()
            && lc.includes.is_empty()
            && lc.methods.is_empty()
            && lc.class_ivar_initializers.is_empty()
            && lc.constants.is_empty()
            && lc.unknown_calls.is_empty()
            && lc.origin.is_none();
        !(bodiless && model_names.contains(lc.name.0.as_str()))
    });

    // Before the splice: it (and every later consumer) looks concerns up
    // by ClassId, so the lexical-scope resolution has to have happened.
    qualify_relative_model_includes(&mut app);
    // Before the concern splices: they read `library_classes`, and this
    // turns `Current`'s metaprogrammed surface into real methods first.
    super::current_attributes::lower_current_attributes(&mut app);
    super::thread_mattr::lower_thread_mattr(&mut app);
    // Alba declarations become ordinary property-reading methods before
    // inference; validate complete original resource bodies, not just IR.
    // A recorded refusal is not support. Strict mode still fails here.
    // Survey mode keeps the ledger entry and continues analysis.
    if let Err(err) = super::alba::lower_alba_resources(&mut app, &sources) {
        survey::continue_or_fail(err)?;
    }
    // graphql-ruby object types: analyzer-only field methods, so
    // inference carries each type's record class down the schema.
    super::graphql_ruby::lower_graphql_types(&mut app);
    // After it, not before: `Current`'s own `delegate` reads an
    // ATTRIBUTE's ivar, which that pass has the declarations for. What
    // reaches here is the general shape, whose target is a method.
    super::delegate::lower_delegates(&mut app);
    // Channels, before the concern splices for the same reason `Current`
    // is: this turns a class-body macro into real methods, and every
    // later pass reads methods.
    super::channel_callbacks::lower_channel_callbacks(&mut app);
    super::channel_callbacks::lower_channel_names(&mut app);
    splice_concerns_into_models(&mut app);
    splice_concern_class_methods_into_includers(&mut app, &concern_class_method_spans);
    super::model_macros::expand_model_macros(&mut app, &sources)?;
    // After the splice, so a class method a concern contributed gets
    // the same treatment as one written in the model.
    qualify_model_class_method_ar_calls(&mut app);
    // After the splice too: the inverse `has_many …, as: :owner` may be
    // declared in a concern's `included do`.
    resolve_polymorphic_targets(&mut app);
    // `allow_browser` becomes a filter plus the method it runs, on the
    // concern (before the splice carries both to the includer) or on
    // the controller that called it directly.
    super::allow_browser::lower_allow_browser(&mut app);
    super::rate_limit::lower_rate_limit(&mut app);
    // The real drain, now that every pass re-ingesting synthesized
    // Ruby has run. A synthesized `"<label>"` re-ingest never takes a
    // slot (`sources::register` refuses a label starting with `<`), so
    // this is the same real-file list that Rubydex indexes, and its
    // answers use these `FileId`s.
    app.sources = super::sources::drain();
    debug_assert_eq!(
        app.sources.len(),
        sources.len(),
        "a pass registered a source after Rubydex took its snapshot"
    );
    drop(sources);
    splice_concerns_into_controllers(&mut app);
    // After the splice: an action a concern provides is not implicit.
    synthesize_template_only_actions(&mut app);
    // After the splice: a macro has to resolve against the concern's
    // class-side methods, and its expansion joins the same filter chain.
    super::class_configuration::expand(&mut app, &concern_class_method_spans, &framework_shadow_scopes)?;
    expand_class_body_macros(&mut app);
    // The same idea one base over: `const` / `prop` under a class
    // whose ancestry a sidecar says reaches `T::Props` IS the
    // `T::Struct` macro, and gets expanded rather than replayed. It
    // runs here because the sidecars are read above and the AST is
    // gone by then — a base's ancestry is the one thing the tree
    // cannot see for itself.
    super::library_class::expand_props_bases(&mut app);
    // After both: the chain is complete, so a repeated declaration can
    // find the one it replaces.
    dedup_repeated_filters(&mut app);
    // After everything that consumes a class-body call: what is still
    // an unrecognized macro is reported, not dropped in silence.
    report_unrecognized_controller_macros(&app);
    // After every library class exists: the additions name constants
    // (campfire's `ContentFilters::EDITOR_FORMATTING_ATTRIBUTES`) that
    // are resolved to their literal here.
    app.content_helper_allowed_attributes = content_helper_attribute_additions(vfs, dir, &app);
    fold_concern_enums_into_models(&mut app, &concern_enums);
    // Include each base's Concern maps, and retain a child's own maps
    // (direct or Concern-declared) before filling inherited columns.
    inherit_enums(&mut app.models);
    // Last: needs every model's complete `enums` table, including the
    // columns an included concern declared.
    map_enum_labels(&mut app);
    // Last of all: `has_rich_text` can arrive through a concern's
    // `included do`, so the declaration scan has to run after the
    // splices — and `ActionText::RichText` has to be in `app.models`
    // before anything downstream enumerates models.
    crate::lower::rich_text::synthesize_record_model(&mut app);
    app.const_resolver = crate::timings::phase("rubydex: wait", || const_resolver.finish());
    // Admission needs complete controller permit demand and model DSL,
    // including declarations contributed by either kind of Concern,
    // and reuses the prepared resolver rather than rebuilding it.
    super::concern_accessors::validate(&mut app, &concern_class_method_spans, &framework_shadow_scopes)?;

    collect_binary_assets(vfs, dir, &mut app);

    debug_assert!(
        super::sources::drain().is_empty(),
        "a pass registered a source after ingest drained the registry"
    );
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
        // Concerns already spliced into this model. A spliced item may
        // itself be an `include` (a concern's `included do include
        // Other end`), whose own items are spliced in turn; a concern
        // is spliced once, so an include cycle terminates.
        let mut spliced: std::collections::HashSet<crate::ident::ClassId> =
            std::collections::HashSet::new();
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
                .filter(|id| spliced.insert((*id).clone()))
                .filter_map(|id| app.concern_model_items.get(id).map(|items| (id, items)))
                .flat_map(|(id, items)| {
                    items.iter().map(|item| rehome_default_fk(item, id, &model_name))
                })
                .collect();
            if items.is_empty() {
                i += 1;
                continue;
            }
            model.body.splice(i + 1..i + 1, items);
            i += 1;
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
/// Only a DEFAULTED key is moved. An explicit `foreign_key:` is left
/// exactly as written, even when its name matches the concern-derived
/// default (`foreign_key: :remarkable_id` inside `Remarkable`). With
/// `as:`, the key belongs to the polymorphic interface, even when its
/// name matches too (`as: :notifiable` inside `Notifiable`).
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
        crate::ident::Symbol::from(format!("{}_id", crate::naming::snake_case(crate::naming::demodulize(concern.0.as_str()))));
    let model_default =
        crate::ident::Symbol::from(format!("{}_id", crate::naming::snake_case(crate::naming::demodulize(model.0.as_str()))));
    match assoc {
        Association::HasMany { foreign_key, foreign_key_explicit: false, as_interface: None, .. }
        | Association::HasOne { foreign_key, foreign_key_explicit: false, as_interface: None, .. } => {
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

/// Canonical carrier definitions and lexical constants. Reopenings replace
/// only the same method; a same-named module singleton is not a carrier.
/// Both receiver-identity splicing and finite configuration use this table.
pub(super) fn concern_class_method_catalog(
    classes: &[LibraryClass],
    carriers: &[ConcernClassMethodSpans],
) -> HashMap<crate::ident::ClassId, (Vec<crate::dialect::MethodDef>, std::collections::HashSet<Symbol>)> {
    use std::collections::HashSet;
    let mut carried: HashMap<&crate::ident::ClassId, HashSet<&crate::span::Span>> = HashMap::new();
    for carrier in carriers {
        carried.entry(&carrier.owner).or_default().extend(&carrier.methods);
    }
    let mut class_side: HashMap<_, (Vec<crate::dialect::MethodDef>, HashSet<Symbol>)> = HashMap::new();
    for lc in classes {
        let Some(spans) = carried.get(&lc.name) else { continue };
        let (methods, consts) = class_side.entry(lc.name.clone()).or_default();
        consts.extend(lc.constants.iter().map(|(n, _)| n.clone()));
        for method in lc.methods.iter()
            .filter(|m| m.receiver == MethodReceiver::Class && spans.contains(&m.name_span))
        {
            if let Some(prior) = methods.iter_mut().find(|m| m.name == method.name) {
                *prior = method.clone();
            } else {
                methods.push(method.clone());
            }
        }
    }
    class_side
}

/// Copy a concern's CLASS-side methods onto every model or library class
/// that includes it, before analysis so receiver-relative bodies are
/// typed against each concrete includer.
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
fn splice_concern_class_methods_into_includers(
    app: &mut App,
    carriers: &[ConcernClassMethodSpans],
) {
    use crate::dialect::ModelBodyItem;
    use crate::ident::{ClassId, Symbol};
    use std::collections::{HashMap, HashSet};

    let nested_carriers: HashSet<&ClassId> = carriers.iter()
        .filter(|c| c.has_nested_carrier).map(|c| &c.owner).collect();
    let mut bridges: HashMap<&ClassId, HashSet<&crate::span::Span>> = HashMap::new();
    for carrier in carriers {
        if nested_carriers.contains(&carrier.owner) {
            bridges.entry(&carrier.owner).or_default().extend(&carrier.bridges);
        }
    }
    let class_side = concern_class_method_catalog(&app.library_classes, carriers);
    let mut module_includes: HashMap<ClassId, Vec<ClassId>> = HashMap::new();
    for lc in &app.library_classes {
        module_includes.entry(lc.name.clone()).or_default().extend(lc.includes.clone());
    }
    if class_side.is_empty() {
        return;
    }

    // A complete bridge may precede or follow the actual carrier in a
    // different reopening. Remove only those exact defs, and only after
    // finding registered carrier methods that this splice will copy.
    for lc in &mut app.library_classes {
        if class_side.get(&lc.name).is_some_and(|(methods, _)| !methods.is_empty()) {
            if let Some(spans) = bridges.get(&lc.name) {
                lc.methods.retain(|m| !spans.contains(&m.name_span));
            }
        }
    }

    let copies = |mut queue: Vec<ClassId>, mut taken: HashSet<Symbol>| {
        // Transitive closure of the includer's includes, in ancestor order.
        let mut order: Vec<ClassId> = Vec::new();
        let mut seen: HashSet<ClassId> = HashSet::new();
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
        let mut added = Vec::new();
        let mut provenance: HashMap<Symbol, ClassId> = HashMap::new();
        for concern in &order {
            let Some((methods, consts)) = class_side.get(concern) else { continue };
            for m in methods {
                if !taken.insert(m.name.clone()) {
                    continue;
                }
                provenance.insert(m.name.clone(), concern.clone());
                let mut m = m.clone();
                if !consts.is_empty() {
                    qualify_lexical_consts(&mut m.body, concern, consts);
                    for param in &mut m.params {
                        if let Some(default) = &mut param.default {
                            qualify_lexical_consts(default, concern, consts);
                        }
                    }
                }
                added.push(m);
            }
        }
        (added, provenance)
    };

    let mut spliced: HashMap<ClassId, HashMap<Symbol, ClassId>> = HashMap::new();
    for model in &mut app.models {
        let taken = model.methods()
            .filter(|m| m.receiver == MethodReceiver::Class)
            .map(|m| m.name.clone())
            .collect();
        let (added, provenance) = copies(crate::analyze::model_includes(model), taken);
        model.body.extend(added.into_iter().map(|method| ModelBodyItem::Method {
            method,
            leading_comments: Vec::new(),
            leading_blank_line: true,
        }));
        if !provenance.is_empty() {
            spliced.insert(model.name.clone(), provenance);
        }
    }
    for lc in &mut app.library_classes {
        if lc.is_module {
            continue;
        }
        let taken = lc.methods.iter()
            .filter(|m| m.receiver == MethodReceiver::Class)
            .map(|m| m.name.clone())
            .collect();
        let (added, provenance) = copies(lc.includes.clone(), taken);
        lc.methods.extend(added);
        if !provenance.is_empty() {
            spliced.insert(lc.name.clone(), provenance);
        }
    }
    app.concern_spliced_class_methods = spliced;
}

/// A routed action with a template and no method behind it gets the
/// empty method Rails behaves as if it had.
///
/// `before_action :set_api_token, only: %i[show edit]` with only `edit`
/// written out still serves `api_tokens/show.html.erb`: the router
/// dispatches `show`, the filters run, and the implicit render finds the
/// template. Nothing downstream keys on a template, though — the view's
/// ivar seed, the filter chain and every emitter's dispatch table are
/// built from the controller's actions — so the template was fed by
/// nothing (`@api_token has no known type` at each read) and the emitted
/// app had no `show` to route to. Writing the method here answers all of
/// them at once, the same way an author adding `def show; end` would.
///
/// All three must hold: a route names `controller#action`, a template
/// exists for it, and neither the controller nor an ancestor defines it.
/// A template no route reaches stays the unreachable file it is.
fn synthesize_template_only_actions(app: &mut App) {
    use crate::dialect::{Action, ControllerBodyItem, RenderTarget};
    use std::collections::{BTreeSet, HashSet};

    let view_names: HashSet<&str> = app.views.iter().map(|v| v.name.as_str()).collect();
    let has_template = |prefix: &str, action: &str| {
        let name = format!("{prefix}/{action}");
        let variant = format!("{name}.");
        view_names.iter().any(|v| *v == name || v.starts_with(&variant))
    };
    let defines = |controller: &crate::dialect::Controller, action: &Symbol| {
        // The controller itself, then its ancestors within the app.
        let mut current = Some(controller);
        let mut depth = 0;
        while let Some(c) = current {
            if c.actions().any(|a| &a.name == action) {
                return true;
            }
            depth += 1;
            if depth > 32 {
                break;
            }
            current = c.parent.as_ref().and_then(|p| app.controllers.iter().find(|o| &o.name == p));
        }
        false
    };

    let mut missing: BTreeSet<(crate::ident::ClassId, Symbol)> = BTreeSet::new();
    for route in crate::lower::routes::flatten_routes(app) {
        let Some(controller) = app.controllers.iter().find(|c| c.name == route.controller) else {
            continue;
        };
        let prefix = crate::analyze::controller_view_prefix(&controller.name);
        if has_template(&prefix, route.action.as_str()) && !defines(controller, &route.action) {
            missing.insert((route.controller.clone(), route.action.clone()));
        }
    }

    for (controller, action) in missing {
        let Ok(mut methods) =
            crate::runtime_src::parse_methods(&format!("def {}\nend\n", action.as_str()))
        else {
            continue;
        };
        let Some(method) = methods.pop() else { continue };
        let Some(controller) = app.controllers.iter_mut().find(|c| c.name == controller) else {
            continue;
        };
        let item = ControllerBodyItem::Action {
            action: Action {
                name: action,
                params: crate::ty::Row::default(),
                opt_params: Vec::new(),
                kw_params: Vec::new(),
                kwrest_param: None,
                block_param: None,
                name_span: crate::span::Span::synthetic(),
                body: method.body,
                renders: RenderTarget::Inferred,
                effects: crate::effect::EffectSet::pure(),
            },
            leading_comments: Vec::new(),
            leading_blank_line: true,
        };
        // Ahead of `private`: an action the router can reach is public.
        let at = controller
            .body
            .iter()
            .position(|item| matches!(item, ControllerBodyItem::PrivateMarker { .. }))
            .unwrap_or(controller.body.len());
        controller.body.insert(at, item);
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

    // controller -> (spliced method -> the module it came from). Built
    // alongside the splice and stored on the App: the copy is typed
    // against the includer's ivar environment, which is the wrong
    // environment when the concern sits high in the chain, and the
    // seeding site needs to know which module to ask about instead.
    let mut spliced_origin: HashMap<
        crate::ident::ClassId,
        HashMap<crate::ident::Symbol, crate::ident::ClassId>,
    > = HashMap::new();

    for controller in &mut app.controllers {
        let include_groups = crate::analyze::controller_include_groups(controller);
        let includes: Vec<crate::ident::ClassId> = include_groups.iter().flatten().cloned().collect();
        if includes.is_empty() {
            continue;
        }
        // Two orders, because Ruby has two. METHOD precedence follows
        // the ancestor chain — `include A, B` puts A ahead of B, and a
        // module ahead of what it includes — so the transitive closure
        // below walks the list as written, dependencies after, and the
        // first definition of a name wins. FILTER registration is the
        // order the `included` hooks fire, which is the reverse: Ruby
        // processes a multi-argument `include` last-argument-first, and
        // ActiveSupport::Concern includes a concern's dependencies
        // before running its own block. campfire's
        // `include AllowBrowser, Authentication, …, VersionHeaders`
        // therefore runs `set_version_headers` first and `allow_browser`
        // last; the chain used to list them the other way round.
        let filter_order = filter_registration_order(&include_groups, &module_includes);
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
        for module in &filter_order {
            for filter in app.concern_filters.get(module).into_iter().flatten() {
                if let Some(call) = &filter.block {
                    // A block-form filter goes back to being what a
                    // controller's own is: an `Unknown` item holding the
                    // call, which `block_form_filter` lowers and
                    // `build_sourced_filter_chain` chains. Provenance
                    // rides in the same map the spliced methods use,
                    // under the sentinel name the chain builder derives
                    // from the item's body index — these spliced items
                    // occupy the head of the body, so the index is the
                    // position here.
                    let crate::expr::ExprNode::Send { method, .. } = &*call.node else { continue };
                    let sentinel = crate::ident::Symbol::from(format!(
                        "__{}_block_{}__",
                        method.as_str(),
                        filters.len()
                    ));
                    spliced_origin
                        .entry(controller.name.clone())
                        .or_default()
                        .insert(sentinel, module.clone());
                    filters.push(ControllerBodyItem::Unknown {
                        expr: call.clone(),
                        leading_comments: Vec::new(),
                        leading_blank_line: false,
                    });
                    continue;
                }
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
        }
        for module in &queue {
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
                let mut kw_params = Vec::new();
                let mut kwrest_param = None;
                for p in &method.params {
                    // A keyword stays a keyword: flattened to a
                    // positional it no longer parses when its name is
                    // reserved (`next: nil`) or when a required keyword
                    // sits beside it, and `**rest` flattened to a
                    // required positional breaks every call.
                    // `**rest` arrives either as a keyword-rest or, when the
                    // library ingest flattened it, as a positional marked
                    // `from_kwrest`. Either way it is the keyword-rest, after
                    // the keywords; as a positional it would land before them.
                    if (p.keyword && p.rest) || p.from_kwrest {
                        kwrest_param = Some(p.name.clone());
                        continue;
                    }
                    if p.keyword {
                        kw_params.push((p.name.clone(), p.default.clone()));
                        continue;
                    }
                    match &p.default {
                        Some(d) => opt_params.push((p.name.clone(), d.clone())),
                        None => {
                            params.fields.insert(p.name.clone(), Ty::Untyped);
                        }
                    }
                }
                spliced_origin
                    .entry(controller.name.clone())
                    .or_default()
                    .insert(method.name.clone(), module.clone());
                methods.push(ControllerBodyItem::Action {
                    action: Action {
                        name_span: method.name_span,
                        name: method.name.clone(),
                        params,
                        opt_params,
                        kw_params,
                        kwrest_param,
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
    app.concern_spliced_actions = spliced_origin;
}

/// The order a controller's concern filters REGISTER in — the order
/// their `included` hooks fire — from its `include` statements as
/// grouped in the source. Ruby's `Module#include` invokes
/// `append_features` on its arguments last-first, so within one
/// statement the last-listed concern registers first; statements run
/// in source order. `ActiveSupport::Concern#append_features` includes
/// a concern's own concern dependencies before evaluating its
/// `included` block, so a dependency's filters land ahead of the
/// dependent's. A module already registered (Ruby `include` is
/// idempotent) is not registered again.
///
/// The dependency list is a module's `includes` as ingested — a flat
/// list, so a multi-argument `include` INSIDE a concern is walked as
/// written rather than reversed; the grouping isn't kept at that level.
pub(super) fn filter_registration_order(
    include_groups: &[Vec<crate::ident::ClassId>],
    module_includes: &HashMap<crate::ident::ClassId, Vec<crate::ident::ClassId>>,
) -> Vec<crate::ident::ClassId> {
    fn visit(
        m: &crate::ident::ClassId,
        module_includes: &HashMap<crate::ident::ClassId, Vec<crate::ident::ClassId>>,
        seen: &mut std::collections::BTreeSet<crate::ident::ClassId>,
        out: &mut Vec<crate::ident::ClassId>,
    ) {
        if !seen.insert(m.clone()) {
            return;
        }
        for dep in module_includes.get(m).into_iter().flatten() {
            visit(dep, module_includes, seen, out);
        }
        out.push(m.clone());
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for group in include_groups {
        for m in group.iter().rev() {
            visit(m, module_includes, &mut seen, &mut out);
        }
    }
    out
}

/// One ancestry snapshot for both class-body expansion paths. Direct include
/// order is retained for the existing filter-macro lookup contract; transitive
/// membership and inherited instance names serve finite class configuration.
pub(super) struct ControllerConcernSurface {
    pub direct_includes: Vec<crate::ident::ClassId>,
    pub includes: Vec<crate::ident::ClassId>,
    pub inherited_includes: Vec<crate::ident::ClassId>,
    pub instance_methods: std::collections::HashSet<Symbol>,
}

pub(super) struct ControllerConcernSurfaces {
    pub module_includes: HashMap<crate::ident::ClassId, Vec<crate::ident::ClassId>>,
    pub controllers: HashMap<crate::ident::ClassId, ControllerConcernSurface>,
}

pub(super) fn controller_concern_surfaces(app: &App) -> ControllerConcernSurfaces {
    let mut module_includes: HashMap<_, Vec<_>> = HashMap::new();
    for lc in &app.library_classes {
        module_includes.entry(lc.name.clone()).or_default().extend(lc.includes.clone());
    }
    let mut controllers = HashMap::new();
    for controller in &app.controllers {
        let mut direct_includes = Vec::new();
        let mut inherited = Vec::new();
        let mut instance_methods = std::collections::HashSet::new();
        let mut cur = Some(controller);
        let mut seen = std::collections::HashSet::new();
        while let Some(c) = cur {
            if !seen.insert(&c.name) {
                break;
            }
            for inc in crate::analyze::controller_includes(c) {
                if c.name != controller.name && !inherited.contains(&inc) {
                    inherited.push(inc.clone());
                }
                if !direct_includes.contains(&inc) {
                    direct_includes.push(inc);
                }
            }
            instance_methods.extend(c.actions().map(|a| a.name.clone()));
            cur = c.parent.as_ref().and_then(|p| app.controllers.iter().find(|o| &o.name == p));
        }
        controllers.insert(controller.name.clone(), ControllerConcernSurface {
            includes: filter_registration_order(&[direct_includes.clone()], &module_includes),
            inherited_includes: filter_registration_order(&[inherited], &module_includes),
            direct_includes,
            instance_methods,
        });
    }
    ControllerConcernSurfaces { module_includes, controllers }
}

/// Rails' `remove_duplicates`: declaring `before_action :set_room` a
/// second time REPLACES the earlier callback rather than adding one.
/// `ActiveSupport::Callbacks::CallbackChain#append` deletes every entry
/// whose kind and filter match before pushing the new one, so the later
/// declaration's `only:` / `except:` are the ones that apply, and the
/// body runs once. campfire writes exactly this: its `RoomScoped` concern
/// says `before_action :set_room` and `MessagesController` says
/// `before_action :set_room, except: :create` — Rails runs `set_room`
/// once, and not at all for `create`; the emit ran it twice, a
/// `find_by!` and a `room` read each time.
///
/// The chain here is body order with a concern's filters ahead of the
/// includer's own (`splice_concerns_into_controllers`), which is Rails'
/// order too, so keeping the LAST of each (kind, target) is the same
/// rule. Conditions (`if:` / `unless:`) do not enter the match, as in
/// Rails. `skip_*` entries are subtractions, not declarations, and are
/// left alone.
fn dedup_repeated_filters(app: &mut App) {
    for controller in app.controllers.iter_mut() {
        let body = &controller.body;
        let superseded: Vec<bool> = body
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let crate::dialect::ControllerBodyItem::Filter { filter, .. } = item else {
                    return false;
                };
                if filter.kind.is_skip() {
                    return false;
                }
                body[i + 1..].iter().any(|later| {
                    matches!(later, crate::dialect::ControllerBodyItem::Filter { filter: f, .. }
                        if f.kind == filter.kind && f.target == filter.target)
                })
            })
            .collect();
        if !superseded.iter().any(|s| *s) {
            continue;
        }
        let mut i = 0;
        controller.body.retain(|_| {
            let keep = !superseded[i];
            i += 1;
            keep
        });
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

/// Class-body calls the pipeline consumes from an `Unknown` item
/// without turning it into a typed item: the concern splice reads
/// `include`, the ingester's side channel reads `layout`, the lowering
/// reads `helper_method` and `rescue_from`. Visibility keywords are
/// markers. (`protect_from_forgery` / `skip_forgery_protection` are not
/// here: `parse_filter_call` types the forms that are modeled, and one
/// left in the body — `with: :null_session`, say — is a real gap.)
const CONSUMED_CONTROLLER_MACROS: &[&str] = &[
    "include",
    "extend",
    "layout",
    "helper_method",
    "rescue_from",
    "private",
    "protected",
    "public",
    // The generator's own `allow_browser versions: :modern` on
    // ApplicationController: recognized, held back from the emit per
    // target until every runtime's request answers `user_agent` — see
    // `ingest::allow_browser`'s module doc. A per-target decision, not
    // a coverage gap.
    "allow_browser",
    // Conditional GET: the runtime answers every request fresh, a
    // decision recorded in docs/pipeline/runtime.md ("Conditional GET
    // is ALWAYS FRESH"), so the importmap ETag macro has nothing to do.
    "stale_when_importmap_changes",
    // `prepend Mod` — read the same way `include Mod` is, by
    // `controller_includes`/`controller_include_groups` (which now
    // scan for both). Ruby's MRO puts a prepended module AHEAD of the
    // class rather than behind it — not modeled, so a prepended
    // module that redefines a name the controller ALSO defines itself
    // resolves to the controller's own version rather than the
    // module's, the opposite of Rails. No controller in this
    // codebase's fixtures collides that way; a real one would need the
    // priority modeled, not just the membership.
    "prepend",
    // `private_constant :NAME` — restricts constant visibility outside
    // the class. Roundhouse's targets have no notion of a private
    // constant (nothing outside the app constant-resolves across a
    // component boundary the same way), so there's nothing to enforce
    // and nothing lost by not enforcing it.
    "private_constant",
    // `helper SomeHelper` / `helper :all` — registers Ruby view
    // helpers for ERB. Rails core, same family as `helper_method`
    // (already recognized): it changes what a VIEW can call, not the
    // controller's own instance surface, so it has nothing to do here.
    "helper",
    // `delegate :a, :b, to: :assoc` — consumed by the controller→
    // library lowering (`lower::controller_to_library::
    // collect_delegate_calls` + `ingest::delegate::
    // expand_delegates_in_class`), the same machinery a model's or
    // concern's `delegate` already goes through. Left here rather
    // than typed at ingest because the shape it can and can't expand
    // (zero-arg forwarders only) is exactly `delegate.rs`'s call, and
    // duplicating that decision would risk the two disagreeing.
    "delegate",
    // `attr_reader`/`attr_writer`/`attr_accessor` — consumed by
    // `lower::controller_to_library::collect_attr_accessor_methods`,
    // which synthesizes the same accessor `MethodDef`s
    // `ingest::library_class` already does for models and concerns.
    "attr_reader",
    "attr_writer",
    "attr_accessor",
    // `alias_method :new, :old` / `undef_method :a, :b` — consumed by
    // `lower::controller_to_library::apply_alias_methods` /
    // `apply_undef_methods`, which resolve against the controller's
    // own already-built methods. A target that resolves to nothing
    // (an inherited or concern-defined name this same-class-only scan
    // can't see) earns its OWN specific ledger line from there, not
    // this generic one — see those functions' doc comments.
    "alias_method",
    "undef_method",
];

/// `using SomeRefinement` — Ruby's block-scoped monkey-patch
/// mechanism. Recognized so it earns its own ledger line rather than
/// the generic "not recognized" one, but never modeled: refinement
/// semantics (a method visible only within the `using` scope) have no
/// analogue in any target here, and pretending the refined methods
/// exist everywhere would be worse than not modeling them at all.
const REFINEMENT_MACROS: &[&str] = &["using"];

/// A receiverless, blockless call left in a controller's class body
/// after every consumer has run is a macro roundhouse does not
/// recognize — `rate_limit`, say. Its effect (a guard, a filter, a
/// header) would otherwise vanish from the output with no trace, which
/// is the one thing the survey exists to prevent: record it so
/// `check --continue` lists it and the coverage note names it. Filters
/// were typed at ingest, concern-exported macros were expanded above,
/// and block-form filters carry a block, so none of those reach here.
fn report_unrecognized_controller_macros(app: &App) {
    use crate::ControllerBodyItem;
    use crate::expr::ExprNode;
    if !survey::is_active() {
        return;
    }
    for controller in &app.controllers {
        for item in &controller.body {
            let ControllerBodyItem::Unknown { expr, .. } = item else { continue };
            let ExprNode::Send { recv: None, method, block: None, .. } = &*expr.node else {
                continue;
            };
            if CONSUMED_CONTROLLER_MACROS.contains(&method.as_str()) {
                continue;
            }
            // `before_action -> { … }, only: […]` (233 controllers) and
            // its `prepend_before_action`/`after_action` siblings — a
            // lambda/proc argument target instead of a Symbol, with no
            // block attached (a block-attached filter already failed
            // the `block: None` match above and never reaches here).
            // `super::controller::lambda_filter_target` is the same
            // recognizer `build_filter_preamble` lowers it with and
            // `build_sourced_filter_chain` seeds its ivars with, so this
            // exclusion is exactly as wide as the support actually is.
            if super::controller::lambda_filter_target(expr).is_some() {
                continue;
            }
            let file = super::sources::path_of(expr.span.file)
                .unwrap_or_else(|| controller.name.0.as_str().to_string());
            if REFINEMENT_MACROS.contains(&method.as_str()) {
                // `using SomeRefinement` — name the refinement in the
                // ledger so the gap is actionable, rather than folding
                // it into the generic "not recognized" bucket every
                // other dropped macro shares.
                let refinement = match &*expr.node {
                    ExprNode::Send { args, .. } => args.first().and_then(|a| match &*a.node {
                        ExprNode::Const { path } => {
                            Some(path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"))
                        }
                        _ => None,
                    }),
                    _ => None,
                }
                .unwrap_or_else(|| "?".to_string());
                survey::record(&IngestError::Unsupported {
                    file,
                    message: format!(
                        "refinement activated: `using {refinement}` (not modeled)"
                    ),
                });
                continue;
            }
            if survey::recorded().iter().any(|gap| {
                gap.contains("class configuration") && gap.contains(method.as_str())
            }) {
                continue;
            }
            survey::record(&IngestError::Unsupported {
                file,
                message: format!(
                    "controller class-body macro not recognized: `{}` (its effect is dropped from the output)",
                    method.as_str()
                ),
            });
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
        for method in class_side {
            let methods = macros.entry(lc.name.clone()).or_default();
            if let Some(prior) = methods.iter_mut().find(|m| m.name == method.name) {
                *prior = method;
            } else {
                methods.push(method);
            }
        }
    }
    if macros.is_empty() {
        return;
    }

    let surfaces = controller_concern_surfaces(app);

    for controller in &mut app.controllers {
        let includes = &surfaces.controllers[&controller.name].direct_includes;
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
            // A reader beside the writer is the normal shape
            // (`def options; @options || {}; end`). The writer still
            // only stores the keyword rest, so the call is consumed.
            // An attached block never reaches this arm.
            // A refused concern is not executed. A method whose body is
            // only the keyword-rest store is still consumed: the other
            // statements stay unexpanded.
            let another_unreadable = controller.body.iter().any(|other| {
                let ControllerBodyItem::Unknown { expr: other_expr, .. } = other else {
                    return false;
                };
                let ExprNode::Send { recv: None, method: other_method, args, block: None, .. } =
                    &*other_expr.node
                else {
                    return false;
                };
                other_method == method && stored_options_init(other_expr, &macro_def).is_none() && !args.is_empty()
            });
            if method_stores_keyword_rest(&macro_def) && !another_unreadable {
                if let Some(init) = stored_options_init(expr, &macro_def) {
                    survey::record(&IngestError::Unsupported {
                        file: controller.name.0.as_str().to_string(),
                        message: format!(
                            "class configuration call stored: `{}`",
                            method.as_str()
                        ),
                    });
                    expanded.push(ControllerBodyItem::ClassIvarInit {
                        expr: init,
                        carrier: module,
                        leading_comments: leading_comments.clone(),
                        leading_blank_line: *leading_blank_line,
                    });
                    continue;
                }
            }
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
/// `**options` a concern macro forwards binds its trailing value. Consume
/// any call-site keyword producer before substituting that value into
/// the body; an argument marker is not part of the options Hash itself.
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
fn stored_options_init(
    expr: &crate::expr::Expr,
    method: &crate::dialect::MethodDef,
) -> Option<crate::expr::Expr> {
    use crate::expr::{Expr, ExprNode, LValue, Literal};
    let body = match &*method.body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => &*exprs[0].node,
        other => other,
    };
    let ExprNode::Assign { target: LValue::Ivar { name }, .. } = body else {
        return None;
    };
    let ExprNode::Send { args, .. } = &*expr.node else { return None };
    let value = match args.as_slice() {
        [] => Expr::new(expr.span, ExprNode::Hash { entries: vec![], kwargs: false }),
        [hash] => {
            let readable = match &*hash.node {
                ExprNode::Hash { entries, .. } => entries.iter().all(|(key, value)| {
                    matches!(&*key.node, ExprNode::Lit { value: Literal::Sym { .. } })
                        && matches!(
                            &*value.node,
                            ExprNode::Lit { .. } | ExprNode::Lambda { .. } | ExprNode::Array { .. }
                        )
                }),
                ExprNode::KeywordSplat { value } => matches!(&*value.node, ExprNode::Hash { .. }),
                _ => false,
            };
            if !readable {
                return None;
            }
            let mut value = hash.clone();
            if let ExprNode::KeywordSplat { value: inner } = &*value.node {
                value = inner.clone();
            }
            value
        }
        _ => return None,
    };
    Some(Expr::new(
        expr.span,
        ExprNode::Assign {
            target: LValue::Ivar { name: name.clone() },
            value,
        },
    ))
}

fn method_stores_keyword_rest(method: &crate::dialect::MethodDef) -> bool {
    use crate::expr::{ExprNode, LValue};
    let [param] = method.params.as_slice() else { return false };
    if method.block_param.is_some() || method.has_anonymous_block {
        return false;
    }
    let body = match &*method.body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => &*exprs[0].node,
        other => other,
    };
    matches!(body, ExprNode::Assign { target: LValue::Ivar { .. }, value }
        if matches!(&*value.node, ExprNode::Var { name, .. } if name == &param.name))
}

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
                Some(a) => match &*a.node {
                    ExprNode::KeywordSplat { value } => value.clone(),
                    _ => a.clone(),
                },
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
pub(super) fn filters_from_macro_body(
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
        "skip_around_action" => FilterKind::SkipAround,
        "skip_after_action" => FilterKind::SkipAfter,
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

    let mut targets: Vec<(crate::ident::Symbol, crate::span::Span)> = Vec::new();
    let mut only: Vec<crate::ident::Symbol> = Vec::new();
    let mut except: Vec<crate::ident::Symbol> = Vec::new();
    for arg in args {
        if let Some(sym) = sym_of(arg) {
            targets.push((sym, arg.span));
            continue;
        }
        // Macro substitution has already bound this keyword producer. Keep
        // the existing literal-options contract without guessing dynamic data.
        let arg = match &*arg.node {
            ExprNode::KeywordSplat { value } => value,
            _ => arg,
        };
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
            .map(|(target, target_span)| Filter {
                target_span,
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
                block: None,
                prepend: false,
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

/// Resolve a controller's relative superclass against Ruby's lexical
/// scope. The superclass expression is evaluated in the nesting around
/// the `class` keyword, so `module Ns; class XController <
/// BaseController` tries `Ns::BaseController` before a top-level
/// `BaseController`, while a top-level `class Ns::XController <
/// BaseController` has only the top level in scope: the `Ns::` prefix
/// names the class without opening `Ns`. Hence `nesting` (recorded at
/// ingest, innermost first), never the segments of the class's name.
///
/// Rewrites only when a candidate names an ingested controller, so
/// `ApplicationController` inside `module Ns` stays top-level. A
/// superclass written qualified (`Admin::BaseController`) or rooted
/// (`::BaseController`, recorded with an empty nesting) is left alone.
fn qualify_relative_controller_superclasses(
    app: &mut App,
    nesting: &std::collections::HashMap<crate::ident::ClassId, Vec<String>>,
) {
    let known: std::collections::HashSet<crate::ident::ClassId> =
        app.controllers.iter().map(|c| c.name.clone()).collect();
    for controller in &mut app.controllers {
        let Some(parent) = controller.parent.clone() else { continue };
        let raw = parent.0.as_str();
        if raw.contains("::") {
            continue;
        }
        for scope in nesting.get(&controller.name).into_iter().flatten() {
            let id = crate::ident::ClassId(crate::ident::Symbol::from(format!("{scope}::{raw}")));
            // `module Admin; class NotesController < NotesController`
            // names the top-level one: a class is never its own
            // superclass.
            if id == controller.name {
                continue;
            }
            if known.contains(&id) {
                controller.parent = Some(id);
                break;
            }
        }
    }
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
/// which `file_fixture` reads) are filtered out here as before, and so
/// is all of `test/fixtures/files/`: it is `file_fixture_path`, data a
/// test reads, and Rails leaves it out of `fixtures :all` even when the
/// files there are YAML.
fn read_yml_files<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = Vec::new();
    walk_yml(vfs, dir, &mut out)?;
    let file_fixtures = dir.join("files");
    out.retain(|p| !p.starts_with(&file_fixtures));
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
                // `.text.erb` (mailer plain-text variants, the mailer
                // layout) and `.json.erb` (the PWA manifest) join them
                // the same way: lowered as `<action>_text` /
                // `<action>_json`, beside the html template. They were
                // the last un-ingested templates in every `rails new`
                // app, so an otherwise fully-covered app still showed
                // four coverage gaps.
                // `.pdf.erb` / `.csv.erb` / `.txt.erb` join the same way:
                // ordinary ERB producing text (a PDF renderer's HTML
                // input, a `CSV.generate` body, a mailer's plaintext
                // part) whose format is a naming/dispatch label, not a
                // different template shape. Lowered as `<action>_pdf` /
                // `<action>_csv` / `<action>_txt`, beside the html
                // template.
                if stem.ends_with(".html")
                    || !stem.contains('.')
                    || matches!(
                        format,
                        Some("turbo_stream" | "svg" | "text" | "json" | "js" | "pdf" | "csv" | "txt")
                    )
                    // Feeds: `.rss.builder` / `.atom.builder` (lobsters'
                    // `home/stories.rss.builder`), lowered as
                    // `<action>_rss` beside the html template, the same
                    // naming answer `_json` and `_svg` use. `.xls.builder`
                    // joins them: it's Builder XML markup too (Microsoft's
                    // SpreadsheetML — `xml.Workbook`/`xml.Worksheet` tags),
                    // served with an `.xls` extension so Excel opens it;
                    // the DSL and shape are identical to the feed formats,
                    // not a different engine concern.
                    || matches!(format, Some("rss" | "atom" | "xml" | "xls"))
                {
                    out.push((path, engine));
                } else {
                    record_skipped_view(&path, &format!("{e} (non-html format)"));
                }
            }
            // A view with a format and NO handler extension renders
            // through Rails' default `Raw` handler: its bytes, verbatim.
            // campfire's `pwa/service_worker.js` is one — the service
            // worker every push subscription registers. Only a name
            // that can become a method is taken. The `rails new` default
            // `service-worker.js` is hyphenated and reached only through
            // Rails' own `Rails::PwaController`, which no app here routes;
            // it holds no Ruby, so passing it over loses no analysis, and
            // recording it would put a gap on every default app.
            Some("js") => {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
                if !stem.is_empty()
                    && !stem.contains('.')
                    && stem.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
                {
                    out.push((path, ViewEngine::Raw));
                }
            }
            // Template engines we don't ingest yet — they hold Ruby (or are
            // pure Ruby, like `.json.ruby`) the analyzer never sees. Record
            // so the hole is visible to `--continue` and the LSP/MCP.
            // Moving one of these into `ViewEngine::from_extension` (above)
            // is the whole walker-side change to support a new engine.
            Some("ruby" | "rabl") => {
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

/// Check configured roots before directory checks, which follow symbolic links.
fn validate_additional_test_paths<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    paths: &[PathBuf],
) -> IngestResult<()> {
    let config_path = dir.join("roundhouse.yml");
    for path in paths {
        let tests_dir = dir.join(path);
        if path_has_symlink_component(vfs, dir, &tests_dir) {
            return Err(IngestError::Parse {
                file: config_path.display().to_string(),
                message: format!(
                    "test_paths entries must not contain symbolic links: {path:?}"
                ),
            });
        }
        if vfs.exists(&tests_dir) && !vfs.is_dir(&tests_dir) {
            return Err(IngestError::Parse {
                file: config_path.display().to_string(),
                message: format!("test_paths entries must name directories: {path:?}"),
            });
        }
    }
    Ok(())
}

/// Return true when a path component below `root` is a symbolic link.
fn path_has_symlink_component<V: Vfs + ?Sized>(vfs: &V, root: &Path, path: &Path) -> bool {
    let relative = if root.as_os_str().is_empty() {
        path
    } else if let Ok(relative) = path.strip_prefix(root) {
        relative
    } else {
        return true;
    };
    let mut current = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => {
                current.push(segment);
                if vfs.is_symlink(&current) {
                    return true;
                }
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return true,
        }
    }
    false
}

/// Collect Ruby test files without following symbolic links.
fn read_test_rb_files<V: Vfs + ?Sized>(
    vfs: &V,
    app_root: &Path,
    test_root: &Path,
) -> IngestResult<Vec<PathBuf>> {
    fn collect<V: Vfs + ?Sized>(
        vfs: &V,
        app_root: &Path,
        dir: &Path,
        out: &mut Vec<PathBuf>,
    ) -> IngestResult<()> {
        if path_has_symlink_component(vfs, app_root, dir) {
            return Ok(());
        }
        for entry in vfs.read_dir(dir)? {
            if path_has_symlink_component(vfs, app_root, &entry) {
                continue;
            }
            if vfs.is_dir(&entry) {
                collect(vfs, app_root, &entry, out)?;
            } else if entry.extension().and_then(|extension| extension.to_str()) == Some("rb") {
                out.push(entry);
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    collect(vfs, app_root, test_root, &mut files)?;
    Ok(files)
}

/// Additional test roots from the app's `roundhouse.yml`.
fn additional_test_paths<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Vec<PathBuf>> {
    #[derive(Default, serde::Deserialize)]
    #[serde(default, deny_unknown_fields)]
    struct RoundhouseConfig {
        test_paths: Vec<PathBuf>,
    }

    let config_path = dir.join("roundhouse.yml");
    let text = match vfs.read_to_string(&config_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(IngestError::Io(std::io::Error::new(
                error.kind(),
                format!("{}: {error}", config_path.display()),
            )));
        }
    };
    let config: Option<RoundhouseConfig> = serde_yaml_ng::from_str(&text).map_err(|error| {
        IngestError::Parse {
            file: config_path.display().to_string(),
            message: error.to_string(),
        }
    })?;

    let mut paths = Vec::new();
    for path in config.into_iter().flat_map(|config| config.test_paths) {
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(segment) => normalized.push(segment),
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(IngestError::Parse {
                        file: config_path.display().to_string(),
                        message: format!(
                            "test_paths entries must be non-empty app-relative directories without '..': {path:?}"
                        ),
                    });
                }
            }
        }
        if normalized.as_os_str().is_empty() {
            return Err(IngestError::Parse {
                file: config_path.display().to_string(),
                message: format!(
                    "test_paths entries must be non-empty app-relative directories without '..': {path:?}"
                ),
            });
        }
        paths.push(normalized);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// The classes from `classes` that are NESTED under `outer` — the ones
/// a model's or controller's own body walk skipped. The outer class
/// itself is ingested by its own pass and must not be registered twice.
fn nested_under(
    outer: &crate::ident::ClassId,
    classes: Vec<crate::dialect::LibraryClass>,
) -> Vec<crate::dialect::LibraryClass> {
    let prefix = format!("{}::", outer.0.as_str());
    classes.into_iter().filter(|c| c.name.0.as_str().starts_with(&prefix)).collect()
}

/// The support roots to walk for library classes: every `app/*`
/// subdirectory that has no ingest pass of its own, plus `extras` and
/// `lib`, each in-repository path gem's `lib/`, and the autoload or
/// eager-load paths from `config/application.rb`. The app's ignore
/// list removes roots. Paths are app-relative, deduplicated, and sorted.
///
/// Rails autoloads *every* `app/*` subdirectory, so a fixed list was a
/// guess about what an app calls its layers. An app whose use cases live
/// in `app/interactors/` registered none of them, and every call site
/// into them reported `send_dispatch_failed` (#86). Discovery matches
/// Rails rather than guessing, including for the trees that are only
/// ever loaded in development: a RuboCop cop under `app/` is autoloaded
/// by Rails too, and an app that does not want that says so itself —
/// which is what the ignore list below is.
///
/// A LIST OF ROOTS on purpose: a Packwerk app puts the same layers under
/// `packs/*/app/*`, which becomes one more source of roots here rather
/// than a second walker.
fn support_roots<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    roots: &[PathBuf],
    path_gems: &[PathBuf],
    lib_ignores: &[String],
) -> Vec<String> {
    // Directories under an app root that another pass already ingests
    // (models, controllers, views, helpers) or that hold no Ruby at all
    // (assets, javascript).
    const OWN_PASS: &[&str] =
        &["models", "controllers", "views", "helpers", "assets", "javascript"];

    let mut out: Vec<String> = vec!["extras".to_string(), "lib".to_string()];
    for gem in path_gems {
        let lib = gem.join("lib");
        if vfs.is_dir(&lib) {
            out.push(lib.strip_prefix(dir).expect("path gems are inside the app").display().to_string());
        }
    }
    for root in roots {
        if let Ok(entries) = vfs.read_dir(&dir.join(root)) {
            for entry in entries {
                if !vfs.is_dir(&entry) {
                    continue;
                }
                let Some(name) = entry.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if OWN_PASS.contains(&name) {
                    continue;
                }
                out.push(format!("{}/{name}", root.display()));
            }
        }
        // A package's `lib/` sits beside its `app/` (`packs/blog/lib`
        // beside `packs/blog/app`) and is autoloaded the same way the
        // root app's `lib/` is. `root.parent()` of the bare `app` root
        // is the empty path, whose `lib` is the root `lib` already
        // pushed above — deduped below, not special-cased here.
        if let Some(parent) = root.parent() {
            let lib = parent.join("lib");
            if vfs.is_dir(&dir.join(&lib)) {
                out.push(lib.display().to_string());
            }
        }
    }
    if let Ok(source) = vfs.read(&dir.join("config/application.rb")) {
        out.extend(extract_autoload_path_roots(&source));
    }
    // `autoload_lib(ignore: %w[…])` names directories the app takes off
    // the autoload paths; a root by that name is off the list for the
    // same reason its `lib/` namesake is skipped below.
    out.retain(|root| !lib_ignores.iter().any(|ignored| ignored == root));
    out.sort();
    out.dedup();
    out
}

/// App-layer roots for one Rails app: `app` first, then one
/// `<pkg>/app` per Packwerk package that has an `app/` directory and
/// one `<engine>/app` per in-repo Rails engine — sorted (after `app`)
/// and deduplicated. Every other layer walk in this file loops over
/// these instead of hardwiring `app/…`, so a Packwerk app's
/// `packs/*/app/*` (or `components/*/app/*`, `engines/*/app/*`) and an
/// engine's `lib/<name>/app/*` get the same
/// models/controllers/views/helpers passes the root `app/` does.
///
/// Apps without Packwerk packages or in-repository engines retain the
/// `["app"]` app-root list. Library-only path gems contribute support
/// roots instead.
pub(super) fn app_roots<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    path_gems: &[PathBuf],
) -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("app")];
    packwerk_app_roots(vfs, dir, &mut roots);
    engine_app_roots(vfs, dir, path_gems, &mut roots);
    roots[1..].sort();
    roots.dedup();
    roots
}

/// In-repository `PATH` sources, normalized once for app and library discovery.
fn path_gem_dirs<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> Vec<PathBuf> {
    let Ok(lock) = vfs.read_to_string(&dir.join("Gemfile.lock")) else { return Vec::new() };
    let mut dirs = Vec::new();
    for remote in crate::gems::lock_path_remotes(&lock) {
        let remote = Path::new(&remote);
        let remote = remote.strip_prefix(dir).unwrap_or(remote);
        let mut relative = PathBuf::new();
        let mut inside = true;
        for component in remote.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(part) => relative.push(part),
                _ => inside = false,
            }
        }
        // The app itself already contributes its app/ and lib/ trees.
        if !inside || relative.as_os_str().is_empty() {
            continue;
        }
        let gem = dir.join(relative);
        if !path_has_symlink_component(vfs, dir, &gem) && vfs.is_dir(&gem) {
            dirs.push(gem);
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs
}

/// Exclude symbolic links throughout selected path gems, without changing other sources.
struct PathGemVfs<'a, V: Vfs + ?Sized> {
    inner: &'a V,
    root: &'a Path,
    dirs: &'a [PathBuf],
}

impl<V: Vfs + ?Sized> PathGemVfs<'_, V> {
    /// Apply the path-gem boundary to direct reads as well as directory walks.
    fn linked(&self, path: &Path) -> bool {
        self.dirs.iter().any(|dir| path.starts_with(dir))
            && path_has_symlink_component(self.inner, self.root, path)
    }

    /// Treat excluded paths as absent, as directory discovery does.
    fn check(&self, path: &Path) -> std::io::Result<()> {
        if self.linked(path) {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "symbolic link excluded from path gem sources",
            ))
        } else {
            Ok(())
        }
    }
}

impl<V: Vfs + ?Sized> Vfs for PathGemVfs<'_, V> {
    /// Read source bytes only after the path-gem link check.
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.check(path)?;
        self.inner.read(path)
    }

    /// Read source text only after the path-gem link check.
    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        self.check(path)?;
        self.inner.read_to_string(path)
    }

    /// Omit linked children from directory listings inside selected path gems.
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<PathBuf>> {
        self.check(path)?;
        let mut entries = self.inner.read_dir(path)?;
        if self.dirs.iter().any(|dir| path.starts_with(dir)) {
            entries.retain(|entry| !self.inner.is_symlink(entry));
        }
        Ok(entries)
    }

    /// Report an excluded path-gem path as absent.
    fn exists(&self, path: &Path) -> bool {
        !self.linked(path) && self.inner.exists(path)
    }

    /// Exclude linked path-gem directories from source discovery.
    fn is_dir(&self, path: &Path) -> bool {
        !self.linked(path) && self.inner.is_dir(path)
    }

    /// Inspect link metadata without opening the target.
    fn is_symlink(&self, path: &Path) -> bool {
        self.inner.is_symlink(path)
    }
}

/// `<engine>/app` for every Rails engine the app carries in its own
/// tree: a `PATH` source in `Gemfile.lock` (`gem "x", path: "lib/x"`)
/// whose directory is inside the app, has an `app/` tree, and declares
/// a `Rails::Engine` subclass under its `lib/`. Rails adds such an
/// engine's `app/*` to the host's autoload and view paths, so its code
/// is the app's code. A path gem without an engine class is a plain
/// library whose `app/` Rails never loads, and one outside the tree
/// (`path: "../shared"`) is not this app's source — neither is a root.
fn engine_app_roots<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    path_gems: &[PathBuf],
    roots: &mut Vec<PathBuf>,
) {
    for engine_dir in path_gems {
        if !vfs.is_dir(&engine_dir.join("app"))
            || !declares_rails_engine(vfs, &engine_dir.join("lib"))
        {
            continue;
        }
        let relative = engine_dir.strip_prefix(dir).expect("path gems are inside the app");
        roots.push(relative.join("app"));
    }
}

/// Whether any Ruby file under `lib_dir` subclasses `Rails::Engine`
/// (`class Engine < ::Rails::Engine`).
fn declares_rails_engine<V: Vfs + ?Sized>(vfs: &V, lib_dir: &Path) -> bool {
    if !vfs.is_dir(lib_dir) {
        return false;
    }
    let Ok(files) = read_rb_files(vfs, lib_dir) else { return false };
    struct EngineVisitor {
        found: bool,
    }
    impl<'pr> ruby_prism::Visit<'pr> for EngineVisitor {
        fn visit_class_node(&mut self, class: &ruby_prism::ClassNode<'pr>) {
            if self.found {
                return;
            }
            self.found = class.superclass()
                .and_then(|parent| parent.as_constant_path_node())
                .is_some_and(|parent| {
                    parent.name().is_some_and(|name| super::util::constant_id_str(&name) == "Engine")
                        && parent.parent().is_some_and(|namespace| {
                            if let Some(name) = namespace.as_constant_read_node() {
                                super::util::constant_id_str(&name.name()) == "Rails"
                            } else {
                                namespace.as_constant_path_node().is_some_and(|name| {
                                    name.parent().is_none()
                                        && name.name().is_some_and(|id| super::util::constant_id_str(&id) == "Rails")
                                })
                            }
                        })
                });
            if !self.found {
                ruby_prism::visit_class_node(self, class);
            }
        }
    }
    files.iter().any(|file| {
        vfs.read(file).is_ok_and(|source| {
            let parsed = ruby_prism::parse(&source);
            let mut visitor = EngineVisitor { found: false };
            ruby_prism::Visit::visit(&mut visitor, &parsed.node());
            visitor.found
        })
    })
}

/// `<pkg>/app` for every Packwerk package that has an `app/`
/// directory. Nothing without a `packwerk.yml` or `packs.yml` at the
/// root.
fn packwerk_app_roots<V: Vfs + ?Sized>(vfs: &V, dir: &Path, roots: &mut Vec<PathBuf>) {
    let has_packwerk = vfs.exists(&dir.join("packwerk.yml")) || vfs.exists(&dir.join("packs.yml"));
    if !has_packwerk {
        return;
    }
    let package_paths = vfs
        .read(&dir.join("packwerk.yml"))
        .ok()
        .and_then(|bytes| parse_package_paths(&bytes));

    let mut package_dirs: Vec<PathBuf> = Vec::new();
    match package_paths {
        Some(globs) => {
            for glob in globs {
                expand_package_glob(vfs, dir, &glob, &mut package_dirs);
            }
        }
        // No `package_paths:` key (absent, or commented out — the
        // common case): Packwerk's own default, `**/` — every
        // directory, any depth, that carries a `package.yml`.
        None => default_package_scan(vfs, dir, &mut package_dirs),
    }
    package_dirs.sort();
    package_dirs.dedup();

    for pkg in package_dirs {
        let rel = pkg.strip_prefix(dir).unwrap_or(&pkg);
        // The root's own `package.yml` names the root package, whose
        // app root is already `app` above — not a second root.
        if rel.as_os_str().is_empty() {
            continue;
        }
        let app_dir = pkg.join("app");
        if vfs.is_dir(&app_dir) {
            roots.push(rel.join("app"));
        }
    }
}

/// `package_paths:` from a `packwerk.yml`'s bytes, as the raw glob
/// strings (Packwerk accepts either a single string or a list).
/// `None` when the key is absent (including commented out — YAML
/// never sees it) or the file doesn't parse as YAML; both cases fall
/// back to Packwerk's own default in [`app_roots`].
fn parse_package_paths(bytes: &[u8]) -> Option<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct PackwerkYml {
        #[serde(default)]
        package_paths: Option<PackagePathsValue>,
    }
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum PackagePathsValue {
        One(String),
        Many(Vec<String>),
    }
    let text = String::from_utf8_lossy(bytes);
    let parsed: PackwerkYml = serde_yaml_ng::from_str(&text).ok()?;
    match parsed.package_paths? {
        PackagePathsValue::One(s) => Some(vec![s]),
        PackagePathsValue::Many(v) => Some(v),
    }
}

/// Directories under `dir` matching a `package_paths:` glob that
/// actually carry a `package.yml` — the candidates for
/// [`app_roots`]. Supports comma-separated brace alternatives, `*`
/// (one directory level), and `**` (any depth, capped at 4 levels
/// beyond the match point); a trailing `/` is insignificant.
fn expand_package_glob<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    glob: &str,
    out: &mut Vec<PathBuf>,
) {
    if let Some(open) = glob.find('{') {
        if let Some(close) = glob[open + 1..].find('}').map(|offset| open + 1 + offset) {
            for alternative in glob[open + 1..close].split(',') {
                let expanded = format!("{}{}{}", &glob[..open], alternative, &glob[close + 1..]);
                expand_package_glob(vfs, dir, &expanded, out);
            }
            return;
        }
    }

    let segments: Vec<&str> = glob.split('/').filter(|s| !s.is_empty()).collect();
    let mut candidates = Vec::new();
    expand_glob_segments(vfs, dir, &segments, 4, &mut candidates);
    for candidate in candidates {
        if vfs.exists(&candidate.join("package.yml")) {
            out.push(candidate);
        }
    }
}

fn expand_glob_segments<V: Vfs + ?Sized>(
    vfs: &V,
    base: &Path,
    segments: &[&str],
    depth_budget: usize,
    out: &mut Vec<PathBuf>,
) {
    let Some((seg, rest)) = segments.split_first() else {
        out.push(base.to_path_buf());
        return;
    };
    match *seg {
        "**" => {
            // Zero levels consumed by `**`, then the rest of the
            // pattern against `base` itself…
            expand_glob_segments(vfs, base, rest, depth_budget, out);
            // …or one more level consumed, `**` still pending against
            // each subdirectory, capped so a pathological tree can't
            // make this unbounded.
            if depth_budget == 0 {
                return;
            }
            if let Ok(entries) = vfs.read_dir(base) {
                for entry in entries {
                    if vfs.is_dir(&entry) {
                        expand_glob_segments(vfs, &entry, segments, depth_budget - 1, out);
                    }
                }
            }
        }
        "*" => {
            if let Ok(entries) = vfs.read_dir(base) {
                for entry in entries {
                    if vfs.is_dir(&entry) {
                        expand_glob_segments(vfs, &entry, rest, depth_budget, out);
                    }
                }
            }
        }
        literal => {
            let next = base.join(literal);
            if vfs.is_dir(&next) {
                expand_glob_segments(vfs, &next, rest, depth_budget, out);
            }
        }
    }
}

/// Packwerk's own default `package_paths` (`**/`, any directory at any
/// depth) when the app declares none: directories carrying a
/// `package.yml`, found by walking down from `dir`, at most 3 levels
/// deep, skipping directories that are never a Packwerk package tree
/// (VCS/dependency/build noise, test trees, and `app` itself — a
/// package never nests a `package.yml` under the layer this scan
/// exists to find roots for).
fn default_package_scan<V: Vfs + ?Sized>(vfs: &V, dir: &Path, out: &mut Vec<PathBuf>) {
    const SKIP: &[&str] = &[
        ".git", "node_modules", "vendor", "tmp", "log", "public", "storage", "spec", "test",
        "db", "config", "app",
    ];
    const MAX_DEPTH: usize = 3;

    fn scan<V: Vfs + ?Sized>(vfs: &V, current: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = vfs.read_dir(current) else { return };
        for entry in entries {
            if !vfs.is_dir(&entry) {
                continue;
            }
            let Some(name) = entry.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if SKIP.contains(&name) {
                continue;
            }
            if vfs.exists(&entry.join("package.yml")) {
                out.push(entry.clone());
            }
            if depth < MAX_DEPTH {
                scan(vfs, &entry, depth + 1, out);
            }
        }
    }
    scan(vfs, dir, 1, out);
}

/// Roots an app adds to `config.autoload_paths` / `config.eager_load_paths`
/// in `config/application.rb`, as paths relative to the app root:
/// `config.eager_load_paths << Rails.root.join("extras")` → `extras`,
/// `config.autoload_paths += %w[app/lib]` → `app/lib`.
///
/// Line-scanned rather than parsed, like the `autoload_lib` and
/// `time_zone` readers next to it: the file is railtie soup ingest does
/// not model, and the generator ships the `<<` form commented out, so
/// comment lines have to stay unmatched.
fn extract_autoload_path_roots(source: &[u8]) -> Vec<String> {
    let source = String::from_utf8_lossy(source);
    let mut roots = Vec::new();
    for line in source.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        if !t.contains("config.autoload_paths") && !t.contains("config.eager_load_paths") {
            continue;
        }
        // `Rails.root.join("a", "b")` — the segments, joined.
        if let Some(rest) = t.split_once("Rails.root.join(").map(|(_, r)| r) {
            let Some(end) = rest.find(')') else { continue };
            let segments: Vec<&str> = rest[..end]
                .split(',')
                .filter_map(|seg| seg.trim().strip_prefix('"')?.strip_suffix('"'))
                .collect();
            if !segments.is_empty() {
                roots.push(segments.join("/"));
                continue;
            }
        }
        // `%w[app/lib extras]` / `"extras"` — plain relative strings.
        if let Some((_, rest)) = t.split_once("%w") {
            let mut chars = rest.chars();
            let close = match chars.next() {
                Some('[') => ']',
                Some('(') => ')',
                Some('{') => '}',
                _ => continue,
            };
            let inner = &rest[1..];
            let Some(end) = inner.find(close) else { continue };
            roots.extend(inner[..end].split_whitespace().map(str::to_string));
            continue;
        }
        if let Some((_, rest)) = t.split_once('"') {
            if let Some((value, _)) = rest.split_once('"') {
                if !value.is_empty() {
                    roots.push(value.to_string());
                }
            }
        }
    }
    roots
}

/// `get "up" => "rails/health#show"` — every `rails new` app's health
/// check (what a Kamal proxy probes) — names Rails' OWN
/// `Rails::HealthController`, which no app tree holds: the route
/// dispatched to nothing and `/up` answered 404. Written here as the
/// controller Rails ships (8.1: `render html:` of the green page; the
/// `rescue_from` → red 500 half needs a boot failure, which a one-shot
/// process reports by not answering at all). Synthesized only when a
/// route targets it and the app doesn't define its own. A namespaced
/// class, so only the targets that emit one receive it
/// (`project::target_files` drops it, with a warning, for the rest).
fn synthesize_rails_health_controller(app: &mut crate::App) {
    use super::routes::RAILS_HEALTH_CONTROLLER;
    let routed = crate::lower::routes::flatten_routes(app)
        .iter()
        .any(|r| r.controller.0.as_str() == RAILS_HEALTH_CONTROLLER);
    if !routed || app.controllers.iter().any(|c| c.name.0.as_str() == RAILS_HEALTH_CONTROLLER) {
        return;
    }
    let src = "class Rails::HealthController < ActionController::Base\n  def show\n    render html: \"<!DOCTYPE html><html><body style=\\\"background-color: green\\\"></body></html>\".html_safe\n  end\nend\n";
    if let Ok(Some(controller)) = super::controller::ingest_controller(src.as_bytes(), "<rails/health>") {
        app.controllers.push(controller);
    }
}

/// The controller the `to: redirect(...)` routes dispatch to: one
/// action per redirect, each answering the location Rails' routing
/// redirect would.
///
/// One deliberate divergence, and it is the reason the routing form
/// exists at all: Rails' `redirect("/x")` carries the request's query
/// string over to the target. A `redirect_to "/x"` does not, and
/// nothing in the synthesized action can see the query string to pass
/// on.
fn synthesize_redirect_controller(
    redirects: &[crate::dialect::RedirectRoute],
) -> crate::dialect::Controller {
    use crate::dialect::{Action, ControllerBodyItem, RenderTarget};
    use crate::expr::{Expr, ExprNode, Literal};
    use crate::span::Span;

    let body = redirects
        .iter()
        .map(|redirect| {
            // Built from Ruby source so the action body is ingested the
            // way a hand-written `redirect_to` would be.
            let location = if let Some(expression) = redirect.location.strip_prefix('\u{0}') {
                expression.to_string()
            } else if redirect.location_is_expression {
                redirect.location.clone()
            } else {
                redirect_location_source(&redirect.location)
            };
            let (location, multiline) = if redirect.keep_query {
                // Rails' options form keeps the request query. The
                // dispatcher stores it on the request object. An empty
                // query leaves the path unchanged; a path that already
                // has `?` is joined with `&`. A fragment stays after the
                // query: `/login#step` plus `x=1` is `/login?x=1#step`,
                // not `/login#step?x=1`.
                (
                    format!(
                        "q = ActionController::Current.request.query_string.to_s\n    parts = {location}.split(\"#\", 2)\n    base = parts[0]\n    joined = q == \"\" ? base : base + (base.include?(\"?\") ? \"&\" : \"?\") + q\n    parts.length == 1 ? joined : joined + \"#\" + parts[1]"
                    ),
                    true,
                )
            } else {
                (location, false)
            };
            let src = if multiline || location.contains('\n') || location.contains(';') {
                format!(
                    "def __redirect\n  location = begin\n    {location}\n  end\n  redirect_to(location, status: :{})\nend\n",
                    redirect_status_symbol(redirect.status),
                )
            } else {
                format!(
                    "def __redirect\n  redirect_to({}, status: :{})\nend\n",
                    location,
                    redirect_status_symbol(redirect.status),
                )
            };
            let body = crate::runtime_src::parse_methods(&src)
                .ok()
                .and_then(|m| m.into_iter().next())
                .map(|m| m.body)
                .expect("synthesized redirect action parses");
            let location_for_target = match &*body.node {
                ExprNode::Send { args, .. } => args[0].clone(),
                _ => Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Str { value: redirect.location.clone() } },
                ),
            };
            ControllerBodyItem::Action {
                action: Action {
                    name: redirect.action.clone(),
                    params: crate::ty::Row::default(),
                    opt_params: Vec::new(),
                    kw_params: Vec::new(),
                    kwrest_param: None,
                    block_param: None,
                    name_span: Span::synthetic(),
                    body,
                    // The action IS the redirect, which is what the
                    // render target says.
                    renders: RenderTarget::Redirect { to: location_for_target },
                    effects: crate::effect::EffectSet::pure(),
                },
                leading_comments: Vec::new(),
                leading_blank_line: false,
            }
        })
        .collect();

    crate::dialect::Controller {
        name: crate::ident::ClassId(Symbol::from(super::routes::REDIRECT_CONTROLLER)),
        // `ActionController::Base`, not the app's `ApplicationController`:
        // a routing redirect never enters the app's controller stack, so
        // the synthesized action must not pick up its filters either. An
        // app that authenticates in `ApplicationController` would
        // otherwise start challenging a redirect Rails answers
        // unconditionally.
        parent: Some(crate::ident::ClassId(Symbol::from("ActionController::Base"))),
        body,
        layout: crate::dialect::LayoutDecl::default(),
        sibling_classes: Vec::new(),
    }
}

/// A routing redirect's target as a Ruby string literal. Rails'
/// `redirect("/~%{username}")` fills each `%{name}` from the matched
/// path parameters, so the placeholder becomes `#{params[:name]}`.
/// (Rails also URI-escapes the value; the emitted action does not.)
fn redirect_location_source(location: &str) -> String {
    let mut out = String::from("\"");
    let mut rest = location;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("%{") {
            if let Some(close) = after.find('}') {
                let name = &after[..close];
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    out.push_str(&format!("#{{params[:{name}]}}"));
                    rest = &after[close + 1..];
                    continue;
                }
            }
        }
        let c = rest.chars().next().unwrap();
        match c {
            '"' | '\\' | '#' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
        rest = &rest[c.len_utf8()..];
    }
    out.push('"');
    out
}

/// The Rack symbol for a redirect status, so the synthesized
/// `redirect_to` passes the Symbol form `resolve_status` takes.
fn redirect_status_symbol(status: u16) -> &'static str {
    match status {
        300 => "multiple_choices",
        302 => "found",
        303 => "see_other",
        304 => "not_modified",
        307 => "temporary_redirect",
        308 => "permanent_redirect",
        _ => "moved_permanently",
    }
}

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
/// `"%Y%m%d%H%M%S"` here), so it is deliberately not folded in:
/// Date's format registry is not modeled, and sharing Time's table would
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

/// `X.prepend Y` / `X.include Y` in a `config/initializers/` file.
///
/// THE ONE INITIALIZER SHAPE THAT CHANGES METHOD LOOKUP. Everything
/// else an initializer does is configuration a lowering reads (time
/// formats, session store, autoload ignores); this one rewrites an
/// ancestor chain, so dropping it leaves a module defined and
/// unreachable. campfire's `turbo_streams_authorization.rb` is the
/// case: `Turbo::StreamsChannel.prepend RoomStreamsAreAuthorized` is
/// what makes `RoomMessagesChannel` the only door onto a room's message
/// stream, and without it the guard is in the tree but not in the
/// lookup.
///
/// TWO NESTINGS, because Rails apps write both: the call bare at the
/// top level, and wrapped in `Rails.application.config.to_prepare do …
/// end` (campfire's spelling — reloading in development re-runs it).
/// `to_run` is the same shape. The block is UNWRAPPED rather than
/// modeled: its body is a list of statements, and what it means to a
/// tree that boots once is exactly that list.
///
/// Deliberately NOT a general initializer evaluator. A call is
/// recognized only when the receiver is a constant, the method is
/// `prepend`/`include`, and the single argument is a constant — three
/// facts readable off the parse with nothing resolved. Anything else in
/// the file is ignored, exactly as it is today.
fn extract_module_mixins(source: &[u8], file: &str) -> Vec<crate::app::ModuleMixin> {
    use crate::app::{MixinKind, ModuleMixin};

    let result = super::prism::parse(source, file);
    let root = result.node();
    let src = String::from_utf8_lossy(source).into_owned();
    let Some(program) = root.as_program_node() else { return Vec::new() };

    let mut out = Vec::new();
    for stmt in initializer_statements(&program) {
        let Some(call) = stmt.as_call_node() else { continue };
        let kind = match super::util::constant_id_str(&call.name()) {
            "prepend" => MixinKind::Prepend,
            "include" => MixinKind::Include,
            _ => continue,
        };
        // A RECEIVER is required. `include Foo` with none is a mixin
        // into `main`, which is not a lookup change this tree can carry.
        let Some(recv) = call.receiver() else { continue };
        let Some(target) = constant_text(&recv, &src) else { continue };

        // Exactly one argument. `prepend A, B` is legal Ruby and the
        // corpus has never written it; taking only the single-argument
        // form keeps the recorded fact unambiguous.
        let Some(args) = call.arguments() else { continue };
        let args: Vec<_> = args.arguments().iter().collect();
        let [arg] = args.as_slice() else { continue };
        let Some(module) = constant_text(arg, &src) else { continue };

        out.push(ModuleMixin { target: Symbol::from(target), module: Symbol::from(module), kind });
    }
    out
}

/// What the app's boot adds to `ActionText::ContentHelper.allowed_attributes`
/// — the list Action Text's sanitizer (and campfire's `SanitizeAttributes`,
/// which reads it) allows — as literals, in the order Rails' unions leave
/// them.
///
/// Two contributors, read at compile time rather than replayed:
///
/// * The `lexxy` gem, when the lockfile has it: its engine's
///   `lexxy.sanitization` initializer (0.9.24) sets the list to the
///   defaults plus `controls poster data-language style value start`.
/// * The app's own assignment, in a `config/initializers/` or `lib/` file
///   (campfire's `lib/rails_ext/action_text_allowed_tags.rb`, required by
///   an initializer), bare or inside `to_prepare`:
///
///   ```ruby
///   ActionText::ContentHelper.allowed_attributes =
///     (ActionText::ContentHelper.allowed_attributes || defaults.sanitizer_allowed_attributes) |
///     ContentFilters::EDITOR_FORMATTING_ATTRIBUTES
///   ```
///
///   The CURRENT value and the defaults are what the runtime already
///   has; each other operand of `|` / `+` must be an Array literal or a
///   constant holding one. Anything else is a survey gap — the addition
///   is a computation this does not evaluate — rather than a guess.
///
/// (`allowed_tags` has the same shape and is not read: nothing in the
/// runtime consults Action Text's tag list.)
fn content_helper_attribute_additions<V: Vfs + ?Sized>(vfs: &V, dir: &Path, app: &App) -> Vec<String> {
    use crate::expr::{ExprNode, Literal};

    let mut out: Vec<String> = Vec::new();
    let add = |names: Vec<String>, out: &mut Vec<String>| {
        for n in names {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    };
    if app.gem_lock.as_ref().is_some_and(|l| l.has("lexxy")) {
        add(
            ["controls", "poster", "data-language", "style", "value", "start"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            &mut out,
        );
    }

    fn string_list(e: &crate::expr::Expr) -> Option<Vec<String>> {
        let ExprNode::Array { elements, .. } = &*e.node else { return None };
        elements
            .iter()
            .map(|el| match &*el.node {
                ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                _ => None,
            })
            .collect()
    }
    // The current value or the framework defaults — what the runtime's
    // own list already is.
    fn is_base(e: &crate::expr::Expr) -> bool {
        match &*e.node {
            ExprNode::Send { method, .. } => {
                matches!(method.as_str(), "allowed_attributes" | "sanitizer_allowed_attributes")
            }
            ExprNode::BoolOp { left, right, .. } => is_base(left) && is_base(right),
            ExprNode::Seq { exprs } if exprs.len() == 1 => is_base(&exprs[0]),
            _ => false,
        }
    }
    fn operands<'a>(e: &'a crate::expr::Expr, acc: &mut Vec<&'a crate::expr::Expr>) {
        match &*e.node {
            ExprNode::Send { recv: Some(l), method, args, block: None, .. }
                if matches!(method.as_str(), "|" | "+") && args.len() == 1 =>
            {
                operands(l, acc);
                operands(&args[0], acc);
            }
            ExprNode::Seq { exprs } if exprs.len() == 1 => operands(&exprs[0], acc),
            _ => acc.push(e),
        }
    }
    let resolve = |e: &crate::expr::Expr| -> Option<Vec<String>> {
        if let Some(list) = string_list(e) {
            return Some(list);
        }
        let ExprNode::Const { path } = &*e.node else { return None };
        let (last, owner) = path.split_last()?;
        let owner = owner.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
        let lc = app.library_classes.iter().find(|lc| lc.name.0.as_str() == owner)?;
        let (_, value) = lc.constants.iter().find(|(n, _)| n == last)?;
        string_list(value)
    };

    let mut files: Vec<PathBuf> = Vec::new();
    for sub in ["config/initializers", "lib"] {
        let d = dir.join(sub);
        if vfs.is_dir(&d) {
            files.extend(read_rb_files(vfs, &d).unwrap_or_default());
        }
    }
    for entry in files {
        let Ok(bytes) = vfs.read(&entry) else { continue };
        let file = entry.display().to_string();
        let result = super::prism::parse(&bytes, &file);
        let src = String::from_utf8_lossy(&bytes).into_owned();
        let root = result.node();
        let Some(program) = root.as_program_node() else { continue };
        for stmt in initializer_statements(&program) {
            let Some(call) = stmt.as_call_node() else { continue };
            if super::util::constant_id_str(&call.name()) != "allowed_attributes=" {
                continue;
            }
            let Some(recv) = call.receiver() else { continue };
            if constant_text(&recv, &src).as_deref() != Some("ActionText::ContentHelper") {
                continue;
            }
            let Some(args) = call.arguments() else { continue };
            let args: Vec<_> = args.arguments().iter().collect();
            let [arg] = args.as_slice() else { continue };
            let Ok(value) = super::expr::ingest_expr(arg, &file) else { continue };
            let mut ops = Vec::new();
            operands(&value, &mut ops);
            for op in ops {
                if is_base(op) {
                    continue;
                }
                match resolve(op) {
                    Some(list) => add(list, &mut out),
                    None => survey::record(&IngestError::Unsupported {
                        file: file.clone(),
                        message: "ActionText::ContentHelper.allowed_attributes addition is not a literal list or a constant holding one; the attributes it adds are not allowed by the emitted sanitizer".to_string(),
                    }),
                }
            }
        }
    }
    out
}

/// Does a layout write `stylesheet_link_tag :all` — the expansion that
/// walks the whole asset path, gems' stylesheets included? Read off the
/// layout SOURCES: views are not ingested yet where the stylesheet list
/// is built, and the call is a literal wherever an app writes it.
fn layouts_link_all_stylesheets<V: Vfs + ?Sized>(vfs: &V, dir: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| {
        let layouts = dir.join(root).join("views/layouts");
        if !vfs.is_dir(&layouts) {
            return false;
        }
        let Ok(entries) = vfs.read_dir(&layouts) else { return false };
        entries.iter().any(|entry| {
            vfs.read_to_string(entry)
                .map(|src| {
                    src.contains("stylesheet_link_tag :all")
                        || src.contains("stylesheet_link_tag(:all")
                })
                .unwrap_or(false)
        })
    })
}

/// Top-level statements, plus the body of any `to_prepare`/`to_run`
/// block, flattened into one list. One level of unwrapping is enough:
/// nesting a second config block inside the first is not a shape
/// Rails apps write, and guessing at it would be inventing a need.
fn initializer_statements<'a>(program: &ruby_prism::ProgramNode<'a>) -> Vec<Node<'a>> {
    let mut stmts: Vec<Node> = Vec::new();
    for stmt in program.statements().body().iter() {
        let mut unwrapped = false;
        if let Some(call) = stmt.as_call_node() {
            let name = super::util::constant_id_str(&call.name());
            if matches!(name, "to_prepare" | "to_run") {
                if let Some(body) = call
                    .block()
                    .and_then(|b| b.as_block_node())
                    .and_then(|b| b.body())
                    .and_then(|b| b.as_statements_node())
                {
                    stmts.extend(body.body().iter());
                    unwrapped = true;
                }
            }
        }
        if !unwrapped {
            stmts.push(stmt);
        }
    }
    stmts
}

/// `X.before_action :m[, only: :a | %i[a b]]` in a `config/initializers/`
/// file — the mixin's companion (see `extract_module_mixins`): a guard
/// mixed into a framework controller does nothing until that
/// controller is told to run it, and campfire writes the two lines
/// together in `active_storage_authentication.rb`.
///
/// The same discipline as the mixin reader: a constant receiver, a
/// Symbol method, and at most an `only:` of Symbols, all readable off
/// the parse. `except:`, a block, an `if:` — anything else — is not
/// recorded, and the lowering reports the target it never sees a
/// filter for.
fn extract_initializer_filters(source: &[u8], file: &str) -> Vec<crate::app::InitializerFilter> {
    use crate::app::InitializerFilter;

    let result = super::prism::parse(source, file);
    let root = result.node();
    let src = String::from_utf8_lossy(source).into_owned();
    let Some(program) = root.as_program_node() else { return Vec::new() };

    let mut out = Vec::new();
    for stmt in initializer_statements(&program) {
        let Some(call) = stmt.as_call_node() else { continue };
        if super::util::constant_id_str(&call.name()) != "before_action" {
            continue;
        }
        let Some(recv) = call.receiver() else { continue };
        let Some(target) = constant_text(&recv, &src) else { continue };
        let Some(args) = call.arguments() else { continue };
        let args: Vec<_> = args.arguments().iter().collect();
        let (method, options) = match args.as_slice() {
            [m] => (m, None),
            [m, o] => (m, Some(o)),
            _ => continue,
        };
        let Some(method) = super::util::symbol_value(method) else { continue };
        let mut only: Vec<Symbol> = Vec::new();
        if let Some(options) = options {
            let Some(hash) = options.as_keyword_hash_node() else { continue };
            let mut readable = true;
            for element in hash.elements().iter() {
                let Some(pair) = element.as_assoc_node() else { readable = false; break };
                if super::util::symbol_value(&pair.key()).as_deref() != Some("only") {
                    readable = false;
                    break;
                }
                let value = pair.value();
                if let Some(one) = super::util::symbol_value(&value) {
                    only.push(Symbol::from(one));
                } else if let Some(list) = value.as_array_node() {
                    for e in list.elements().iter() {
                        match super::util::symbol_value(&e) {
                            Some(s) => only.push(Symbol::from(s)),
                            None => { readable = false; break }
                        }
                    }
                } else {
                    readable = false;
                }
            }
            if !readable || only.is_empty() {
                continue;
            }
        }
        out.push(InitializerFilter { target: Symbol::from(target), method: Symbol::from(method), only });
    }
    out
}

/// The source text of a node that is a constant or a constant path
/// (`Foo`, `Turbo::StreamsChannel`), or None for anything else. Read as
/// TEXT rather than resolved: the receiver may name a class this tree
/// does not define, and reporting that gap needs the app's spelling.
fn constant_text(node: &Node, src: &str) -> Option<String> {
    if node.as_constant_read_node().is_none() && node.as_constant_path_node().is_none() {
        return None;
    }
    let loc = node.location();
    Some(src[loc.start_offset()..loc.end_offset()].to_string())
}

/// A registered format as its initializer spells it, before the lambda
/// form is parsed into IR.
enum TimeFormatSource {
    Strftime(String),
    Lambda { param: String, body: String },
}

/// Each assignment as its PATH SEGMENTS plus the value's source text —
/// `config.x.vapid.private_key = …` is `(["x", "vapid", "private_key"],
/// "ENV.fetch(…)")`. The path rather than the joined reader name because
/// the caller synthesizes a reader for an intermediate node too
/// (`config.x.vapid` read as a whole is an OrderedOptions in Rails), and
/// `x_vapid_private_key` cannot be split back into its segments — the
/// leaf has an underscore of its own.
fn extract_config_assignments(source: &[u8], file: &str) -> Vec<(Vec<String>, String)> {
    fn walk(stmts: Vec<ruby_prism::Node<'_>>, src: &str, out: &mut Vec<(Vec<String>, String)>) {
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
            let mut path = prefix;
            path.push(base.to_string());
            out.push((
                path,
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

/// The module wrapping `class Application < Rails::Application` in
/// config/application.rb — `Campfire` for campfire, `Lobsters` for
/// lobsters.
///
/// Textual and per-line, the same contract as the other
/// config/application.rb readers here: railtie soup is deliberately not
/// parsed. The first `module <Const>` line above an `Application <
/// Rails::Application` class is the namespace; a file that does not
/// spell both is not one this can read, and the runtime default stands.
fn extract_app_namespace(source: &[u8]) -> Option<String> {
    let src = String::from_utf8_lossy(source);
    let mut module: Option<String> = None;
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        if let Some(rest) = t.strip_prefix("module ") {
            let name = rest.split_whitespace().next().unwrap_or("").trim();
            if !name.is_empty() && module.is_none() {
                module = Some(name.to_string());
            }
            continue;
        }
        if t.contains("< Rails::Application") {
            return module;
        }
    }
    None
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

/// The MIME types a `config.active_storage.variable_content_types -=
/// %w[…]` line subtracts, in source order. Line-scanned like the
/// other config reads; the `%w[` literal may start on the next line
/// (campfire breaks after the `-=`), so the scan continues to the
/// closing bracket. Only the subtractive form: an app REPLACING the
/// list (`= %w[…]`) is not read, and keeps Rails' default.
fn extract_variable_content_type_exclusions(source: &[u8]) -> Vec<String> {
    let source = String::from_utf8_lossy(source);
    let mut out = Vec::new();
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        let Some(idx) = t.find("active_storage.variable_content_types") else { continue };
        let rest = t[idx + "active_storage.variable_content_types".len()..].trim_start();
        let Some(rest) = rest.strip_prefix("-=") else { continue };
        let mut text = rest.to_string();
        while !text.contains(']') {
            let Some(next) = lines.next() else { break };
            text.push(' ');
            text.push_str(next.trim());
        }
        let Some(open) = text.find("%w[") else { continue };
        let body = &text[open + 3..];
        let Some(close) = body.find(']') else { continue };
        out.extend(body[..close].split_whitespace().map(|s| s.to_string()));
    }
    out
}

/// `(Vips.block_untrusted(true) seen, operations named by Vips.block(
/// "<name>", true))` from an initializer. Line-shaped like the
/// content-type trim above: the two calls are one statement each, and
/// only the `true` arms are policy (`false` is libvips' default).
/// Parens are optional (`Vips.block_untrusted true`); a `#` comment
/// does not count. Named blocks accept double or single quotes.
fn extract_vips_loader_policy(source: &[u8]) -> (bool, Vec<String>) {
    let source = String::from_utf8_lossy(source);
    let mut untrusted = false;
    let mut blocked = Vec::new();
    for line in source.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        if let Some(rest) = t.strip_prefix("Vips.block_untrusted") {
            let arg = rest.trim().trim_start_matches('(').trim_end_matches(')').trim();
            if arg.starts_with("true") {
                untrusted = true;
            }
            continue;
        }
        if let Some(rest) = t.strip_prefix("Vips.block") {
            let rest = rest.trim_start();
            let Some(rest) = rest.strip_prefix('(') else { continue };
            let Some(close) = rest.find(')') else { continue };
            let mut parts = rest[..close].splitn(2, ',');
            let name = parts.next().unwrap_or("").trim().trim_matches('"').trim_matches('\'');
            let state = parts.next().unwrap_or("").trim();
            if !name.is_empty() && state == "true" {
                blocked.push(name.to_string());
            }
        }
    }
    (untrusted, blocked)
}

/// `<param>.default_per_page = N` inside a `Kaminari.configure do
/// |<param>| … end` block (top level or under `to_prepare`), the last
/// one winning as it does when Ruby runs the block. Read off the parse
/// rather than the lines, so an assignment in another config block of
/// the same file (`Rails.application.configure do |config|`) is not
/// mistaken for Kaminari's.
///
/// A value that is not a positive Integer literal (a constant, an ENV
/// read) is recognized but not evaluated: it is a survey gap, and this
/// file contributes no page size rather than a guessed one.
fn extract_kaminari_default_per_page(source: &[u8], file: &str) -> Option<u64> {
    let src = String::from_utf8_lossy(source);
    // Cheap skip before parsing: this runs over every initializer, and
    // most never name Kaminari.
    if !src.contains("Kaminari") {
        return None;
    }
    let result = super::prism::parse(source, file);
    let root = result.node();
    let program = root.as_program_node()?;
    let mut found = None;
    for stmt in initializer_statements(&program) {
        let Some(call) = stmt.as_call_node() else { continue };
        if super::util::constant_id_str(&call.name()) != "configure" {
            continue;
        }
        let Some(recv) = call.receiver() else { continue };
        if !matches!(constant_text(&recv, &src).as_deref(), Some("Kaminari" | "::Kaminari")) {
            continue;
        }
        let Some(block) = call.block().and_then(|b| b.as_block_node()) else { continue };
        let Some(param) = block
            .parameters()
            .and_then(|p| p.as_block_parameters_node())
            .and_then(|p| p.parameters())
            .and_then(|p| p.requireds().iter().next())
            .and_then(|p| p.as_required_parameter_node())
        else {
            continue;
        };
        let param = super::util::constant_id_str(&param.name());
        let Some(body) = block.body().and_then(|b| b.as_statements_node()) else { continue };
        for inner in body.body().iter() {
            let Some(assign) = inner.as_call_node() else { continue };
            if super::util::constant_id_str(&assign.name()) != "default_per_page=" {
                continue;
            }
            let on_param = assign
                .receiver()
                .and_then(|r| r.as_local_variable_read_node())
                .is_some_and(|r| super::util::constant_id_str(&r.name()) == param);
            if !on_param {
                continue;
            }
            let Some(args) = assign.arguments() else { continue };
            let args: Vec<_> = args.arguments().iter().collect();
            let [value] = args.as_slice() else { continue };
            let literal = value
                .as_integer_node()
                .and_then(|i| super::util::integer_i64(&i.value()))
                .and_then(|n| u64::try_from(n).ok())
                .filter(|&n| n > 0);
            if literal.is_none() {
                let loc = value.location();
                survey::record(&IngestError::Unsupported {
                    file: file.to_string(),
                    message: format!(
                        "Kaminari default_per_page is not a positive Integer literal (`{}`); the emitted app does not apply it",
                        &src[loc.start_offset()..loc.end_offset()]
                    ),
                });
            }
            found = literal;
        }
    }
    found
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
        if let Some(source) = read_or_ledger(vfs, &helper_rb)? {
            if let Some(classes) = unwrap_or_record(ingest_library_classes(
                &source,
                &helper_rb.display().to_string(),
            ))? {
                for lc in classes {
                    wanted.extend(lc.includes.iter().map(|c| c.0.clone()));
                }
            }
        }
    }
    // The Rails 8 authentication generator's spelling: the helper file
    // includes ITSELF, at its foot —
    //   ActiveSupport.on_load(:action_dispatch_integration_test) do
    //     include SessionTestHelper
    //   end
    // — and `test/test_helper.rb` only `require_relative`s it.
    for entry in read_rb_files(vfs, &helpers_dir)? {
        let Some(source) = read_or_ledger(vfs, &entry)? else { continue };
        for name in on_load_test_includes(&source, &entry.display().to_string()) {
            if !wanted.contains(&name) {
                wanted.push(name);
            }
        }
    }
    if wanted.is_empty() {
        return Ok(Vec::new());
    }

    let mut out: Vec<LibraryClass> = Vec::new();
    for entry in read_rb_files(vfs, &helpers_dir)? {
        let Some(source) = read_or_ledger(vfs, &entry)? else { continue };
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

/// Modules a file mixes into the test cases through a top-level
/// `ActiveSupport.on_load(:action_dispatch_integration_test |
/// :active_support_test_case) do include M end`. Every test module is a
/// spliced test case here, so both hooks reach the same place.
fn on_load_test_includes(source: &[u8], file: &str) -> Vec<Symbol> {
    let result = super::prism::parse(source, file);
    let root = result.node();
    let stmts = root
        .as_program_node()
        .map(|p| p.statements().body().iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let src = String::from_utf8_lossy(source).into_owned();
    let text = |loc: ruby_prism::Location<'_>| src[loc.start_offset()..loc.end_offset()].to_string();
    let mut out = Vec::new();
    for stmt in stmts {
        let Some(call) = stmt.as_call_node() else { continue };
        if super::util::constant_id_str(&call.name()) != "on_load" {
            continue;
        }
        if call.receiver().map(|r| text(r.location())).as_deref() != Some("ActiveSupport") {
            continue;
        }
        let hook = call
            .arguments()
            .and_then(|a| a.arguments().iter().next())
            .and_then(|a| a.as_symbol_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned()));
        if !matches!(hook.as_deref(), Some("action_dispatch_integration_test" | "active_support_test_case")) {
            continue;
        }
        let Some(block) = call.block().and_then(|b| b.as_block_node()) else { continue };
        let Some(body) = block.body().and_then(|b| b.as_statements_node()) else { continue };
        for inner in body.body().iter() {
            let Some(inc) = inner.as_call_node() else { continue };
            if inc.receiver().is_some() || super::util::constant_id_str(&inc.name()) != "include" {
                continue;
            }
            for arg in inc.arguments().into_iter().flat_map(|a| a.arguments().iter()) {
                if arg.as_constant_read_node().is_some() || arg.as_constant_path_node().is_some() {
                    out.push(Symbol::from(text(arg.location()).trim_start_matches("::")));
                }
            }
        }
    }
    out
}

/// Run the app-wide `ActiveSupport::TestCase` setup ahead of a test
/// module's own — the order Rails' setup callbacks fire in (a
/// superclass's before a subclass's). The module's `setup` is inlined
/// at the head of every test by the lowerer, so prepending here puts
/// the app's statements first in each.
fn splice_test_case_setup(tm: &mut TestModule, case_setup: &crate::expr::Expr) {
    use crate::expr::{Expr, ExprNode};
    let mut exprs = match &*case_setup.node {
        ExprNode::Seq { exprs } => exprs.clone(),
        _ => vec![case_setup.clone()],
    };
    if let Some(own) = tm.setup.take() {
        match *own.node {
            ExprNode::Seq { exprs: own_exprs } => exprs.extend(own_exprs),
            _ => exprs.push(own),
        }
    }
    tm.setup = Some(Expr::new(crate::span::Span::synthetic(), ExprNode::Seq { exprs }));
}

/// Copy shared helper methods onto a test class, the test's own
/// definitions winning on a name collision (Ruby resolves the class
/// body ahead of an included module).
///
/// CONSTANTS COME TOO, and they have to: a helper module's method body
/// reads its own constants by BARE name (Ruby resolves them lexically,
/// inside the module), and once the method is spliced onto the test
/// class that bare name has nowhere to land. campfire's
/// `DnsTestHelper#stub_web_push_dns_resolution` is one line —
/// `stub_dns_resolution(WEB_PUSH_PUBLIC_TEST_IP)` — and without the
/// constant it is a NameError in the SETUP of three test files, 28
/// tests, none of which ever runs a line of its own.
///
/// The test bodies spell the same constant the other way,
/// `DnsTestHelper::WEB_PUSH_PUBLIC_TEST_IP`, because an `include` does
/// NOT bring a module's constants into the including class's lexical
/// scope — so both spellings are real Ruby and both have to resolve.
/// The qualified one is rewritten to the bare one here rather than by
/// keeping the module around, so there is exactly one definition of the
/// value in the emitted file.
fn splice_test_helpers(tm: &mut TestModule, helpers: &[LibraryClass]) {
    // Candidate pool: every shared instance method the class does not
    // already define itself (its own definition wins, as Ruby's class
    // body beats an included module).
    let mut pool: Vec<&crate::dialect::MethodDef> = Vec::new();
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
            pool.push(m);
        }
    }

    // Every name the class's OWN code mentions, as reachability roots.
    let mut wanted: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
    let mut consts: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
    for t in &tm.tests {
        collect_referenced_names(&t.body, &mut wanted, &mut consts);
    }
    if let Some(setup) = &tm.setup {
        collect_referenced_names(setup, &mut wanted, &mut consts);
    }
    for h in &tm.helpers {
        collect_referenced_names(&h.body, &mut wanted, &mut consts);
    }
    for ic in &tm.inner_classes {
        for m in &ic.methods {
            collect_referenced_names(&m.body, &mut wanted, &mut consts);
        }
    }

    // Fixpoint — a helper that IS reached pulls in the helpers it calls.
    let mut taken: Vec<crate::dialect::MethodDef> = Vec::new();
    loop {
        let mut grew = false;
        for m in &pool {
            if taken.iter().any(|t| t.name == m.name) || !wanted.contains(&m.name) {
                continue;
            }
            collect_referenced_names(&m.body, &mut wanted, &mut consts);
            let mut m = (*m).clone();
            m.enclosing_class = Some(tm.name.0.clone());
            taken.push(m);
            grew = true;
        }
        if !grew {
            break;
        }
    }
    tm.helpers.extend(taken);

    // Constants follow the methods that survived. A constant's own value
    // can name another, so this settles too.
    loop {
        let mut grew = false;
        for lc in helpers {
            for (name, value) in &lc.constants {
                if !consts.contains(name) || tm.constants.iter().any(|(n, _)| n == name) {
                    continue;
                }
                collect_referenced_names(value, &mut wanted, &mut consts);
                tm.constants.push((name.clone(), value.clone()));
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    let qualified: Vec<(Symbol, Symbol)> = helpers
        .iter()
        .flat_map(|lc| {
            lc.constants
                .iter()
                .map(|(n, _)| (lc.name.0.clone(), n.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    if qualified.is_empty() {
        return;
    }
    for t in &mut tm.tests {
        unqualify_helper_constants(&mut t.body, &qualified);
    }
    if let Some(setup) = tm.setup.as_mut() {
        unqualify_helper_constants(setup, &qualified);
    }
    for h in &mut tm.helpers {
        unqualify_helper_constants(&mut h.body, &qualified);
    }
}

/// Every method-ish and constant name an expression mentions.
///
/// Over-approximates on purpose. A bare zero-arg call can reach the IR
/// as `Var` rather than `Send` depending on how the source spelled it,
/// so both are collected; the cost of a false positive is one helper
/// that stays, and the cost of a false negative is a NameError at
/// runtime. Constants take the LAST path segment, which catches both
/// `WEB_PUSH_PUBLIC_TEST_IP` and `DnsTestHelper::WEB_PUSH_PUBLIC_TEST_IP`
/// — the two spellings the splice has to keep working (see
/// `splice_test_helpers`).
fn collect_referenced_names(
    expr: &crate::expr::Expr,
    methods: &mut std::collections::HashSet<Symbol>,
    consts: &mut std::collections::HashSet<Symbol>,
) {
    use crate::expr::ExprNode as EN;
    match &*expr.node {
        EN::Send { method, .. } => {
            methods.insert(method.clone());
        }
        EN::Var { name, .. } => {
            methods.insert(name.clone());
        }
        EN::Const { path } => {
            if let Some(last) = path.last() {
                consts.insert(last.clone());
            }
        }
        _ => {}
    }
    expr.node
        .for_each_child(&mut |c| collect_referenced_names(c, methods, consts));
}

/// `DnsTestHelper::WEB_PUSH_PUBLIC_TEST_IP` -> `WEB_PUSH_PUBLIC_TEST_IP`
/// for every (module, constant) pair `splice_test_helpers` just lifted
/// onto the test class. Two segments only — a deeper path is some other
/// module's constant that happens to share a first segment.
fn unqualify_helper_constants(
    e: &mut crate::expr::Expr,
    qualified: &[(Symbol, Symbol)],
) {
    if let crate::expr::ExprNode::Const { path } = &mut *e.node {
        if path.len() == 2
            && qualified
                .iter()
                .any(|(m, c)| m == &path[0] && c == &path[1])
        {
            let name = path[1].clone();
            *path = vec![name];
        }
    }
    e.node
        .for_each_child_mut(&mut |c| unqualify_helper_constants(c, qualified));
}

// Not left on the declaring base: an abstract base's `enum :state` reads through the subclass's own column reader, which has to know the mapping.
fn inherit_enums(models: &mut [crate::dialect::Model]) {
    let declared: std::collections::HashMap<crate::ident::ClassId, (Option<crate::ident::ClassId>, indexmap::IndexMap<crate::ident::Symbol, Vec<(String, crate::expr::Literal)>>)> =
        models.iter().map(|m| (m.name.clone(), (m.parent.clone(), m.enums.clone()))).collect();
    let defaults: std::collections::HashMap<crate::ident::ClassId, indexmap::IndexMap<crate::ident::Symbol, crate::expr::Literal>> =
        models.iter().map(|m| (m.name.clone(), m.enum_defaults.clone())).collect();
    for m in models.iter_mut() {
        let mut current = m.parent.clone();
        for _ in 0..32 {
            let Some(p) = current else { break };
            let Some((grand, enums)) = declared.get(&p) else { break };
            for (col, mapping) in enums {
                m.enums.entry(col.clone()).or_insert_with(|| mapping.clone());
            }
            if let Some(d) = defaults.get(&p) {
                for (col, v) in d {
                    m.enum_defaults.entry(col.clone()).or_insert_with(|| v.clone());
                }
            }
            current = grand.clone();
        }
    }
}
