//! Whole-app orchestrator: walks a Rails app directory, calls the
//! per-domain ingesters, and assembles an `App`. Also owns the small
//! DSLs that don't warrant their own submodule — `config/importmap.rb`
//! and the `.rb` / `.yml` / `.erb` file walkers.
//!
//! All filesystem access goes through the [`Vfs`] trait so that the
//! ingest pipeline drives both the on-disk Rails app (CLI) and an
//! in-memory tree (wasm transpile entry point). [`ingest_app`] is the
//! convenience wrapper for the disk case.

use std::cell::OnceCell;
use crate::det::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

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
    ClassKind, ConcernClassMethodSpans, ConcernEnumDecl, classify_class_file,
    ingest_concern_class_method_spans, ingest_concern_filters,
    ingest_concern_included_redefs, ingest_concern_model_items_with_constants,
    ingest_helper_method_names, ingest_library_classes, ingest_nested_library_classes,
    ingest_rails_application_singleton_methods,
};
use super::model::ingest_model_with_enum_constants;
use super::routes::{EngineRouteSource, RouteHelperSource, ingest_routes_with_engines};
use super::schema::{ingest_migration, ingest_schema};
use super::structure_sql::ingest_structure_sql;
use super::test::{ingest_test_bases, ingest_test_files};
use super::view::{ViewEngine, ingest_template};
use super::survey::{self, unwrap_or_record};
use super::{IngestError, IngestResult};

use crate::ingest::prism_api::PrismLocation;
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
pub fn ingest_app_from_tree(tree: crate::vfs::FileTree) -> IngestResult<App> {
    ingest_app_with_vfs(&MapVfs::new(tree), Path::new(""))
}

/// The actual whole-app walker. Generic over [`Vfs`] so it can read
/// from disk or from an in-memory map without code duplication.
/// What `inflections.rb` ingest does with one expression: the single
/// definition of which `inflect.<m>(…)` calls the app's inflections carry,
/// shared with the initializer ledger so the two cannot drift.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum InflectEffect {
    /// Not a call on the block parameter `inflect`.
    NotInflect,
    /// An `inflect.<m>` call no rule reads (`human`, `clear`, a wrong arity).
    Ignored,
    /// Carried into `AppInflections`.
    Carried,
    /// A `plural`/`singular` regex rule: counted, reported as one survey gap.
    RegexRule,
}

/// Apply `e` to `out` when it is an `inflect.<m>(…)` call, and say what happened.
pub(super) fn apply_inflect_send(
    e: &crate::expr::Expr,
    out: &mut crate::naming::AppInflections,
) -> InflectEffect {
    use crate::expr::{Expr, ExprNode, Literal};
    fn strings(e: &Expr, out: &mut Vec<String>) {
        match &*e.node {
            ExprNode::Lit { value: Literal::Str { value } } => out.push(value.clone()),
            ExprNode::Lit { value: Literal::Sym { value } } => out.push(value.as_str().to_string()),
            ExprNode::Array { elements, .. } => elements.iter().for_each(|x| strings(x, out)),
            _ => {}
        }
    }
    let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node else {
        return InflectEffect::NotInflect;
    };
    if !matches!(&*recv.node, ExprNode::Var { name, .. } if name.as_str() == "inflect") {
        return InflectEffect::NotInflect;
    }
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
        "plural" | "singular" => {
            out.regex_rules += 1;
            return InflectEffect::RegexRule;
        }
        _ => return InflectEffect::Ignored,
    }
    InflectEffect::Carried
}

/// `config/initializers/inflections.rb` → the app's inflection
/// declarations: the `inflect.irregular` / `uncountable` / `acronym`
/// calls inside the `ActiveSupport::Inflector.inflections(:en) do
/// |inflect| … end` block, with literal arguments. `inflect.plural` /
/// `inflect.singular` take regexes the port cannot carry; they are
/// counted and, under survey mode, reported as a gap rather than
/// silently dropped. Any parse trouble yields the defaults — the
/// initializer is optional and often all comments.
pub fn ingest_inflections<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> crate::naming::AppInflections {
    use crate::expr::Expr;
    let path = dir.join("config/initializers/inflections.rb");
    let mut out = crate::naming::AppInflections::default();
    let Ok(source) = vfs.read_to_string(&path) else { return out };
    let file = path.to_string_lossy().into_owned();
    let Ok(program) = super::expr::ingest_ruby_program(&source, &file) else { return out };

    fn walk(e: &Expr, out: &mut crate::naming::AppInflections) {
        apply_inflect_send(e, out);
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

/// Does Ruby source `text` read the top-level constant `name` as code: a
/// bare `X`, a rooted `::X`, or the `X` that starts a path `X::Y`?
fn names_root_constant(text: &str, name: &str) -> bool {
    struct Reads<'n> {
        name: &'n str,
        found: bool,
    }
    impl<'pr> ruby_prism::Visit<'pr> for Reads<'_> {
        fn visit_constant_read_node(&mut self, node: &ruby_prism::ConstantReadNode<'pr>) {
            self.found |= super::util::constant_id_str(&node.name()) == self.name;
        }
        fn visit_constant_path_node(&mut self, node: &ruby_prism::ConstantPathNode<'pr>) {
            if node.parent().is_none() {
                self.found |= node.name().is_some_and(|id| super::util::constant_id_str(&id) == self.name);
            }
            ruby_prism::visit_constant_path_node(self, node);
        }
        // A `module X` / `class X` declaration names X without reading it
        // (`module Shops` around an app model reopens the namespace): only
        // a namespace path it nests under (`class A::X` reads A) is a read.
        fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
            if let Some(parent) = node.constant_path().as_constant_path_node().and_then(|p| p.parent()) {
                self.visit(&parent);
            }
            if let Some(body) = node.body() {
                self.visit(&body);
            }
        }
        fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
            if let Some(parent) = node.constant_path().as_constant_path_node().and_then(|p| p.parent()) {
                self.visit(&parent);
            }
            if let Some(superclass) = node.superclass() {
                self.visit(&superclass);
            }
            if let Some(body) = node.body() {
                self.visit(&body);
            }
        }
    }
    let parsed = super::prism::parse_silent(text.as_bytes());
    let mut reads = Reads { name, found: false };
    ruby_prism::Visit::visit(&mut reads, &parsed.node());
    reads.found
}

/// Top-level namespaces the framework defines before any app file loads.
pub(super) const FRAMEWORK_ROOTS: &[&str] = &[
    "Rails", "ActiveRecord", "ActiveSupport", "ActiveModel", "ActiveJob",
    "ActiveStorage", "ActionController", "ActionDispatch", "ActionView",
    "ActionMailer", "ActionMailbox", "ActionCable", "ActionText", "Rack",
];

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
            if !(rel.starts_with("app/") || rel.starts_with("lib/")) {
                return false;
            }
            // In Ruby, a comment or a string naming it is not a reference.
            if rel.ends_with(".rb") {
                return f.text.contains(name) && names_root_constant(&f.text, name);
            }
            // A template is not Ruby to parse, so its text is matched.
            f.text.match_indices(name).any(|(i, _)| {
                let head = &f.text[..i];
                let rooted = head.strip_suffix("::").is_some_and(|h| {
                    !h.chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':')
                });
                let before = head.chars().next_back();
                let after = f.text[i + name.len()..].chars().next();
                // Not only `X.` / `X::`: forem reads `ApplicationConfig["KEY"]`.
                (rooted || !before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':'))
                    && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
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
    // Again with the installed gems the code needs from source, until no
    // new one appears: a gem brought in can need another (typeid's uuid7).
    let mut auto: Vec<String> = Vec::new();
    loop {
        let marks = (super::survey::mark(), super::prism::mark());
        let mut app = ingest_rails_app(vfs, dir, &auto)?;
        let more: Vec<String> = super::auto_source_gems::candidates(&app, vfs, dir)
            .into_iter()
            .filter(|g| !auto.contains(g))
            .collect();
        if more.is_empty() || auto.len() >= 32 {
            app.auto_source_gems = auto.into_iter().filter(|g| app.gem_lock.as_ref().is_some_and(|l| l.source.contains(g))).collect();
            return Ok(app);
        }
        // Not the superseded pass's parse errors and survey gaps as well: the next pass reports them again.
        super::survey::rewind(marks.0);
        super::prism::rewind(marks.1);
        auto.extend(more);
    }
}

fn ingest_rails_app<V: Vfs + ?Sized>(vfs: &V, dir: &Path, auto_source_gems: &[String]) -> IngestResult<App> {
    super::sources::reset();
    super::each_validator::reset();
    let _source_root = super::sources::set_root(dir);
    let config = roundhouse_config(vfs, dir)?;
    let extensions = project_extensions(vfs, dir, &config)?;
    if let Some(mode) = config.spinel.int_overflow.as_deref()
        && !matches!(mode, "raise" | "wrap" | "promote")
    {
        return Err(IngestError::Parse {
            file: dir.join("roundhouse.yml").display().to_string(),
            message: format!("spinel.int_overflow must be raise, wrap or promote, not {mode:?}"),
        });
    }
    let spinel_int_overflow = config.spinel.int_overflow.clone().filter(|mode| mode != "raise");
    let lock = vfs.read_to_string(&dir.join("Gemfile.lock")).ok().map(|text| {
        let mut lock = crate::gems::Lockfile::parse(&text);
        if let Ok(gemfile) = vfs.read_to_string(&dir.join("Gemfile")) {
            lock.gemfile_requires = crate::gems::gemfile_requires(&gemfile);
        }
        lock
    });
    let mut source_gems = super::source_gems::resolve(vfs, dir, lock.as_ref(), &extensions.source_gems)
        .map_err(|message| IngestError::Parse { file: dir.join("roundhouse.yml").display().to_string(), message })?;
    // Not fatal for an automatic one: the gem stays a reported boundary, as it was.
    let mut auto_sourced: Vec<String> = Vec::new();
    for gem in auto_source_gems {
        if let Ok(mut resolved) = super::source_gems::resolve(vfs, dir, lock.as_ref(), std::slice::from_ref(gem)) {
            source_gems.append(&mut resolved);
            auto_sourced.push(gem.clone());
        }
    }
    // One walk of an unchanging tree: remember what each path answered.
    let cached_vfs = crate::vfs::CachedVfs::new(vfs);
    let mount_vfs = super::source_gems::MountVfs { inner: &cached_vfs, gems: &source_gems };
    let mut path_gems = path_gem_dirs(&mount_vfs, dir);
    path_gems.extend(source_gems.iter().map(|gem| gem.mount.clone()));
    let source_vfs = PathGemVfs::new(&mount_vfs, dir, &path_gems);
    let factory_vfs = super::module_factory::expand(&source_vfs, dir);
    let vfs = &factory_vfs;
    let engine_routes = engine_route_sources(vfs, dir, &path_gems);
    let additional_test_paths = additional_test_paths(dir, &config)?;
    validate_additional_test_paths(vfs, dir, &additional_test_paths)?;
    let roots = app_roots(vfs, dir, &path_gems);
    let lib_ignores: Vec<String> = vfs
        .read(&dir.join("config/application.rb"))
        .ok()
        .map(|s| extract_autoload_lib_ignores(&s))
        .unwrap_or_default()
        .into_iter()
        .filter(|ignored| !lib_dir_is_explicitly_required(vfs, dir, ignored))
        .collect();
    // Helper-source scanning reads and compiles a second copy of app sources.
    // Defer it until an engine mount is accepted and needs proxy diagnostics;
    // the same snapshot serves every accepted mount in this route set.
    let helper_sources = OnceCell::new();
    let load_helper_sources = || {
        route_helper_sources(
            vfs,
            dir,
            &path_gems,
            &engine_routes,
            &roots,
            &additional_test_paths,
            &lib_ignores,
        )
    };
    let mut app = App::new();
    app.extensions = extensions;
    app.spinel_int_overflow = spinel_int_overflow;
    // `enum` columns declared inside a concern's `included do`, keyed by
    // the module. Local rather than a field on `App`: they exist only
    // until the splice folds them into each including model's own
    // `enums` table, and nothing downstream reads them by module.
    let mut concern_enums: Vec<(crate::ident::ClassId, Vec<ConcernEnumDecl>)> = Vec::new();

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
            let mut lock = crate::gems::Lockfile::parse(&text);
            if let Ok(gemfile) = vfs.read_to_string(&dir.join("Gemfile")) {
                lock.gemfile_requires = crate::gems::gemfile_requires(&gemfile);
            }
            lock.source = app.extensions.source_gems.clone();
            lock.source.extend(auto_sourced.iter().cloned());
            app.gem_lock = Some(lock);
            if let Some(lock) = &app.gem_lock {
                app.gem_boundary = super::rbi::load_gem_boundary(vfs, dir, lock, &app.extensions);
            }
        }
    }

    if let Ok(entries) = vfs.read_dir(&dir.join("config/environments")) {
        let mut envs: Vec<String> = entries
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "rb"))
            .filter(|p| vfs.read_to_string(p).is_ok_and(|text| sets_assume_ssl(&text)))
            .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
            .collect();
        envs.sort();
        app.assume_ssl_envs = envs;
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
    app.app_roots = roots.iter().map(|r| r.display().to_string()).collect();
    // A namespace's `table_name_prefix` has to be known BEFORE the model
    // it prefixes is ingested, and file order does not guarantee that
    // (`push/subscription.rb` may be read before `push.rb`). One cheap
    // pre-pass over the same files, so the fact is complete when the
    // models loop starts.
    // Hoisted above the models pre-pass: the base set below has to
    // cover BOTH trees before either is classified, and the support
    // roots need this to be enumerated.
    let ignored_lib_file = |entry: &Path| {
        entry.strip_prefix(dir.join("lib")).is_ok_and(|rel| {
            rel.components().next().is_some_and(|c| {
                lib_ignores.iter().any(|ig| c.as_os_str() == ig.as_str())
            })
        })
    };

    let mut table_prefixes = super::model::TablePrefixes::default();
    // Action Text engine `isolate_namespace` → `action_text_` prefix.
    // Writebook's Markdown model lives under `module ActionText` without
    // an app-declared `table_name_prefix`, but its schema table is
    // `action_text_markdowns` (not `markdowns`). Seed the framework
    // prefix so ordinary model ingest matches the gem.
    table_prefixes.insert("ActionText".to_string(), "action_text_".to_string());
    // An app engine's own `isolate_namespace` (in its `lib/`): a prefix
    // the module's declared `table_name_prefix` overrides.
    let mut isolated_prefixes = super::model::TablePrefixes::default();
    // Each class's own `self.table_name_prefix =`, for the models that
    // inherit it from an abstract base.
    let mut class_prefixes = super::model::TablePrefixes::default();
    // Qualified enum arrays can live in a later file (e.g. a service
    // module). Collect literal inputs before expanding any model DSL.
    let mut enum_constants = super::model::EnumConstants::default();
    let mut enum_input_files = crate::det::HashSet::default();
    // The same pre-pass answers a second question: which classes are
    // ActiveRecord bases. A model descending through the app's own
    // abstract base was classified a library class and lost its DSL,
    // and one file's AST cannot resolve that — the base is declared in
    // another file, possibly later.
    let mut model_bases = super::library_class::ModelBases::new();
    // A model subclass's own validations, placed once every class is read.
    let mut sti_validations = Vec::new();
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
            class_prefixes.extend(super::model::ingest_class_table_prefixes(&source, &entry.display().to_string()));
            model_bases.record(&source, &mut base_pairs);
            model_bases.record_tableless(&source);
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
            isolated_prefixes.extend(super::model::ingest_isolated_namespace_prefixes(&source, &entry.display().to_string()));
            class_prefixes.extend(super::model::ingest_class_table_prefixes(&source, &entry.display().to_string()));
            model_bases.record(&source, &mut base_pairs);
            if sub != "lib" || !ignored_lib_file(&entry) {
                enum_constants.record(&source, &entry.display().to_string());
                enum_input_files.insert(entry);
            }
        }
    }
    for (module, prefix) in isolated_prefixes {
        table_prefixes.entry(module).or_insert(prefix);
    }
    let base_prefixes = super::model::inherited_table_prefixes(&class_prefixes, &base_pairs);
    // What runs at boot, before models load and routes are drawn: a
    // constant an initializer appends to, replaces or reassigns has that
    // value when a validation or a constraint reads it, not the input's.
    let boot_files = boot_source_files(vfs, dir, &enum_input_files)?;
    for file in &boot_files {
        if let Some(text) = read_or_ledger(vfs, file)? {
            enum_constants.record_boot(&text, &file.display().to_string());
        }
    }
    enum_constants.finish();
    model_bases.close_over(&base_pairs);
    model_bases.close_over_tableless();
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
                            &source, &path_str, &app.schema, &table_prefixes, &base_prefixes, &enum_constants,
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
                                unwrap_or_record(ingest_nested_library_classes(&source, &path_str, outer.0.as_str()))?
                            {
                                let nested = nested_under(&outer, classes);
                                let (concern_items, concern_enum_decls, concern_guarded) =
                                    ingest_concern_model_items_with_constants(
                                        &source, &path_str, &enum_constants,
                                    );
                                for (id, name) in ingest_concern_included_redefs(&source, &path_str) {
                                    app.concern_included_redefs.entry(id).or_default().push(name);
                                }
                                append_concern_items(&mut app, concern_items, concern_guarded, |id| {
                                    nested.iter().any(|class| class.name == *id)
                                });
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
                        // After the file is registered, so the spans name it.
                        sti_validations.extend(super::sti_validations::harvest(&source, &path_str));
                        // Concern modules (app/models/concerns/…) also
                        // carry `included do` declarations that belong
                        // to every includer: filters (controller-side)
                        // and model DSL (associations/scopes).
                        if let Some(concern_filters) =
                            unwrap_or_record(ingest_concern_filters(&source, &path_str))?
                        {
                            app.concern_filters.extend(concern_filters);
                        }
                        let (concern_items, concern_enum_decls, concern_guarded) =
                            ingest_concern_model_items_with_constants(
                                &source, &path_str, &enum_constants,
                            );
                        for (id, name) in ingest_concern_included_redefs(&source, &path_str) {
                            app.concern_included_redefs.entry(id).or_default().push(name);
                        }
                        append_concern_items(&mut app, concern_items, concern_guarded, |_| true);
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
    let gem_libs: Vec<PathBuf> = path_gems.iter().map(|gem| gem.join("lib")).collect();
    app.boot_gem_entries = boot_gem_entries(vfs, dir, lock.as_ref(), &path_gems);
    let mut support_seen: crate::det::HashSet<PathBuf> = crate::det::HashSet::default();
    let boot_required = boot_required_files(vfs, dir);
    let mut support_batches: Vec<(String, PathBuf, Vec<PathBuf>)> = Vec::new();
    for sub in support_roots(vfs, dir, &roots, &path_gems, &lib_ignores) {
        let support_dir = dir.join(&sub);
        if !vfs.is_dir(&support_dir) {
            continue;
        }
        let Ok(entries) = read_rb_files(vfs, &support_dir) else { continue };
        support_batches.push((sub, support_dir, entries));
    }
    // Last, so a file a support root already reached is ingested once.
    support_batches.push((String::new(), dir.to_path_buf(), boot_required.clone()));
    let mut held_aliases: Vec<super::inherited_alias::Held> = Vec::new();
    for (sub, support_dir, entries) in support_batches {
        let sub = sub.as_str();
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
                    &source, &path_str, &app.schema, &table_prefixes, &base_prefixes, &enum_constants,
                    &model_bases,
                ) {
                    Ok(Some(model)) => {
                        let outer = model.name.clone();
                        app.models.push(model);
                        // Classes nested in the model's body are classes
                        // of their own, exactly as under `app/models` —
                        // and, same as there, the plain (non-nested)
                        // walk would re-walk the model's own class with
                        // none of its synthesized methods in scope, so
                        // an alias naming one (an enum predicate, say)
                        // would hard-fail here and drop every genuinely
                        // nested class in the file too.
                        if let Ok(classes) = ingest_nested_library_classes(&source, &path_str, outer.0.as_str()) {
                            let nested = nested_under(&outer, classes);
                            let (concern_items, concern_enum_decls, concern_guarded) =
                                ingest_concern_model_items_with_constants(
                                    &source, &path_str, &enum_constants,
                                );
                            for (id, name) in ingest_concern_included_redefs(&source, &path_str) {
                                app.concern_included_redefs.entry(id).or_default().push(name);
                            }
                            append_concern_items(&mut app, concern_items, concern_guarded, |id| {
                                nested.iter().any(|class| class.name == *id)
                            });
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
            let (ingested, mut aliases) =
                super::inherited_alias::capturing(|| ingest_library_classes(&source, &path_str));
            match ingested {
                Ok(mut classes) => {
                    // A gem's file runs more than its declarations when it
                    // loads (`file_program`).
                    if !gem_libs.iter().any(|lib| entry.starts_with(lib)) {
                        // An app file loads at boot; the gem files it
                        // requires load with it.
                        for feature in super::file_program::literal_requires(&source, &path_str) {
                            let file = if feature.ends_with(".rb") { feature } else { format!("{feature}.rb") };
                            let found = gem_libs.iter().map(|lib| lib.join(&file)).find(|path| vfs.exists(path));
                            if let Some(found) = found.map(|f| f.display().to_string()) {
                                if !app.boot_gem_entries.contains(&found) {
                                    app.boot_gem_entries.push(found);
                                }
                            }
                        }
                    }
                    if gem_libs.iter().any(|lib| entry.starts_with(lib)) {
                        let (branched, branch_aliases) = super::inherited_alias::capturing(|| {
                            super::library_class::ingest_file_branch_classes(&source, &path_str)
                        });
                        for span in branch_aliases {
                            if !aliases.contains(&span) {
                                aliases.push(span);
                            }
                        }
                        if let Ok(branched) = branched {
                            let branched: Vec<_> = branched.into_iter().filter(|m| !classes.iter().any(|c| c.name == m.name)).collect();
                            classes.extend(branched);
                        }
                        if let Ok(empty) = super::library_class::ingest_empty_modules(&source, &path_str) {
                            let empty: Vec<_> = empty.into_iter().filter(|m| !classes.iter().any(|c| c.name == m.name)).collect();
                            classes.extend(empty);
                        }
                        let carried: crate::det::HashSet<String> =
                            classes.iter().map(|c| c.name.0.as_str().to_string()).collect();
                        let list_dir = |dir: &Path| -> Vec<(String, bool)> {
                            vfs.read_dir(dir)
                                .map(|entries| {
                                    entries
                                        .into_iter()
                                        .filter_map(|e| {
                                            let name = e.file_name()?.to_str()?.to_string();
                                            Some((name, vfs.is_dir(&e)))
                                        })
                                        .collect()
                                })
                                .unwrap_or_default()
                        };
                        if let Some(program) = super::file_program::file_program(&source, &path_str, &carried, &list_dir) {
                            app.file_programs.push(program);
                        }
                    }
                    let conditional: crate::det::HashSet<&crate::ident::ClassId> =
                        classes.iter().flat_map(|c| c.conditional_definitions.iter().flat_map(|d| d.declared_names())).collect();
                    let unconditional =
                        classes.iter().map(|c| c.name.clone()).filter(|name| !conditional.contains(name)).collect();
                    app.library_class_files.push((path_str.clone(), unconditional));
                    let start = app.library_classes.len();
                    app.library_classes.extend(classes);
                    if !aliases.is_empty() {
                        held_aliases.push(super::inherited_alias::Held {
                            path: path_str.clone(),
                            classes: start..app.library_classes.len(),
                            spans: aliases,
                        });
                    }
                    // A Concern here runs its `included do` in a model that
                    // includes it, as one under app/models/concerns does.
                    let (concern_items, concern_enum_decls, concern_guarded) =
                        ingest_concern_model_items_with_constants(&source, &path_str, &enum_constants);
                    for (id, name) in ingest_concern_included_redefs(&source, &path_str) {
                        app.concern_included_redefs.entry(id).or_default().push(name);
                    }
                    append_concern_items(&mut app, concern_items, concern_guarded, |_| true);
                    concern_enums.extend(concern_enum_decls);
                }
                Err(err) => {
                    if survey::is_active() {
                        survey::record(&err);
                    }
                    // Left out of the IR, and named at what failed it.
                    if let IngestError::Unsupported { message, .. } = &err {
                        let (start, end) = super::error_site::of(&err).unwrap_or((0, 0));
                        let file = super::sources::file_id(&path_str);
                        app.ingest_drops.push(crate::app::IngestDrop {
                            span: crate::span::Span { file, start: start as u32, end: end as u32 },
                            message: message.clone(),
                        });
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

    super::inherited_alias::settle(&mut app, held_aliases);

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
                        if is_helper_module {
                            app.helper_modules.extend(classes.iter().map(|lc| lc.name.clone()));
                        }
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
    // Rails' ParamsWrapper default (see `App::wrap_parameters_by_default`).
    app.wrap_parameters_by_default = read_wrap_parameters_by_default(vfs, dir);
    // The ActiveJob queue adapter (see `App::queue_adapter`).
    app.queue_adapter = read_queue_adapter(vfs, dir);
    // Where Sidekiq pushes (see `App::sidekiq_redis`).
    app.sidekiq_redis = read_sidekiq_redis(vfs, dir);
    // The worker's defaults (see `App::sidekiq_worker`).
    let (worker_defaults, worker_warnings) = super::sidekiq_yml::read(vfs, dir);
    app.sidekiq_worker = worker_defaults;
    // Its warnings, and cable.yml's, wait for ApplicationJob's adapter
    // below: they are said only to a tree that ends up on :sidekiq.
    let mut sidekiq_warnings = worker_warnings;
    // Broadcasts between processes (see `App::cable_redis`). On
    // :sidekiq, a production adapter other than redis keeps a job's
    // broadcasts in the worker, as Rails does with `async`: say so.
    // The warning for that waits for ApplicationJob's adapter, below.
    let (cable_adapter, cable_redis, cable_warnings) = super::cable_yml::read(vfs, dir);
    app.cable_redis = cable_redis;
    sidekiq_warnings.extend(cable_warnings);
    let app_config_path = dir.join("config/application.rb");
    if let Ok(source) = vfs.read(&app_config_path) {
        app.loaded_frameworks =
            crate::lower::defined_const_fold::loaded_frameworks(&String::from_utf8_lossy(&source));
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
        // Application reopen; the boot makes it the runtime's default
        // zone (`ActiveSupport.set_default_zone`). Every other config.* line
        // remains railtie noise ingest deliberately does not model.
        if let Some(zone) = extract_config_time_zone(&source) {
            if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                "def config_time_zone\n  {zone:?}\nend\n"
            )) {
                methods.append(&mut synth);
            }
        }
        // `config.content_security_policy do |policy| … end`. The
        // request copies this per call; nil (the runtime default) when
        // the initializer is absent or DISABLE_CONTENT_SECURITY_POLICY
        // is set. Sources are quoted here (`:self` → "'self'") because
        // the runtime setter stores the list it is given.
        if let Ok(init) = vfs.read(&dir.join("config/initializers/content_security_policy.rb")) {
            if let Some(method) = content_security_policy_method(&String::from_utf8_lossy(&init)) {
                if let Ok(mut synth) = crate::runtime_src::parse_methods(&method) {
                    methods.append(&mut synth);
                }
            }
        }
        // The forgery check's verification strategy (Rails main's
        // Fetch Metadata rule; see `read_forgery_verification_strategy`).
        // Only a non-default answer is synthesized; the runtime default
        // in runtime/ruby/rails.rb is Rails' class default.
        if let Some(strategy) = read_forgery_verification_strategy(vfs, dir) {
            if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                "def forgery_protection_verification_strategy\n  {strategy:?}\nend\n"
            )) {
                methods.append(&mut synth);
            }
        }
        // Action Cable's handshake origin rule: `config.action_cable.
        // allowed_request_origins`, `allow_same_origin_as_host` and
        // `disable_request_forgery_protection`, per environment (see
        // `action_cable_config`). Only keys the app assigns are
        // synthesized; the runtime defaults in runtime/ruby/rails.rb
        // are Rails' own.
        // The app's own expressions are spliced in, so a parse failure
        // is the app's settings lost: refused rather than answered with
        // the permissive defaults.
        if let Some(cable) = super::action_cable_config::synthesize(vfs, dir)? {
            let mut synth = crate::runtime_src::parse_methods(&cable).map_err(|message| IngestError::Unsupported {
                file: "config/ (config.action_cable settings)".to_string(),
                message: format!("the app's Action Cable origin settings do not parse once synthesized: {message}"),
            })?;
            methods.append(&mut synth);
        }
        // The headers `ActionDispatch::Static` puts on every `public/`
        // file (`config.public_file_server.headers`; see
        // `public_file_server_config`). Spliced like the cable settings,
        // and refused the same way when the effective value is not a
        // Hash the runtime can type.
        if let Some(headers) = super::public_file_server_config::synthesize(vfs, dir)? {
            let mut synth = crate::runtime_src::parse_methods(&headers).map_err(|message| IngestError::Unsupported {
                file: "config/ (config.public_file_server.headers)".to_string(),
                message: format!("the app's public file server headers do not parse once synthesized: {message}"),
            })?;
            methods.append(&mut synth);
        }
        // Session cookie name: application-derived default plus any
        // `config.session_store` declarations (see `session_config`).
        // Spliced like the cable / public-file-server settings. The
        // synthesizer reads the VFS itself so an initializer-only tree
        // (no `config/application.rb` body methods) still gets a key.
        if let Some(cookie) = super::session_config::synthesize(vfs, dir)? {
            let mut synth = crate::runtime_src::parse_methods(&cookie).map_err(|message| IngestError::Unsupported {
                file: "config/ (config.session_store)".to_string(),
                message,
            })?;
            methods.append(&mut synth);
        }
        // A helper that overrides ActionView's `token_tag` to answer ""
        // (campfire's ApplicationHelper: "Header-only forgery protection
        // needs no secret in forms or cached HTML"). Rails' form helpers
        // build their token field through it, so every form then has
        // none; synthesized as the runtime's switch for that. Another
        // body is not read.
        if app_helpers_blank_token_tag(vfs, dir) {
            if let Ok(mut synth) =
                crate::runtime_src::parse_methods("def token_fields_omitted\n  true\nend\n")
            {
                methods.append(&mut synth);
            }
        }
        // `GlobalID.app` — the first segment of every `gid://<app>/
        // <Model>/<id>` this runtime mints, and half of every turbo
        // stream name that names a record. globalid's railtie takes it
        // from the application's railtie name with every `_application`
        // removed, dasherized (`Campfire` -> "campfire", `MyApp` ->
        // "my-app", `Outer::HTMLApp` -> "outer-html-app",
        // `MyApplication` -> "my"), so a gid a real Rails
        // process minted for a multi-word app locates here too.
        // Synthesized as an override on the Application reopen, the
        // same shape as `session_cookie_key` above; the namespace itself
        // is dropped at ingest, so without this the name is gone by
        // emit time.
        if let Some(app_name) = super::session_config::application_global_id_app(&source) {
            if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                "def global_id_app\n  {app_name:?}\nend\n"
            )) {
                methods.append(&mut synth);
            }
        }
        // Active Storage initializer lifts — one walk of
        // `config/initializers` for the cluster that shares that
        // directory: variable_content_types trim, video_preview_arguments,
        // and previewers VideoPreviewer → replacement Const map.
        {
            let init_dir = dir.join("config/initializers");
            let mut excluded: Vec<String> = Vec::new();
            let mut video_args: Option<String> = None;
            let mut previewer_replacement: Option<String> = None;
            if vfs.is_dir(&init_dir) {
                for entry in read_rb_files(vfs, &init_dir)? {
                    if let Ok(bytes) = vfs.read(&entry) {
                        excluded.extend(extract_variable_content_type_exclusions(&bytes));
                        match extract_video_preview_arguments(&bytes) {
                            VideoPreviewArgsExtract::Value(a) => video_args = Some(a),
                            VideoPreviewArgsExtract::Unsupported => {
                                // A later computed assignment must not leave an
                                // earlier literal override in effect.
                                video_args = None;
                                survey::record(&IngestError::Unsupported {
                                    file: entry.display().to_string(),
                                    message: "config.active_storage.video_preview_arguments is not a string-literal concatenation; the emitted app keeps the framework default".to_string(),
                                });
                            }
                            VideoPreviewArgsExtract::Absent => {}
                        }
                        if let Some(name) = extract_video_previewer_replacement(&bytes) {
                            previewer_replacement = Some(name);
                        }
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
            // One Rails knob → one Application override. The vf filter
            // is peeled at `ActiveStorage.video_preview_vf_filter`.
            if let Some(arguments) = video_args {
                if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                    "def active_storage_video_preview_arguments
  {arguments:?}
end
"
                )) {
                    methods.append(&mut synth);
                }
            }
            if let Some(replacement) = previewer_replacement {
                if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                    "def active_storage_previewers
  [{replacement}]
end
"
                )) {
                    methods.append(&mut synth);
                }
            }
        }
        // `config.filter_parameters += [:passw, …]` — the keys Rails
        // masks in `request.filtered_parameters`. Application body
        // first, then the initializers in load order, as Rails runs
        // them. (Rails also copies the list into
        // `ActiveSupport.filter_parameters` for the event reporter; the
        // runtime's stays the reporter's own, as before.) Synthesized as
        // `config_filter_parameters` on the reopen, over the framework
        // default (`[]`) in runtime/ruby/rails.rb. A Regexp literal is
        // kept as written (ParameterFilter matches it against the key);
        // an interpolated Regexp, a Proc or a computed operand is not
        // read, and the survey names it.
        {
            let mut names: Vec<String> = Vec::new();
            let mut seen = false;
            let mut fold = |bytes: &[u8], file: &Path| {
                for line in extract_filter_parameters(bytes) {
                    match line {
                        FilterParametersLine::Literal { replace, filters: more } => {
                            seen = true;
                            if replace {
                                names.clear();
                            }
                            names.extend(more);
                        }
                        FilterParametersLine::Computed { replace } => {
                            // A computed `=` still replaces what came before
                            // it; only its own operand goes unread.
                            if replace {
                                seen = true;
                                names.clear();
                            }
                            survey::record(&IngestError::Unsupported {
                                file: file.display().to_string(),
                                message: "config.filter_parameters is not a list of Symbol, String or Regexp literals; request.filtered_parameters does not apply it".to_string(),
                            })
                        }
                    }
                }
            };
            fold(&source, &app_config_path);
            let init_dir = dir.join("config/initializers");
            if vfs.is_dir(&init_dir) {
                for entry in read_rb_files(vfs, &init_dir)? {
                    if let Ok(bytes) = vfs.read(&entry) {
                        fold(&bytes, &entry);
                    }
                }
            }
            if seen {
                let literal = names.join(", ");
                if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                    "def config_filter_parameters\n  [{literal}]\nend\n"
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
        // Default page size for `Relation#page`. A `Kaminari.configure`
        // block's literal `default_per_page = N` is one input spelling;
        // synthesized as `default_per_page` on the reopen, over the
        // runtime default of 25 in runtime/ruby/rails.rb.
        {
            let init_dir = dir.join("config/initializers");
            let mut per_page: Option<u64> = None;
            if vfs.is_dir(&init_dir) {
                for entry in read_rb_files(vfs, &init_dir)? {
                    if let Ok(bytes) = vfs.read(&entry) {
                        let file = entry.display().to_string();
                        per_page = extract_default_per_page(&bytes, &file).or(per_page);
                    }
                }
            }
            if let Some(n) = per_page {
                if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
                    "def default_per_page\n  {n}\nend\n"
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
            for (path, bytes) in &sources {
                assignments.extend(extract_config_assignments(bytes, path));
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
                parent_span: Default::default(),
                includes: Vec::new(),
                include_spans: Vec::new(),
                methods,
                nullable_columns: Vec::new(),
                origin: None,
                constants,
                conditional_definitions: Vec::new(),
                unknown_calls: Vec::new(),
                class_ivar_initializers: Vec::new(),
            });
        }
    } else if let Some(cookie) = super::session_config::synthesize(vfs, dir)? {
        // No `config/application.rb`: still lift a key from initializers /
        // env files onto a reopen, same method body the branch above uses.
        let methods = crate::runtime_src::parse_methods(&cookie).map_err(|message| {
            IngestError::Unsupported {
                file: "config/ (config.session_store)".to_string(),
                message,
            }
        })?;
        app.rails_application = Some(crate::dialect::LibraryClass {
            name: crate::ident::ClassId(crate::ident::Symbol::from("Rails::Application")),
            is_module: false,
            parent: None,
            parent_span: Default::default(),
            includes: Vec::new(),
            include_spans: Vec::new(),
            methods,
            nullable_columns: Vec::new(),
            origin: None,
            constants: Vec::new(),
            conditional_definitions: Vec::new(),
            unknown_calls: Vec::new(),
            class_ivar_initializers: Vec::new(),
        });
    }

    // Top-level constants an initializer defines that are not mixins —
    // kept or dropped after every app file is read (see
    // `keep_initializer_defined`).
    let mut initializer_defined: Vec<LibraryClass> = Vec::new();
    // `ActionController::Base.prepend_view_path …`, applied with the class
    // bodies' own once every class is read (see `view_paths`).
    let mut view_path_ops = Vec::new();

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
                view_path_ops.extend(super::view_paths::extract_initializer_view_paths(&bytes, &path_str));
                app.initializer_filters.extend(extract_initializer_filters(&bytes, &path_str));
                app.initializer_defs.extend(extract_initializer_defs(&bytes, &path_str));
                app.gem_package_settings.extend(super::gem_package_settings::extract(&bytes, &path_str));
                app.initializer_response_headers
                    .extend(extract_initializer_response_headers(&bytes, &path_str));
                if let Some(store) = extract_collection_cache_store(&bytes, &path_str) {
                    app.collection_cache_store = Some(store);
                }
                app.sql_functions
                    .extend(super::sql_functions::extract_sql_functions(&bytes, &path_str));
            }
        }
    }
    // `config/database.yml`'s `pragmas:`, run on each connection after
    // the Db shim's own (see `database_yml`).
    app.database_pragmas = super::database_yml::database_pragmas(vfs, dir);
    // Its configurations, resolved at build time; registered as a source
    // so a refusal points into it. Before the source snapshot below.
    let database_yml = dir.join("config/database.yml");
    if let Ok(text) = vfs.read_to_string(&database_yml) {
        let file = super::sources::register(&database_yml.display().to_string(), &text);
        let configurations = super::database_yml::database_configurations(&text, file);
        app.databases = configurations.databases;
        app.database_refusals.extend(configurations.refusals);
    }

    for root in &roots {
        let controllers_dir = dir.join(root).join("controllers");
        if !vfs.is_dir(&controllers_dir) {
            continue;
        }
        for entry in read_rb_files(vfs, &controllers_dir)? {
            let Some(source) = read_or_ledger(vfs, &entry)? else { continue };
            let path_str = entry.display().to_string();
            if let Some(maybe_controller) =
                unwrap_or_record(ingest_controller(&source, &path_str))?
            {
                if let Some(controller) = maybe_controller {
                    // `helper_method :x` exposes controller methods to
                    // templates. The ARG-PURE ones (no instance state)
                    // register like app-helper functions — the bare
                    // view call rewrites to `<Controller>.x(args)`
                    // against a class-side clone the controller
                    // lowering synthesizes. Registered before the
                    // app/helpers pass below, so a same-named helper-
                    // module function wins (its insert overwrites).
                    // Judged on the controller as written: the lowering
                    // clones from it once its concerns are spliced in,
                    // and `reconcile_controller_helper_methods` drops
                    // what that controller no longer clones.
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
                    // A concern nested in the controller, here or in a
                    // `cache_controller/versioning.rb` opening, declares
                    // its `included do` filters as one in the concerns
                    // folder does (the branch below).
                    if let Some(concern_filters) =
                        unwrap_or_record(ingest_concern_filters(&source, &path_str))?
                    {
                        let prefix = format!("{}::", outer.0.as_str());
                        app.concern_filters.extend(
                            concern_filters.into_iter().filter(|(id, _)| id.0.as_str().starts_with(&prefix)),
                        );
                    }
                } else {
                    // Not a controller file: a module-only concern under
                    // app/controllers/concerns/, a mixin like
                    // `Authorization`, or a concern whose only classes
                    // are Sorbet value objects (nested `T::Struct`).
                    // Ingest as library classes so methods register and
                    // `include X` dispatch (ClassInfo.includes) can
                    // resolve into them, and capture `included do`
                    // filter declarations for every includer's chain.
                    if let Some(classes) =
                        unwrap_or_record(ingest_library_classes(&source, &path_str))?
                    {
                        app.library_classes.extend(classes);
                        if let Some(concern_filters) =
                            unwrap_or_record(ingest_concern_filters(&source, &path_str))?
                        {
                            app.concern_filters.extend(concern_filters);
                        }
                        let (concern_items, concern_enum_decls, concern_guarded) =
                            ingest_concern_model_items_with_constants(
                                &source, &path_str, &enum_constants,
                            );
                        for (id, name) in ingest_concern_included_redefs(&source, &path_str) {
                            app.concern_included_redefs.entry(id).or_default().push(name);
                        }
                        append_concern_items(&mut app, concern_items, concern_guarded, |_| true);
                        app.view_visible_controller_methods
                            .extend(ingest_helper_method_names(&source));
                        concern_enums.extend(concern_enum_decls);
                    }
                }
            }
        }
    }

    let mut model_files = Vec::new();
    for root in &roots {
        let models_dir = dir.join(root).join("models");
        if vfs.is_dir(&models_dir) {
            model_files.extend(read_rb_files(vfs, &models_dir)?);
        }
    }
    app.generated_helper_methods =
        super::generated_helpers::ingest_generated_helpers(&app, vfs, dir, &model_files);

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
            let mut draw_files: HashMap<String, (Vec<u8>, String)> = HashMap::default();
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
            let block_wrappers = mapper_extension_block_methods(&app, vfs, dir);
            // The constants the route files read, folded from their owners
            // as a model body's are, for `constraints:` values. A route
            // names a controller's constant as often as a model's, so its
            // table also takes the controllers, which the enum inputs above
            // leave out on purpose (a controller is not a model's input).
            let mut route_inputs = super::model::EnumConstants::default();
            let mut route_files: Vec<PathBuf> = enum_input_files.iter().cloned().collect();
            for root in &roots {
                let controllers_dir = dir.join(root).join("controllers");
                if vfs.is_dir(&controllers_dir) {
                    route_files.extend(read_rb_files(vfs, &controllers_dir)?);
                }
            }
            // A controller can also be an autoload input (an app that adds
            // `app/controllers/concerns` to its autoload paths); recorded
            // twice, its constants would count as written twice and fold
            // nowhere.
            route_files.sort();
            route_files.dedup();
            for file in &route_files {
                if let Ok(text) = vfs.read(file) {
                    route_inputs.record_again(&text, &file.display().to_string());
                }
            }
            for file in &boot_files {
                if let Some(text) = read_or_ledger(vfs, file)? {
                    route_inputs.record_boot(&text, &file.display().to_string());
                }
            }
            route_inputs.finish();
            let mut route_consts = super::model::ClassConsts::default();
            for text in std::iter::once(&source).chain(draw_files.values().map(|(s, _)| s)) {
                let parsed = ruby_prism::parse(text);
                route_consts.import(&[parsed.node()], &route_inputs, &[]);
                // Not only other files' constants: a route file may define the one its own `constraints` names (lobsters' DOMAINS_IDENTIFIER)
                let top: Vec<ruby_prism::Node<'_>> = parsed
                    .node()
                    .as_program_node()
                    .map(|program| program.statements().body().iter().collect())
                    .unwrap_or_default();
                route_consts.merge(super::model::ClassConsts::collect(&top, &routes_path.display().to_string()));
            }
            let routes = super::routes::with_route_constants(route_consts, || {
                ingest_routes_with_engines(
                    &source,
                    &routes_path.display().to_string(),
                    &draw_files,
                    &block_wrappers,
                    &engine_routes,
                    &helper_sources,
                    &load_helper_sources,
                )
            });
            if let Some(routes) = unwrap_or_record(routes)? {
                // `to: redirect("/x")` routes point at actions nobody
                // wrote, so write them: one controller, one action per
                // redirect, each a `redirect_to <literal>, status: …`. It
                // is the shape an app uses by hand for the same thing, and
                // it keeps the redirect out of every emitter's route kind.
                if !routes.redirects.is_empty() {
                    app.controllers.push(synthesize_redirect_controller(&routes.redirects));
                    // The inherited runtime calls this helper, outside app call-site inference.
                    let signatures = crate::rbs::parse_app_signatures(
                        "class RoundhouseRedirectsController\n  def same_host_location: (String location) -> String\n  def absolute_redirect_location: (String location) -> String\nend\n",
                    ).expect("routing redirect policy signature parses");
                    for (owner, methods) in signatures {
                        app.rbs_signatures.entry(owner).or_default().extend(methods);
                    }
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
    // Only across roots — within one, nothing changes. The roots are each
    // class's view paths, which start as these and which the app can
    // prepend to, append to or replace (`view_paths`).
    let view_lookup = super::view_paths::ViewLookup::new(
        &app,
        roots.iter().map(|root| root.join("views")).collect(),
        &view_path_ops,
    );
    let mut walked: Vec<super::view_paths::WalkedRoot> = Vec::new();
    let mut ingested: HashMap<PathBuf, crate::dialect::View> = HashMap::default();
    for root in view_lookup.roots() {
        let views_dir = dir.join(root);
        if !vfs.is_dir(&views_dir) {
            continue;
        }
        // A view path inside one already walked (`app/views/themes/next`
        // in `app/views`) holds files that walk read: the same files, not a
        // second read of them.
        if let Some(outer) = walked.iter().find(|w| views_dir.starts_with(&w.dir)) {
            let files = outer.files.iter().filter(|f| f.starts_with(&views_dir)).cloned().collect();
            walked.push(super::view_paths::WalkedRoot { root: root.clone(), dir: views_dir, files });
            continue;
        }
        let mut files = Vec::new();
        let erb_files = read_erb_files(vfs, &views_dir)?;
        for (erb_path, engine) in erb_files {
            files.push(erb_path.clone());
            if ingested.contains_key(&erb_path) {
                continue;
            }
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
                ingested.insert(erb_path, view);
            }
        }

        let jbuilder_files = read_jbuilder_files(vfs, &views_dir)?;
        for jb_path in jbuilder_files {
            files.push(jb_path.clone());
            if ingested.contains_key(&jb_path) {
                continue;
            }
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
                ingested.insert(jb_path, view);
            }
        }
        walked.push(super::view_paths::WalkedRoot { root: root.clone(), dir: views_dir, files });
    }
    app.views.extend(view_lookup.select(&app, dir, &walked, &ingested));

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
    // The helper, the shared helper modules and the fixtures are read on
    // their own terms, above and below; a `test_paths` entry of `test`
    // must not read them a second time as test files.
    let helper_rb = dir.join("test/test_helper.rb");
    let (helpers_dir, fixtures_dir) = (dir.join("test/test_helpers"), dir.join("test/fixtures"));
    test_files.retain(|f| f != &helper_rb && !f.starts_with(&helpers_dir) && !f.starts_with(&fixtures_dir));
    test_files.sort();
    test_files.dedup();

    // The rest of `test/test_helpers/`: modules one test class includes
    // itself (campfire's `include PushServiceTestHelper` in two web push
    // tests). Kept whole — nested classes and module methods too — and
    // carried into each including test's file as inner classes. A file
    // in a test folder that defines a module and no test class is one
    // too (`test_paths: [test/support]`, `include ArgsHelper`).
    let included_test_helpers = ingest_included_test_helper_files(vfs, dir, &shared_test_helpers, &test_files)?;
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

    // The test helper Concerns: an `include` of one runs its `included`
    // block's statements on the including test class, and the ones the
    // helper mixes into every test case run on each of them.
    let test_concerns = {
        let helpers_dir = dir.join("test/test_helpers");
        let mut files: Vec<PathBuf> =
            if vfs.is_dir(&helpers_dir) { read_rb_files(vfs, &helpers_dir)? } else { Vec::new() };
        files.extend(test_files.iter().cloned());
        let mut concerns = Vec::new();
        for f in files {
            let Some(source) = read_or_ledger(vfs, &f)? else { continue };
            let file = f.display().to_string();
            for name in super::test::concern_modules(&source, &file) {
                concerns.push(super::test::TestConcern { name, file: file.clone(), source: source.to_vec() });
            }
        }
        let case_wide = test_case_wide_includes(vfs, dir)?
            .iter()
            .map(|s| s.as_str().trim_start_matches("::").to_string())
            .collect();
        super::test::TestConcerns { concerns, case_wide }
    };

    // A test-case base the helper DEFINES (`class GemCompat::TestCase
    // < ActiveSupport::TestCase`), which the app's tests then subclass. A
    // reopen (`class ActiveSupport::TestCase` with no superclass) is the
    // helper configuring Rails' own base, not a class of the app's.
    if vfs.exists(&helper_rb) {
        if let Some(source) = read_or_ledger(vfs, &helper_rb)? {
            if let Some(tms) =
                unwrap_or_record(ingest_test_bases(&source, &helper_rb.display().to_string()))?
            {
                for mut tm in tms.into_iter().filter(|tm| tm.parent.is_some()) {
                    splice_test_helpers(&mut tm, &shared_test_helpers);
                    app.test_modules.push(tm);
                }
            }
        }
    }

    for entry in test_files {
        let Some(source) = read_or_ledger(vfs, &entry)? else { continue };
        // An RSpec file in a listed folder (`test_paths: [spec/models]`):
        // a `*_spec.rb` whose top level is an example group. RSpec loads
        // its own helper, never `test/test_helper.rb`, so nothing from
        // that file is spliced into its modules. A `*_spec.rb` holding a
        // test class is read as before.
        if entry.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with("_spec.rb"))
            && super::spec::is_rspec_source(&source)
        {
            if let Some(tms) =
                unwrap_or_record(super::spec::ingest_spec_files(&source, &entry.display().to_string()))?
            {
                for mut tm in tms {
                    // Names are unique within one spec file, but two files
                    // can give the same one (`article/comments_spec.rb` and
                    // `article_comments_spec.rb`). A taken name gets the next
                    // free number, as a repeated group does within a file.
                    let base = tm.name.0.as_str().to_string();
                    let mut n = 1;
                    while app.test_modules.iter().any(|t| t.name == tm.name) {
                        n += 1;
                        tm.name = crate::ident::ClassId(Symbol::from(format!("{base}{n}").as_str()));
                    }
                    app.test_modules.push(tm);
                }
            }
            continue;
        }
        let ingested = super::test::with_test_concerns(test_concerns.clone(), || {
            ingest_test_files(&source, &entry.display().to_string())
        });
        if let Some(tms) = unwrap_or_record(ingested)? {
            for mut tm in tms {
                // A module every test case already includes, included
                // again: a no-op in Ruby, and spliced rather than
                // carried here, so the `include` would name nothing.
                tm.includes.retain(|i| !shared_test_helpers.iter().any(|lc| &lc.name == i));
                splice_test_helpers(&mut tm, &shared_test_helpers);
                carry_included_test_helpers(&mut tm, &included_test_helpers);
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
                    if let Some(method) = importmap_json_method(&importmap.pins) {
                        if let Some(rails_app) = app.rails_application.as_mut() {
                            if let Ok(mut synth) = crate::runtime_src::parse_methods(&method) {
                                rails_app.methods.append(&mut synth);
                            }
                        }
                    }
                    app.importmap = Some(importmap);
                }
            }
        }
    }

    app.vite_manifest = super::vite::read_manifest(vfs, dir);
    let vite_digest = app.vite_manifest.as_ref().map(|m| m.digest.clone());
    app.inertia = super::inertia::load_inertia_config(vfs, dir, vite_digest.as_deref())?;

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

    // `config/locales/**/*.yml`, Rails' default I18n load path for the app.
    let locales_dir = dir.join("config/locales");
    let mut files = Vec::new();
    let mut stack = if vfs.is_dir(&locales_dir) { vec![locales_dir] } else { Vec::new() };
    while let Some(current) = stack.pop() {
        for entry in vfs.read_dir(&current)? {
            if vfs.is_dir(&entry) {
                stack.push(entry);
            } else if entry.extension().and_then(|s| s.to_str()) == Some("yml") {
                if let Some(source) = read_to_string_or_ledger(vfs, &entry)? {
                    let rel = entry.strip_prefix(dir).unwrap_or(&entry).display().to_string();
                    files.push((rel, source));
                }
            }
        }
    }
    let (mut gem_files, missing_gems) = match &app.gem_lock {
        Some(lock) => crate::i18n::gems::gem_locale_files(vfs, dir, lock, &path_gem_dirs(vfs, dir)),
        None => (Vec::new(), Vec::new()),
    };
    let application_rb = vfs.read_to_string(&dir.join("config/application.rb")).unwrap_or_default();
    let configured = application_rb.lines().map(str::trim).filter(|l| !l.starts_with('#')).find_map(available_locales_line).and_then(Result::ok);
    gem_files.retain(|(path, _)| crate::i18n::rails_i18n::loads(path, &application_rb, configured.as_deref()));
    for gem in missing_gems.iter().filter(|g| !g.starts_with("rails-i18n ") || crate::i18n::rails_i18n::enables(&application_rb, "locale")) {
        warn_i18n(format!("{gem} is in Gemfile.lock but installed in no gem root (BUNDLE_PATH, GEM_HOME, GEM_PATH): its locale files are not read, so a key only it defines answers as a missing translation"));
    }
    if let Some(version) = app.gem_lock.as_ref().and_then(|l| l.version_of("rails-i18n")) {
        if version != crate::i18n::rails_i18n::RULES_VERSION && crate::i18n::rails_i18n::enables(&application_rb, "pluralization") {
            warn_i18n(format!("rails-i18n {version}: plural rules are read as rails-i18n {}'s, which may differ from this version's", crate::i18n::rails_i18n::RULES_VERSION));
        }
    }
    let (catalog, problems) = crate::i18n::Catalog::load_with_gems(&default_locale(vfs, dir), &gem_files, &mut files);
    for (file, problem) in problems {
        survey::record(&IngestError::Unsupported { file, message: format!("locale file not loaded: {problem}") });
    }
    app.i18n = catalog;
    app.i18n.missing = missing_policy(vfs, dir);
    let (available, fallbacks, available_unknown) = locale_settings(vfs, dir, &app.i18n.locale, &files);
    app.i18n.load_locales(available, fallbacks, &gem_files, &mut files);
    app.i18n.available_unknown = available_unknown;
    app.i18n.field_error_proc = field_error_proc_file(vfs, dir);
    app.i18n.plurals = plural_rules(vfs, dir, app.gem_lock.as_ref(), &app.i18n);
    crate::i18n::stamp_models(&mut app);

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
            let (signatures, abstracts) = super::sorbet_sig::ingest_sorbet_declarations(&source);
            drop_abstract_stubs(&mut app, &abstracts);
            for (class_id, methods) in signatures {
                let declared = app.rbs_signatures.entry(class_id).or_default();
                for (name, ty) in methods {
                    declared.entry(name).or_insert(ty);
                }
            }
        }
    }

    // The boot sources were observed before the folds; observing them
    // again would count each of their writes twice.
    let observed: crate::det::HashSet<PathBuf> =
        enum_input_files.iter().chain(boot_files.iter()).cloned().collect();
    enum_constants.validate_consumed_sources(&app, &sources, &observed)?;
    keep_initializer_defined(&mut app, dir, &sources, initializer_defined);
    // `self.queue_adapter =` on ApplicationJob (see `App::queue_adapter`).
    apply_application_job_queue_adapter(&mut app);
    if app.queue_adapter.as_deref() == Some("sidekiq") {
        sidekiq_warnings.into_iter().for_each(warn_sidekiq);
    }
    if app.queue_adapter.as_deref() == Some("sidekiq") && app.cable_redis.is_none() {
        warn_sidekiq(format!(
            "broadcasts from jobs run by `sidekiq` workers stay in the worker process: config/cable.yml's production adapter is {}, not redis",
            cable_adapter.map(|a| format!("`{a}`")).unwrap_or_else(|| "not set".to_string())
        ));
    }
    super::propshaft::ingest(vfs, dir, &mut app)?;
    // The ledger of boot statements no recognizer above read. A warning
    // per statement, in the ingest diagnostic channel; no `App` field.
    let app_namespaces: HashSet<String> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str())
        .chain(app.library_classes.iter().map(|c| c.name.0.as_str()))
        .chain(app.controllers.iter().map(|c| c.name.0.as_str()))
        .map(|n| n.split("::").next().unwrap_or(n).to_string())
        .collect();
    super::initializer_ledger::record(
        vfs,
        dir,
        &path_gems,
        &engine_routes.keys().cloned().collect::<HashSet<String>>(),
        app.gem_lock.as_ref(),
        &app_namespaces,
    );
    // Carrier provenance must not depend on where a module lives:
    // models, services, helpers and lib all use the same splice.
    let mut concern_class_method_spans = Vec::new();
    let mut framework_shadow_scopes = crate::det::HashSet::default();
    for source in sources.iter().filter(|source| source.path.ends_with(".rb")) {
        let (carriers, shadows) = ingest_concern_class_method_spans(source.text.as_bytes(), &source.path);
        concern_class_method_spans.extend(carriers);
        framework_shadow_scopes.extend(shadows);
    }
    // Registered source paths are prefixed with this (the fs walk
    // joins `dir`); map-VFS trees pass `""` and register app-relative.
    app.root = dir.display().to_string().trim_end_matches('/').to_string();

    // `app/models/post/summary.rb` often reopens `class Post` only to
    // hold `Post::Summary`. That reopen is a namespace, not a class of
    // its own: kept as a library class, it owns the file
    // `app/models/post.rb` and the emit writes it over the model. With
    // the reopen dropped, the nested class keeps its own file, as a
    // class nested in the model's own file does. This runs after every
    // walk, because `app/services` and `lib` can hold the same reopen,
    // and `lib` can hold the model.
    let model_names: crate::det::HashSet<&str> =
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
    // The same reopen for a controller: `cache_controller/fetching.rb`
    // opens `class CacheController` only to hold `CacheController::
    // Fetching`. Nested classes are not body items, so that opening has no
    // superclass and no body. Kept, it also writes the controller's file,
    // and emit reopens the controller without its superclass around each
    // nested class. One opening stays when nothing else declares the class.
    // An opening that carries anything stays, and `diagnose` refuses it.
    // That includes a computed superclass (`< choose_base()`): it has a
    // span but no name.
    let is_opening = |c: &crate::dialect::Controller| {
        c.parent.is_none()
            && c.parent_span.is_synthetic()
            && c.body.is_empty()
            && c.layout.is_inherit()
            && c.sibling_classes.is_empty()
    };
    let declared: HashSet<crate::ident::ClassId> =
        app.controllers.iter().filter(|c| !is_opening(c)).map(|c| c.name.clone()).collect();
    let mut kept = HashSet::default();
    app.controllers.retain(|c| {
        !is_opening(c) || (!declared.contains(&c.name) && kept.insert(c.name.clone()))
    });

    // A model's `begin include(X) rescue NameError end` becomes its
    // `include`s (or nothing), now that every module of the app is
    // known; before the qualification below, which reads them.
    super::guarded_include::splice_guarded_includes(&mut app);
    // Every superclass and mixin, as Rubydex resolved it, before the
    // concern splices and every later consumer look them up by ClassId.
    // A module-nested `class XController < BaseController` names
    // `Ns::BaseController`; `include Avatar` inside `class User` names
    // `User::Avatar`. The passes above leave both as written.
    let const_resolver = crate::timings::phase("rubydex: wait", || const_resolver.finish());
    super::written_names::resolve(&mut app, &sources, &const_resolver.for_sources(&sources));
    // Before the concern splices: they read `library_classes`, and this
    // turns `Current`'s metaprogrammed surface into real methods first.
    super::current_attributes::lower_current_attributes(&mut app);
    // `attribute :x, :type` under `include ActiveModel::Attributes` -> real
    // casting accessors, before anything reads the class's methods.
    super::active_model_attributes::lower_active_model_attributes(&mut app);
    super::thread_mattr::lower_thread_mattr(&mut app);
    // Alba declarations become ordinary property-reading methods before
    // inference; validate complete original resource bodies, not just IR.
    // A recorded refusal is not support. Strict mode still fails here.
    // Survey mode keeps the ledger entry and continues analysis.
    if let Err(err) = super::alba::lower_alba_resources(&mut app, &sources) {
        survey::continue_or_fail(err)?;
    }
    // `Dry::Struct` classes become plain readers and a constructor,
    // before anything reads their methods.
    super::dry_struct::lower_dry_structs(&mut app, &sources);
    // After the structs: an option typed with a struct builds the lowered one.
    super::dry_initializer::lower_dry_initializers(&mut app, &sources);
    // Contracts and schemas, whose messages are the app's own files too.
    // A load path is read only inside the app, as the other app-relative
    // paths are; `read_configs` refuses one that reaches out.
    super::dry_validation::lower_dry_validation(&mut app, &sources, &|path: &str| {
        let mut relative = PathBuf::new();
        for component in Path::new(path).components() {
            match component {
                Component::CurDir => {}
                Component::Normal(segment) => relative.push(segment),
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
            }
        }
        let full = dir.join(relative);
        if path_has_symlink_component(vfs, dir, &full) {
            return None;
        }
        vfs.read_to_string(&full).ok()
    })?;
    // After it: a contract result's `to_monad` is a Result.
    super::dry_monads::lower_dry_monads(&mut app, &sources)?;
    // Oj reads its mode from the initializers (`Oj.mimic_JSON`).
    {
        let init_dir = dir.join("config/initializers");
        let mut initializers = Vec::new();
        if vfs.is_dir(&init_dir) {
            for entry in read_rb_files(vfs, &init_dir)? {
                if let Ok(text) = vfs.read_to_string(&entry) {
                    initializers.push(text);
                }
            }
        }
        super::oj::lower_oj(&mut app, &sources, &initializers)?;
    }
    super::dry_initializer::drop_type_constants(&mut app, &sources);
    // graphql-ruby object types: analyzer-only field methods, so
    // inference carries each type's record class down the schema.
    super::graphql_ruby::lower_graphql_types(&mut app);
    // Representable decorators: expanded into plain methods the same way;
    // survey mode ledgers an unsupported one and keeps going.
    super::representable::lower_representable_decorators(&mut app, &sources)?;
    // ActiveModel::Serializer classes, the same way; their controllers'
    // `render json: x, serializer: S` sites call the expansion.
    super::active_model_serializer::lower_active_model_serializers(&mut app, &sources)?;
    // After it, not before: `Current`'s own `delegate` reads an
    // ATTRIBUTE's ivar, which that pass has the declarations for. What
    // reaches here is the general shape, whose target is a method.
    super::delegate::lower_delegates(&mut app);
    // `Other.include(M)` written in another body: `Other`'s include,
    // before the concern splices read the includes.
    super::external_include::carry(&mut app);
    // After the passes that make methods an alias may name, and before
    // the concern splices copy a class's methods into its includers.
    super::inherited_alias::resolve(&mut app);
    // Channels, before the concern splices for the same reason `Current`
    // is: this turns a class-body macro into real methods, and every
    // later pass reads methods.
    super::channel_callbacks::lower_channel_callbacks(&mut app);
    super::channel_callbacks::lower_channel_names(&mut app);
    super::on_load_reopen::apply_pending(&mut app);
    splice_concerns_into_models(&mut app, &concern_class_method_spans);
    splice_concern_class_methods_into_includers(&mut app, &concern_class_method_spans);
    // After the splice, so a concern's DSL on a tableless parent reaches
    // its subclasses too; before the macros, which read the result.
    super::tableless_inheritance::inherit_tableless_parents(&mut app);
    super::tableless_inheritance::inherit_abstract_parent_validations(&mut app);
    super::sti_validations::apply(&mut app, sti_validations);
    super::acts_as_votable::expand_acts_as_votable(&mut app)?;
    // After the splice: `include Discard::Model` can arrive through a concern.
    super::discard::expand_discard_models(&mut app)?;
    // After the splice for the same reason; reads the initializers for an
    // `ActsAsTenant.configure` it does not model.
    {
        let init_dir = dir.join("config/initializers");
        let mut initializers = Vec::new();
        if app.gem_lock.as_ref().is_some_and(|l| l.has("acts_as_tenant")) && vfs.is_dir(&init_dir) {
            for entry in read_rb_files(vfs, &init_dir)? {
                if let Ok(text) = vfs.read_to_string(&entry) {
                    initializers.push((entry.display().to_string(), text));
                }
            }
        }
        super::acts_as_tenant::expand_acts_as_tenant(&mut app, &initializers)?;
    }
    // Before the macros: a macro's `extend Owner::ClassMethods` names the module this surfaces.
    super::referenced_carriers::surface_referenced_carriers(&mut app, &sources);
    super::conditional_permit::apply(&mut app);
    let hook_macros_called = super::class_macros::hook_macro_calls(&app);
    super::model_macros::expand_model_macros(&mut app, &sources)?;
    // After the concern-macro expansion: a concern's macros are that one's to expand or refuse.
    // The sources do not change between the two: one word index serves both.
    let mentions = super::class_macros::Mentions::new(&sources);
    super::class_macros::expand_class_macros(&mut app, &sources, &mentions);
    super::class_macros::drop_expanded_hook_macros(&mut app, &hook_macros_called, &mentions);
    super::model_callbacks::expand_model_callbacks(&mut app);
    // After the macros: an expanded `has_ancestry` is what writes the `extend`.
    super::extend_splice::splice_sole_extended_modules(&mut app);
    super::column_accessors::rewrite_super(&mut app);
    // After the splice, so a class method a concern contributed gets
    // the same treatment as one written in the model.
    qualify_model_class_method_ar_calls(&mut app);
    // After the splice too: the inverse `has_many …, as: :owner` may be
    // declared in a concern's `included do`.
    resolve_polymorphic_targets(&mut app);
    // After the splice too: the source association may come from a
    // concern on the through model.
    resolve_through_targets(&mut app);
    // `allow_browser` becomes a filter plus the method it runs, on the
    // concern (before the splice carries both to the includer) or on
    // the controller that called it directly.
    super::allow_browser::lower_allow_browser(&mut app);
    super::rate_limit::lower_rate_limit(&mut app);
    // The concern that wraps the pagy gem's `pagy` in a bounds check.
    super::pagy::lower_pagy(&mut app)?;
    super::invisible_captcha::lower_invisible_captcha(&mut app);
    super::impersonates::lower_impersonates(&mut app);
    super::inertia::lower_inertia(&mut app);
    super::vite::lower_vite_tags(&mut app);
    // The real drain, now that every pass re-ingesting synthesized
    // Ruby has run. A synthesized `"<label>"` re-ingest never takes a
    // slot (`sources::register` refuses a label starting with `<`), so
    // this is the same real-file list that Rubydex indexes, and its
    // answers use these `FileId`s.
    app.sources = super::sources::drain();
    super::sources::assert_snapshot_matches(&sources, &app.sources);
    app.source_index_required = !app.sources.is_empty();
    drop(sources);
    carry_included_block_includes_into_controllers(&mut app);
    splice_concerns_into_controllers(&mut app);
    // After the splice: an action a concern provides is not implicit.
    synthesize_template_only_actions(&mut app);
    // After the splice: a macro has to resolve against the concern's
    // class-side methods, and its expansion joins the same filter chain.
    super::class_configuration::expand(&mut app, &concern_class_method_spans, &framework_shadow_scopes)?;
    super::class_attribute::expand(&mut app, &concern_class_method_spans, &framework_shadow_scopes);
    expand_class_body_macros(&mut app)?;
    // After the splice and the macros: a concern's action, or one a macro synthesizes, gates its permit too.
    super::conditional_permit::apply(&mut app);
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
    app.content_helper_allowed_tags = content_helper_tag_additions(vfs, dir, &app);
    fold_concern_enums_into_models(&mut app, &concern_enums);
    // Include each base's Concern maps, and retain a child's own maps
    // (direct or Concern-declared) before filling inherited columns.
    inherit_enums(&mut app.models);
    // Last: needs every model's complete `enums` table, including the
    // columns an included concern declared.
    map_enum_labels(&mut app);
    // Last of all: `has_rich_text` / `has_markdown` can arrive through a
    // concern's `included do`, so the declaration scan has to run after
    // the splices — and the ActionText record models have to be in
    // `app.models` before anything downstream enumerates models.
    crate::lower::rich_text::synthesize_record_model(&mut app);
    crate::lower::plain_text_attr::synthesize_record_model(&mut app);
    crate::lower::attachment_model::synthesize_attachment_model(&mut app);
    // Second chance for `on_load(:active_storage_attachment)` includes
    // whose target was only synthesized above; then splice ONLY those
    // models so `has_many_attached` from the concern lands — a full
    // re-splice would duplicate every concern already expanded at the
    // first pass (doubled scopes, unique-constraint failures at run).
    let late_on_load = super::on_load_reopen::apply_pending(&mut app);
    if !late_on_load.is_empty() {
        splice_concerns_into_models_named(&mut app, &late_on_load, &concern_class_method_spans);
    }
    super::model_delegate::lower_model_delegates(&mut app);
    super::on_load_reopen::drain_pending(&mut app);
    // `numericality:` on an untyped attribute is decided per target
    // (`refuse_numericality_allow_on_untyped`, `session::analyze_and_lower_for`).
    gap_acceptance_on_undeclared(&mut app);
    // Not at model ingest: the `select` naming a model's aliases can sit in another model, class, controller or view.
    super::select_aliases::stamp(&mut app);
    gap_uniqueness_on_polymorphic_attribute(&mut app);
    // After every model and library class: a custom `validates` key is
    // kept only when it names an app `EachValidator`.
    super::each_validator::resolve(&mut app);
    app.const_resolver = const_resolver;
    // Admission needs complete controller permit demand and model DSL,
    // including declarations contributed by either kind of Concern,
    // and reuses the prepared resolver rather than rebuilding it.
    super::concern_accessors::validate(&mut app, &concern_class_method_spans, &framework_shadow_scopes)?;
    // After every model and reopen: a model may declare its key anywhere.
    key_composite_tables(&mut app)?;
    // Model-to-database ownership reads every model, concern splice and
    // library module, so it runs once they all exist.
    super::database_ownership::resolve(&mut app);

    // After every library class and include is known: a `...` that only
    // reaches a stdlib method of known signature through `super` takes
    // that signature (campfire's `WebPush::Connections::Stages`).
    crate::lower::known_super_forwarding::restate(&mut app);
    // After the controllers and views are in: their bodies read framework settings too.
    lift_read_framework_settings(vfs, dir, &mut app);
    collect_binary_assets(vfs, dir, &mut app);
    let mut require_roots: Vec<PathBuf> = roots.iter().map(|root| dir.join(root)).collect();
    require_roots.extend(
        support_roots(vfs, dir, &roots, &path_gems, &lib_ignores)
            .iter()
            .map(|root| dir.join(root)),
    );
    require_roots.extend(boot_required);
    app.app_requires = collect_app_requires(&app.sources, &require_roots);
    // Generated re-ingest labels never register real sources. No later
    // pass may append source-backed FileIds beyond the indexed snapshot.
    let late_sources = super::sources::drain();
    super::sources::assert_snapshot_matches(&[], &late_sources);
    // After the concern splices, which copy items in unfolded.
    super::lock_versions::fold_lock_versions(&mut app);
    // Before analysis too, so `check` types the recount the emit writes (`Origin.reset_counters(id, :stories)`), rather than reporting a method the lowering answers.
    crate::lower::reset_counters::apply_reset_counters_lowering(&mut app);
    // Before analysis, so the controller a functional test builds and the
    // controllers' clearing methods are typed like source.
    crate::lower::controller_test_case::prepare(&mut app);
    // Before the body statements, which ask whether a class-side writer
    // the class body writes through is defined.
    super::hook_class_attribute::expand(&mut app);
    super::app_extension_calls::mark(&mut app);
    super::body_statements::mark(&mut app);
    report_unmodelled_records(&app);
    super::unreachable_views::mark(&mut app);
    // After every pass that changes a controller's methods, the concern
    // splice and the class-body macros above all: the lowering clones
    // from the controller they leave.
    reconcile_controller_helper_methods(&mut app);
    super::uncalled_helper_copies::mark(&mut app);
    let (rake_tasks_source, rake_task_gaps) = super::rake_tasks::ingest(vfs, dir);
    app.rake_tasks_source = rake_tasks_source;
    app.rake_task_gaps = rake_task_gaps;
    Ok(app)
}

/// The `helper_method_index` entries a controller owns, judged again on
/// the controller ingest is done with. Each was registered from the
/// controller as written, right after `ingest_controller`; the
/// class-side clone the view call relies on is decided in the lowering,
/// on the controller its concerns were spliced into. A concern can make
/// a pure helper impure there: its `class_attribute :salutation` gives
/// the controller a class-side `salutation`, a clone of a `greeting`
/// that calls the private instance `salutation` would reach the
/// attribute instead, and the lowering keeps `greeting` on the
/// instance. Left registered, the view called a
/// `GreetingsController.greeting` nobody defined. Dropped, it reaches
/// the request's controller through `ActionController::Current
/// .controller`, as Rails runs a helper method.
///
/// Only dropped, never added: a helper the splice made pure (its callee
/// is a concern's method) keeps the dynamic path, which is still what
/// Rails runs, and the clone the lowering gives it goes unused. An entry
/// a helper module owns is left as it is.
/// A table whose primary key spans several columns
/// (`Table.primary_key_columns`) keys its records on the column its
/// models declare (`self.primary_key = "cityhash"`), which is then the
/// table's key column. A model that declares none — itself or through a
/// model ancestor — is refused, as the composite key was before: Rails
/// keys such a model on every column of it, and this compiler does not
/// yet.
fn key_composite_tables(app: &mut App) -> IngestResult<()> {
    let declared = |model: &crate::dialect::Model| -> Option<crate::ident::Symbol> {
        let mut current = Some(model);
        let mut seen = 0;
        while let Some(m) = current {
            // A reopen (`app/models/event/mentioned.rb`) is the same class:
            // any of its definitions may carry the declaration.
            if let Some(key) = app.models.iter().filter(|o| o.name == m.name).find_map(|o| o.primary_key.clone()) {
                return Some(key);
            }
            seen += 1;
            if seen > app.models.len() {
                return None;
            }
            current = m.parent.as_ref().and_then(|p| app.models.iter().find(|o| &o.name == p));
        }
        None
    };
    let mut keyed: Vec<(crate::ident::Symbol, crate::ident::Symbol)> = Vec::new();
    for model in &app.models {
        let Some(table) = app.schema.tables.get(&model.table.0) else { continue };
        if table.primary_key_columns.is_empty() {
            continue;
        }
        match declared(model) {
            Some(key) if table.columns.iter().any(|c| c.name == key) => keyed.push((table.name.clone(), key)),
            _ => {
                let columns = table.primary_key_columns.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", ");
                let refusal = super::IngestError::Unsupported {
                    file: "db/structure.sql".into(),
                    message: format!(
                        "primary key dropped: composite key ({}({columns})): `{}` declares no `self.primary_key` naming one of its columns",
                        table.name.as_str(),
                        model.name.0.as_str()
                    ),
                };
                super::survey::unwrap_or_record::<()>(Err(refusal))?;
            }
        }
    }
    for (table, key) in keyed {
        if let Some(table) = app.schema.tables.get_mut(&table) {
            for column in &mut table.columns {
                column.primary_key = column.name == key;
            }
        }
    }
    Ok(())
}

fn reconcile_controller_helper_methods(app: &mut App) {
    let owners: HashSet<&crate::ident::ClassId> = app.helper_method_index.values().collect();
    let mut still_pure: HashMap<crate::ident::ClassId, Vec<Symbol>> = HashMap::default();
    for controller in app.controllers.iter().filter(|c| owners.contains(&c.name)) {
        still_pure
            .entry(controller.name.clone())
            .or_default()
            .extend(crate::lower::controller_to_library::controller_helper_method_names(controller));
    }
    app.helper_method_index
        .retain(|name, owner| still_pure.get(owner).is_none_or(|names| names.contains(name)));
}

/// Standard libraries a Ruby program loads by name, the ones an app's
/// own `require` can mean. A require of anything else is the app's own
/// file (Zeitwerk or `require_relative` territory) or a gem, which the
/// Gemfile and the runtime wiring answer for.
// Not a warning per dropped declaration: the class is not a model here at all, so none of its persistence exists.
fn report_unmodelled_records(app: &App) {
    for lc in app.library_classes.iter().filter(|lc| !lc.is_module) {
        let Some(parent) = &lc.parent else { continue };
        if parent.0.as_str().trim_start_matches("::") != "ActiveRecord::Base" || app.models.iter().any(|m| m.name == lc.name) {
            continue;
        }
        super::prism::push(crate::diagnostic::Diagnostic::unsupported(
            lc.parent_span,
            None,
            super::UNMODELLED_RECORD,
            format!("`{}` extends ActiveRecord::Base from a file the app does not load as a model (a gem's, like audited's `Audited::Audit`), so its table, associations, scopes, validations and callbacks are missing", lc.name.0.as_str()),
        ));
    }
}

const APP_REQUIRABLE_STDLIB: &[&str] = &[
    "base64", "benchmark", "bigdecimal", "cgi", "csv", "date", "delegate", "digest",
    "digest/md5", "digest/sha1", "digest/sha2", "English", "erb", "etc", "fileutils", "find",
    "forwardable", "io/console", "io/nonblock", "io/wait", "ipaddr", "json", "logger",
    "monitor", "net/http", "observer", "open3", "openssl", "optparse", "ostruct", "pathname",
    "pp", "pty", "securerandom", "set", "shellwords", "singleton", "socket", "stringio",
    "strscan", "tempfile", "time", "timeout", "tmpdir", "uri", "weakref", "yaml", "zlib",
];

/// [`App::app_requires`]: every literal top-level `require "lib"` of a
/// standard library in the ingested Ruby sources under the app and
/// support roots, including files loaded by the boot chain. A terminal
/// channel that requires `pty` and `io/console` needs the second for `IO#winsize=`,
/// which io/console adds and no constant names, so the bundled requires
/// (driven by the constants a file names) cannot bring it back.
fn collect_app_requires(sources: &[crate::span::SourceFile], roots: &[PathBuf]) -> Vec<String> {
    let mut out = std::collections::BTreeSet::new();
    for source in sources {
        let path = Path::new(&source.path);
        if path.extension().and_then(|e| e.to_str()) != Some("rb")
            || !roots.iter().any(|root| path.starts_with(root))
        {
            continue;
        }
        for line in source.text.lines() {
            let Some(rest) = line.strip_prefix("require ") else { continue };
            // `require "io/console" # why`: the literal, then nothing but
            // a comment
            let rest = rest.trim_start();
            let Some(q) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else { continue };
            let Some(end) = rest[1..].find(q) else { continue };
            let after = rest[end + 2..].trim_start();
            let quoted = (after.is_empty() || after.starts_with('#')).then(|| &rest[1..end + 1]);
            if let Some(lib) = quoted {
                if APP_REQUIRABLE_STDLIB.contains(&lib) {
                    out.insert(lib.to_string());
                }
            }
        }
    }
    out.into_iter().collect()
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

/// `numericality:` (with or without `allow_nil:` / `allow_blank:`) on an
/// attribute the model does not type as a number (no Integer/Float column: an
/// `attr_accessor` of an `ActiveModel::Model` form, fed request Strings)
/// is the reported gap on a target whose check does not parse a String
/// number (#1158): there it would reject `"30"` where Rails accepts it. The
/// whole `validates` call is refused, and the warnings are returned. The
/// ruby family parses it (`emit::ruby::library::
/// parse_untyped_numericality_input`) and does not run this.
pub(crate) fn refuse_numericality_allow_on_untyped(app: &mut App, target: &str) -> Vec<crate::diagnostic::Diagnostic> {
    use crate::dialect::{ModelBodyItem, ValidationRule};
    let mut gaps: Vec<(crate::span::Span, String)> = Vec::new();
    for model in &mut app.models {
        let numeric = |attr: &crate::ident::Symbol| {
            let ty = model.attributes.fields.get(attr);
            let inner = match ty {
                Some(crate::ty::Ty::Union { variants }) => {
                    variants.iter().find(|v| !matches!(v, crate::ty::Ty::Nil)).cloned()
                }
                other => other.cloned(),
            };
            matches!(inner, Some(crate::ty::Ty::Int | crate::ty::Ty::Float))
        };
        let mut refused: Vec<(usize, crate::span::Span, String)> = Vec::new();
        for (i, item) in model.body.iter().enumerate() {
            let ModelBodyItem::Validation { validation, .. } = item else { continue };
            if numeric(&validation.attribute) {
                continue;
            }
            for rule in &validation.rules {
                if let ValidationRule::Numericality { allow_nil, allow_blank, .. } = rule {
                    // Without `allow_*` the check is `is_a?(Numeric)`, which
                    // rejects `"12"` as surely: refused too.
                    let construct = if *allow_blank == Some(true) {
                        format!("validates :{}, numericality: allow_blank:", validation.attribute.as_str())
                    } else if *allow_nil == Some(true) {
                        format!("validates :{}, numericality: allow_nil:", validation.attribute.as_str())
                    } else {
                        format!("validates :{}, numericality:", validation.attribute.as_str())
                    };
                    refused.push((i, item.span(), construct));
                }
            }
        }
        refuse_validates_calls(model, refused, &mut gaps);
    }
    validates_gap_diagnostics(
        app,
        target,
        gaps,
        "an untyped attribute's String input is not parsed as a number (#1158); emitting the check would reject input Rails accepts",
    )
}

/// `acceptance:` on an attribute the model does not declare as a column,
/// an `attr_*` or a method: Rails' `AcceptanceValidator` defines a plain,
/// uncast reader and writer for it on first use, so the model gains the
/// same `attr_accessor` here and the check reads what it writes. A name
/// declared only by `attribute :name, :type` of another type stays the
/// reported gap: its synthesized writer stores the input verbatim, where
/// Rails casts it (`true` is `"t"` on a String attribute). Refused as a
/// whole call, as the numericality gap is.
fn gap_acceptance_on_undeclared(app: &mut App) {
    use crate::dialect::{MethodReceiver, ModelBodyItem, ValidationRule};
    use crate::expr::{ExprNode, Literal};
    let mut attribute_api: Vec<(crate::span::Span, String)> = Vec::new();
    for model in &mut app.models {
        let attribute_call = |attr: &crate::ident::Symbol| {
            model.body.iter().any(|item| match item {
                ModelBodyItem::Unknown { expr, .. } => matches!(&*expr.node,
                    ExprNode::Send { recv: None, method, args, .. }
                        if method.as_str() == "attribute"
                            && args.first().is_some_and(|arg| matches!(&*arg.node, ExprNode::Lit { value: Literal::Sym { value } } if value == attr))),
                _ => false,
            })
        };
        let declared = |attr: &crate::ident::Symbol| {
            let setter = crate::ident::Symbol::from(format!("{}=", attr.as_str()));
            model.attributes.fields.contains_key(attr)
                || model.body.iter().any(|item| match item {
                    ModelBodyItem::Method { method, .. } => {
                        method.receiver == MethodReceiver::Instance && (method.name == *attr || method.name == setter)
                    }
                    ModelBodyItem::Unknown { expr, .. } => match &*expr.node {
                        ExprNode::Send { recv: None, method, args, .. }
                            if matches!(method.as_str(), "attr_accessor" | "attr_reader" | "attr_writer") =>
                        {
                            args.iter().any(|arg| {
                                matches!(&*arg.node, ExprNode::Lit { value: Literal::Sym { value } } if value == attr)
                            })
                        }
                        _ => false,
                    },
                    _ => false,
                })
        };
        let mut refused_attribute_api: Vec<(usize, crate::span::Span, String)> = Vec::new();
        let mut virtual_attrs: Vec<(usize, crate::ident::Symbol, crate::span::Span)> = Vec::new();
        for (i, item) in model.body.iter().enumerate() {
            let ModelBodyItem::Validation { validation, .. } = item else { continue };
            if !validation.rules.iter().any(|rule| matches!(rule, ValidationRule::Acceptance { .. })) {
                continue;
            }
            let attr = &validation.attribute;
            let construct = format!("validates :{}, acceptance:", attr.as_str());
            // An `attribute :x, :boolean` with the synthesized writer is
            // lowered on the ruby family and reported per strict target
            // (`lower::validations::acceptance_gaps_for_target`).
            if declared(attr) || crate::lower::model_to_library::markers::synthesized_boolean_attribute(model, attr) {
                continue;
            } else if attribute_call(attr) {
                refused_attribute_api.push((i, item.span(), construct));
            } else if !virtual_attrs.iter().any(|(_, a, _)| a == attr) {
                virtual_attrs.push((i, attr.clone(), item.span()));
            }
        }
        // Rails' `AcceptanceValidator` defines a plain `attr_reader` and
        // `attr_writer` for an attribute the model does not declare
        // (`LazilyDefineAttributes`), uncast: the same accessor written
        // as `attr_accessor :agreement`, placed ahead of the validation.
        for (i, attr, span) in virtual_attrs.into_iter().rev() {
            let accessor = crate::expr::Expr::new(
                span,
                ExprNode::Send {
                    recv: None,
                    method: crate::ident::Symbol::from("attr_accessor"),
                    args: vec![crate::expr::Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: attr } })],
                    block: None,
                    parenthesized: false,
                },
            );
            model.body.insert(i, ModelBodyItem::Unknown { expr: accessor, leading_comments: Vec::new(), leading_blank_line: false });
        }
        // After the inserts: their indices are the body's as scanned, and
        // the refusal removes by span, not by index.
        refuse_validates_calls(model, refused_attribute_api, &mut attribute_api);
    }
    report_validates_gaps(
        app,
        attribute_api,
        "acceptance: on an `attribute` the model declares, whose writer stores the input uncast where Rails casts it",
    );
}

/// Drop every Validation item of each refused `validates` call, as an
/// ingest refusal (`ParsedValidates::Gap`) does: every item a call
/// produced shares its span (one per attribute, and per-rule `if:` /
/// `unless:` splits), so a `presence:` beside the refused rule is not kept.
fn refuse_validates_calls(
    model: &mut crate::dialect::Model,
    refused: Vec<(usize, crate::span::Span, String)>,
    gaps: &mut Vec<(crate::span::Span, String)>,
) {
    use crate::dialect::ModelBodyItem;
    let calls: crate::det::HashSet<(crate::span::FileId, u32, u32)> =
        refused.iter().map(|(_, span, _)| (span.file, span.start, span.end)).collect();
    model.body.retain(|item| {
        !matches!(item, ModelBodyItem::Validation { .. }) || {
            let span = item.span();
            !calls.contains(&(span.file, span.start, span.end))
        }
    });
    for (_, span, construct) in refused {
        gaps.push((span, construct));
    }
}

/// `report_validates_gaps` after ingest: the warnings go back to the
/// caller, which reports them with its own diagnostics.
pub(super) fn validates_gap_diagnostics(
    app: &App,
    target: &str,
    gaps: Vec<(crate::span::Span, String)>,
    detail: &str,
) -> Vec<crate::diagnostic::Diagnostic> {
    gaps.into_iter()
        .map(|(span, construct)| {
            let file = crate::ide::source(app, span.file).map(|s| s.path.clone()).unwrap_or_default();
            let mut diagnostic = crate::diagnostic::Diagnostic::unsupported(span, Some(crate::ident::Symbol::from(target)), construct, detail);
            diagnostic.severity = crate::diagnostic::Severity::Warning;
            survey::record(&IngestError::Unsupported { file, message: diagnostic.message.clone() });
            diagnostic
        })
        .collect()
}

pub(super) fn report_validates_gaps(app: &App, gaps: Vec<(crate::span::Span, String)>, detail: &str) {
    for (span, construct) in gaps {
        let file = crate::ide::source(app, span.file).map(|s| s.path.clone()).unwrap_or_default();
        let mut diagnostic = crate::diagnostic::Diagnostic::unsupported(span, None, construct, detail);
        diagnostic.severity = crate::diagnostic::Severity::Warning;
        let message = diagnostic.message.clone();
        super::prism::push(diagnostic);
        survey::record(&IngestError::Unsupported { file, message });
    }
}

/// `uniqueness:` on an attribute naming a polymorphic `belongs_to`
/// (`validates :subject, uniqueness: true`) stays the reported gap. Rails
/// 8.1.4 resolves the attribute to its foreign key through the association's
/// class, which a polymorphic one does not have: validating a record that
/// holds a subject raises `ArgumentError` ("Polymorphic associations do not
/// support computing the class."). Refused as a whole call, as the
/// numericality gap is.
fn gap_uniqueness_on_polymorphic_attribute(app: &mut App) {
    use crate::dialect::{Association, ModelBodyItem, ValidationRule};
    let mut gaps: Vec<(crate::span::Span, String)> = Vec::new();
    for model in &mut app.models {
        let polymorphic = |attr: &crate::ident::Symbol| {
            model
                .associations()
                .any(|a| matches!(a, Association::BelongsTo { name, polymorphic: true, .. } if name == attr))
        };
        let refused: Vec<(crate::span::Span, String)> = model
            .body
            .iter()
            .filter_map(|item| {
                let ModelBodyItem::Validation { validation, .. } = item else { return None };
                (validation.rules.iter().any(|r| matches!(r, ValidationRule::Uniqueness { .. }))
                    && polymorphic(&validation.attribute))
                .then(|| (item.span(), format!("validates :{}, uniqueness:", validation.attribute.as_str())))
            })
            .collect();
        let calls: crate::det::HashSet<(crate::span::FileId, u32, u32)> =
            refused.iter().map(|(span, _)| (span.file, span.start, span.end)).collect();
        model.body.retain(|item| {
            !matches!(item, ModelBodyItem::Validation { .. }) || {
                let span = item.span();
                !calls.contains(&(span.file, span.start, span.end))
            }
        });
        gaps.extend(refused);
    }
    for (span, construct) in gaps {
        let file = crate::ide::source(app, span.file).map(|s| s.path.clone()).unwrap_or_default();
        let mut diagnostic = crate::diagnostic::Diagnostic::unsupported(
            span,
            None,
            construct,
            "the attribute is a polymorphic association, which Rails' uniqueness check cannot resolve to a class; Rails raises ArgumentError validating a record that holds one",
        );
        diagnostic.severity = crate::diagnostic::Severity::Warning;
        let message = diagnostic.message.clone();
        super::prism::push(diagnostic);
        survey::record(&IngestError::Unsupported { file, message });
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
fn splice_concerns_into_models(app: &mut App, carriers: &[ConcernClassMethodSpans]) {
    splice_concerns_into_models_named(app, &[], carriers);
}

/// Splice concern `included do` bodies into models.
///
/// When `only` is empty, every model is visited (the first pass). When
/// non-empty, only models whose [`ClassId`] name is listed — the late
/// `on_load(:active_storage_attachment)` path after Attachment /
/// Markdown synthesis, which must not re-expand includes already
/// spliced on the first pass.
fn splice_concerns_into_models_named(app: &mut App, only: &[crate::ident::Symbol], carriers: &[ConcernClassMethodSpans]) {
    use crate::dialect::ModelBodyItem;
    use crate::expr::ExprNode;

    if app.concern_model_items.is_empty() && app.concern_guarded_items.is_empty() {
        return;
    }

    // A carried statement keeps the concern's lexical constants.
    let mut defined = crate::det::HashSet::default();
    for class in &app.library_classes {
        defined.insert(class.name.0.as_str().to_string());
        for (name, _) in &class.constants {
            defined.insert(format!("{}::{}", class.name.0.as_str(), name.as_str()));
        }
    }
    defined.extend(app.models.iter().map(|model| model.name.0.as_str().to_string()));
    // Each concern's own constants whose value is a literal count or
    // duration, for `inline_concern_literals`.
    let literals: crate::det::HashMap<crate::ident::ClassId, Vec<(crate::ident::Symbol, crate::expr::Expr)>> = app
        .library_classes
        .iter()
        .map(|class| {
            let consts = class
                .constants
                .iter()
                .filter(|(_, value)| crate::lower::duration::literal_seconds(value).is_some())
                .cloned()
                .collect::<Vec<_>>();
            (class.name.clone(), consts)
        })
        .filter(|(_, consts)| !consts.is_empty())
        .collect();

    let dependencies = concern_dependencies(app, carriers);

    for model in &mut app.models {
        if !only.is_empty() && !only.iter().any(|n| n == &model.name.0) {
            continue;
        }
        // Concerns already spliced into this model. A spliced item may
        // itself be an `include` (a concern's `included do include
        // Other end`), whose own items are spliced in turn; a concern
        // is spliced once, so an include cycle terminates.
        let mut spliced: crate::det::HashSet<crate::ident::ClassId> =
            crate::det::HashSet::default();
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
                                ExprNode::Const { path } => Some(concern_named(
                                    model.name.0.as_str(),
                                    path,
                                    &app.concern_model_items,
                                )),
                                _ => None,
                            })
                            .collect()
                    }
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            };
            // A Concern's own Concern includes run on the includer first
            // (concern.rb `append_features`).
            let concern_ids: Vec<crate::ident::ClassId> =
                concern_ids.iter().flat_map(|id| with_dependencies(id, &dependencies, &mut Vec::new())).collect();
            let model_name = model.name.clone();
            let items: Vec<ModelBodyItem> = concern_ids
                .iter()
                .filter(|id| spliced.insert((*id).clone()))
                .flat_map(|id| {
                    // The module's unconditional items with the guarded
                    // ones that admit this model, in source order (a
                    // callback's place in the chain is its order).
                    let plain: &[ModelBodyItem] =
                        app.concern_model_items.get(id).map_or(&[], Vec::as_slice);
                    let guarded: &[crate::app::ConcernGuardedItem] =
                        app.concern_guarded_items.get(id).map_or(&[], Vec::as_slice);
                    let includer = model_name.0.as_str();
                    let mut ordered: Vec<&ModelBodyItem> = Vec::new();
                    for i in 0..=plain.len() {
                        for g in guarded {
                            let here = if i == plain.len() { g.at >= i } else { g.at == i };
                            if here && g.guard.admits(includer) {
                                ordered.push(&g.item);
                            }
                        }
                        if let Some(item) = plain.get(i) {
                            ordered.push(item);
                        }
                    }
                    ordered.into_iter().map(|item| {
                        let mut item = rehome_default_fk(item, id, &model_name);
                        if let ModelBodyItem::Unknown { expr, .. } = &mut item {
                            qualify_carried_constants(expr, id.0.as_str(), &defined);
                            if let Some(consts) = literals.get(id) {
                                inline_concern_literals(expr, consts);
                            }
                        }
                        item
                    }).collect::<Vec<_>>()
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

/// Each ActiveSupport::Concern's includes that are Concerns too: Ruby
/// defers them to the Concern's includer (concern.rb's `@_dependencies`),
/// resolved from the Concern's namespace outward.
fn concern_dependencies(
    app: &App,
    carriers: &[ConcernClassMethodSpans],
) -> crate::det::HashMap<crate::ident::ClassId, Vec<crate::ident::ClassId>> {
    let concerns: crate::det::HashSet<&str> =
        carriers.iter().filter(|c| !c.concern_extensions.is_empty()).map(|c| c.owner.0.as_str()).collect();
    let mut out: crate::det::HashMap<crate::ident::ClassId, Vec<crate::ident::ClassId>> = Default::default();
    for lc in app.library_classes.iter().filter(|lc| lc.is_module && concerns.contains(lc.name.0.as_str())) {
        for include in &lc.includes {
            let written = include.0.as_str();
            let found = match written.strip_prefix("::") {
                Some(top) => concerns.contains(top).then(|| top.to_string()),
                None => {
                    let mut scope = Some(lc.name.0.as_str());
                    let mut found = None;
                    while let Some(prefix) = scope {
                        let candidate = format!("{prefix}::{written}");
                        if concerns.contains(candidate.as_str()) {
                            found = Some(candidate);
                            break;
                        }
                        scope = prefix.rsplit_once("::").map(|(prefix, _)| prefix);
                    }
                    found.or_else(|| concerns.contains(written).then(|| written.to_string()))
                }
            };
            let Some(found) = found.map(|name| crate::ident::ClassId(crate::ident::Symbol::from(name))) else { continue };
            let deps = out.entry(lc.name.clone()).or_default();
            if found != lc.name && !deps.contains(&found) {
                deps.push(found);
            }
        }
    }
    out
}

/// `id` after its dependencies, each after its own.
fn with_dependencies(
    id: &crate::ident::ClassId,
    dependencies: &crate::det::HashMap<crate::ident::ClassId, Vec<crate::ident::ClassId>>,
    seen: &mut Vec<crate::ident::ClassId>,
) -> Vec<crate::ident::ClassId> {
    if seen.contains(id) {
        return Vec::new();
    }
    seen.push(id.clone());
    let mut out: Vec<crate::ident::ClassId> =
        dependencies.get(id).into_iter().flatten().flat_map(|dep| with_dependencies(dep, dependencies, seen)).collect();
    out.push(id.clone());
    out
}

/// An enclosing module's constant keeps its lexical meaning after splicing.
/// The concern's own constants remain reachable through model ancestry.
/// A macro call carried out of a concern's `included do` block reads a
/// constant the concern itself defines (`generates_token_for :transfer,
/// expires_in: TRANSFER_LINK_EXPIRY_DURATION`, with `TRANSFER_LINK_
/// EXPIRY_DURATION = 4.hours` in campfire's `User::Transferable`). The
/// block runs in the includer, but lexically inside the concern, so
/// Rails reads the concern's constant once, as the class loads. When the
/// value is a literal count or duration, the call's argument becomes that
/// literal: the same value at the same moment, and one a macro that folds
/// its option at compile time (`lower::generates_token_for`'s
/// `expires_in:`) can read. Only the call's arguments, never a block it
/// takes, and only a bare reference to the concern's own constant.
fn inline_concern_literals(
    expr: &mut crate::expr::Expr,
    consts: &[(crate::ident::Symbol, crate::expr::Expr)],
) {
    use crate::expr::ExprNode;
    fn inline(arg: &mut crate::expr::Expr, consts: &[(crate::ident::Symbol, crate::expr::Expr)]) {
        match &mut *arg.node {
            ExprNode::Const { path } if path.len() == 1 => {
                if let Some((_, value)) = consts.iter().find(|(name, _)| name == &path[0]) {
                    let span = arg.span;
                    *arg = value.clone();
                    arg.span = span;
                }
            }
            ExprNode::Hash { entries, .. } => {
                for (_, value) in entries.iter_mut() {
                    inline(value, consts);
                }
            }
            _ => {}
        }
    }
    if let ExprNode::Send { recv: None, args, .. } = &mut *expr.node {
        for arg in args.iter_mut() {
            inline(arg, consts);
        }
    }
}

fn qualify_carried_constants(
    expr: &mut crate::expr::Expr,
    concern: &str,
    defined: &crate::det::HashSet<String>,
) {
    use crate::expr::ExprNode;
    let Some((enclosing, _)) = concern.rsplit_once("::") else { return };
    if let ExprNode::Const { path } = &mut *expr.node {
        if path.first().is_some_and(|first| !first.as_str().is_empty()) {
            let written = path.iter().map(|part| part.as_str()).collect::<Vec<_>>().join("::");
            let mut scope = Some(enclosing);
            while let Some(owner) = scope {
                if defined.contains(&format!("{owner}::{written}")) {
                    let mut qualified: Vec<Symbol> = owner.split("::").map(Symbol::from).collect();
                    qualified.append(path);
                    *path = qualified;
                    break;
                }
                scope = owner.rsplit_once("::").map(|(scope, _)| scope);
            }
        }
        return;
    }
    expr.node.for_each_child_mut(&mut |child| qualify_carried_constants(child, concern, defined));
}

/// The concern an `include` arg written inside `owner` names. A rooted
/// `::Blog::Concerns::X` (a leading empty segment) names the top-level
/// `Blog::Concerns::X`, with no lexical lookup. A relative `Concerns::X`
/// resolves from the owner's namespace outward (`Blog::Post::Concerns::X`,
/// then `Blog::Concerns::X`), innermost first, falling back to the path as
/// written. Keyed by the written path, a rooted include matched no
/// concern and its `included do` block vanished from the includer.
fn concern_named<V>(
    owner: &str,
    path: &[crate::ident::Symbol],
    concerns: &crate::det::HashMap<crate::ident::ClassId, V>,
) -> crate::ident::ClassId {
    let written = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
    if let Some(top) = written.strip_prefix("::") {
        return crate::ident::ClassId(crate::ident::Symbol::from(top));
    }
    let mut scope = Some(owner);
    while let Some(prefix) = scope {
        let id = crate::ident::ClassId(crate::ident::Symbol::from(format!("{prefix}::{written}")));
        if concerns.contains_key(&id) {
            return id;
        }
        scope = prefix.rsplit_once("::").map(|(prefix, _)| prefix);
    }
    crate::ident::ClassId(crate::ident::Symbol::from(written))
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
) -> HashMap<crate::ident::ClassId, (Vec<crate::dialect::MethodDef>, crate::det::HashSet<Symbol>)> {
    use crate::det::HashSet;
    let mut carried: HashMap<&crate::ident::ClassId, HashSet<&crate::span::Span>> = HashMap::default();
    for carrier in carriers {
        carried.entry(&carrier.owner).or_default().extend(&carrier.methods);
    }
    let mut class_side: HashMap<_, (Vec<crate::dialect::MethodDef>, HashSet<Symbol>)> = HashMap::default();
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
    use crate::det::{HashMap, HashSet};

    let nested_carriers: HashSet<&ClassId> = carriers.iter()
        .filter(|c| c.has_nested_carrier).map(|c| &c.owner).collect();
    let mut bridges: HashMap<&ClassId, HashSet<&crate::span::Span>> = HashMap::default();
    for carrier in carriers {
        if nested_carriers.contains(&carrier.owner) {
            bridges.entry(&carrier.owner).or_default().extend(&carrier.bridges);
        }
    }
    let class_side = concern_class_method_catalog(&app.library_classes, carriers);
    let mut module_includes: HashMap<ClassId, Vec<ClassId>> = HashMap::default();
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
        let mut seen: HashSet<ClassId> = HashSet::default();
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
        let mut provenance: HashMap<Symbol, ClassId> = HashMap::default();
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

    let mut spliced: HashMap<ClassId, HashMap<Symbol, ClassId>> = HashMap::default();
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
    use std::collections::BTreeSet;
    use crate::det::HashSet;

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
                rest_param: None,
                anonymous_formal: None,
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
/// A `sig { abstract… }` method is a declaration: its `def` is an empty stub
/// and the includer supplies the real one. Spliced into the includer, the
/// stub answered `nil` ahead of the implementation the includer inherits
/// (a concern declaring `abstract.returns(ActionController::Parameters)
/// def params; end` made every includer's `params` nil). The sig stays in
/// `rbs_signatures` as the module's declaration; only the empty body goes.
fn drop_abstract_stubs(
    app: &mut App,
    abstracts: &HashMap<crate::ident::ClassId, crate::det::HashSet<crate::ident::Symbol>>,
) {
    for lc in &mut app.library_classes {
        let Some(names) = abstracts.get(&lc.name) else { continue };
        lc.methods.retain(|m| {
            !(matches!(m.receiver, crate::dialect::MethodReceiver::Instance)
                && names.contains(&m.name)
                && is_empty_body(&m.body))
        });
    }
}

fn is_empty_body(body: &crate::expr::Expr) -> bool {
    match &*body.node {
        crate::expr::ExprNode::Seq { exprs } => exprs.is_empty(),
        crate::expr::ExprNode::Lit { value: crate::expr::Literal::Nil } => true,
        _ => false,
    }
}

/// A concern can include a module from its `included do` block:
///
/// ```text
/// module Authorization
///   extend ActiveSupport::Concern
///   included do
///     include ActionPolicy::Controller
///   end
/// end
/// ```
///
/// Rails runs that block on the controller, so the controller gets
/// `ActionPolicy::Controller` too, placed ahead of `Authorization` in
/// method lookup. Rewriting the controller's `include Authorization` to
/// `include ActionPolicy::Controller, Authorization` gives that order,
/// and also registers `Authorization`'s filters first, because a
/// multi-argument `include` runs its modules' hooks last argument first.
/// The method splice, the class-body macro expansion and the analyzer all
/// read include statements, so this one rewrite reaches all three.
///
/// Models get the same through `splice_concerns_into_models`.
fn carry_included_block_includes_into_controllers(app: &mut App) {
    use crate::dialect::{ControllerBodyItem, ModelBodyItem};
    use crate::expr::{Expr, ExprNode};
    use crate::ident::ClassId;

    // `concern_model_items` holds every concern's `included do`
    // statements, a controller concern's too.
    // One entry per `include` call, its arguments as written.
    let block_includes: HashMap<ClassId, Vec<Vec<Expr>>> = app
        .concern_model_items
        .iter()
        .filter_map(|(module, items)| {
            let calls: Vec<Vec<Expr>> = items
                .iter()
                .filter_map(|item| match item {
                    ModelBodyItem::Unknown { expr, .. } => Some(expr),
                    _ => None,
                })
                .filter_map(|expr| match &*expr.node {
                    ExprNode::Send { recv: None, method, args, block: None, .. } if method.as_str() == "include" => {
                        Some(args.iter().filter(|a| matches!(&*a.node, ExprNode::Const { .. })).cloned().collect())
                    }
                    _ => None,
                })
                .collect();
            (!calls.is_empty()).then(|| (module.clone(), calls))
        })
        .collect();
    if block_includes.is_empty() {
        return;
    }

    fn id_of(expr: &Expr) -> Option<ClassId> {
        match &*expr.node {
            ExprNode::Const { path } => Some(crate::analyze::mixin_path_id(path)),
            _ => None,
        }
    }

    // Ruby puts a later `include` call ahead of an earlier one, hence
    // `rev()` over the calls, but keeps one call's `include A, B` as
    // written, with `A` ahead. A module already on the controller is
    // skipped, which also ends an include cycle.
    fn ahead_of(
        module: &ClassId,
        block_includes: &HashMap<ClassId, Vec<Vec<Expr>>>,
        seen: &mut HashSet<ClassId>,
        out: &mut Vec<Expr>,
    ) {
        for call in block_includes.get(module).into_iter().flatten().rev() {
            for included in call {
                let Some(id) = id_of(included) else { continue };
                if !seen.insert(id.clone()) {
                    continue;
                }
                ahead_of(&id, block_includes, seen, out);
                out.push(included.clone());
            }
        }
    }

    for controller in &mut app.controllers {
        let mut seen: HashSet<ClassId> =
            crate::analyze::controller_includes(controller).into_iter().collect();
        for item in &mut controller.body {
            let ControllerBodyItem::Unknown { expr, .. } = item else { continue };
            let ExprNode::Send { recv: None, method, args, .. } = &mut *expr.node else { continue };
            if method.as_str() != "include" {
                continue;
            }
            let mut carried = Vec::with_capacity(args.len());
            for arg in std::mem::take(args) {
                if let Some(id) = id_of(&arg) {
                    ahead_of(&id, &block_includes, &mut seen, &mut carried);
                }
                carried.push(arg);
            }
            *args = carried;
        }
    }
}

fn splice_concerns_into_controllers(app: &mut App) {
    use crate::dialect::{Action, ControllerBodyItem, MethodReceiver, RenderTarget};
    use crate::ty::{Row, Ty};

    // Instance methods per concern module, the constants those bodies
    // resolve lexically, and the modules it includes. The methods are
    // borrowed, not copied: this indexes every library class, most of
    // which no controller includes, and a method that is spliced has
    // its body cloned below anyway.
    let mut module_methods: HashMap<crate::ident::ClassId, Vec<&crate::dialect::MethodDef>> =
        HashMap::default();
    let mut module_constants: HashMap<
        crate::ident::ClassId,
        crate::det::HashSet<crate::ident::Symbol>,
    > = HashMap::default();
    let mut module_includes: HashMap<crate::ident::ClassId, Vec<crate::ident::ClassId>> =
        HashMap::default();
    for lc in &app.library_classes {
        module_methods.insert(
            lc.name.clone(),
            lc.methods
                .iter()
                .filter(|m| matches!(m.receiver, MethodReceiver::Instance))
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
    > = HashMap::default();
    let mut posts_spliced: crate::det::HashSet<(crate::ident::ClassId, crate::ident::Symbol)> =
        crate::det::HashSet::default();

    // Per controller, the action names a route reaches on it or on a
    // controller that inherits from it. A module method that is PUBLIC
    // and routed here is an action of the includer, as in Rails (a
    // concern's public method is an action like any other); the rest
    // stay behind the private marker, as helpers.
    let routed_here = routed_actions_with_descendants(app);

    for controller in &mut app.controllers {
        let statements = crate::analyze::controller_include_statements(controller);
        let include_groups: Vec<Vec<crate::ident::ClassId>> =
            statements.iter().map(|(_, group)| group.clone()).collect();
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
        // Each module comes with the body index of the `include` that
        // registers it.
        let filter_order: Vec<(usize, crate::ident::ClassId)> = statements
            .iter()
            .zip(filter_registration_by_statement(&include_groups, &module_includes))
            .flat_map(|((at, _), modules)| modules.into_iter().map(move |module| (*at, module)))
            .collect();
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

        let mut defined: crate::det::HashSet<crate::ident::Symbol> = controller
            .body
            .iter()
            .filter_map(|item| match item {
                ControllerBodyItem::Action { action, .. } => Some(action.name.clone()),
                _ => None,
            })
            .collect();

        // Each spliced filter with the body index of its `include`.
        let mut filters: Vec<(usize, crate::ident::ClassId, ControllerBodyItem)> = Vec::new();
        let mut methods: Vec<ControllerBodyItem> = Vec::new();
        let mut actions: Vec<ControllerBodyItem> = Vec::new();
        let mut method_origins = Vec::new();
        let routed = routed_here.get(&controller.name);
        for (at, module) in &filter_order {
            for filter in app.concern_filters.get(module).into_iter().flatten() {
                if let Some(call) = &filter.block {
                    // A block-form filter goes back to being what a
                    // controller's own is: an `Unknown` item holding the
                    // call, which `block_form_filter` lowers and
                    // `build_sourced_filter_chain` chains.
                    let crate::expr::ExprNode::Send { .. } = &*call.node else { continue };
                    filters.push((*at, module.clone(), ControllerBodyItem::Unknown {
                        expr: call.clone(),
                        leading_comments: Vec::new(),
                        leading_blank_line: false,
                    }));
                    continue;
                }
                let mut filter = filter.clone();
                // Provenance for the chain view: `defined_in` is the
                // module, not the controller that included it.
                filter.from_concern = Some(module.clone());
                filters.push((*at, module.clone(), ControllerBodyItem::Filter {
                    filter,
                    leading_comments: Vec::new(),
                    leading_blank_line: false,
                }));
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
                // `Action` keeps `*rest` in its own slot, between the
                // optionals and the keywords; a required positional
                // AFTER it has no slot, and the loop below would move it
                // in front. Ledger that on the module's own def instead
                // (see the end), and splice no misbound copy.
                if method
                    .params
                    .iter()
                    .skip_while(|p| !(p.rest && !p.keyword && !p.forwarding))
                    .skip(1)
                    .any(|p| !p.keyword && !p.rest && !p.forwarding && !p.from_kwrest && !p.from_keyword)
                {
                    posts_spliced.insert((module.clone(), method.name.clone()));
                    continue;
                }
                let mut params = Row::closed();
                let mut opt_params = Vec::new();
                let mut kw_params = Vec::new();
                let mut kwrest_param = None;
                let mut rest_param = None;
                let mut anonymous_formal = None;
                for p in &method.params {
                    if p.forwarding {
                        anonymous_formal = Some(crate::dialect::AnonymousFormal::Forwarding);
                        continue;
                    }
                    if p.keyword && p.rest && p.name.as_str().is_empty() {
                        anonymous_formal = Some(crate::dialect::AnonymousFormal::KeywordRest);
                        continue;
                    }
                    if p.rest && !p.keyword {
                        rest_param = Some(p.name.clone());
                        continue;
                    }
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
                    // An optional keyword the library ingest flattened to
                    // a positional (`from_keyword`, when no required
                    // keyword or rest kept the group) is a keyword in the
                    // source too: the includer's callers pass it by name.
                    if p.keyword || p.from_keyword {
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
                method_origins.push((method.name.clone(), module.clone()));
                let is_action = matches!(method.visibility, crate::dialect::MethodVisibility::Public)
                    && routed.is_some_and(|names| names.contains(&method.name));
                let into = if is_action { &mut actions } else { &mut methods };
                into.push(ControllerBodyItem::Action {
                    action: Action {
                        name_span: method.name_span,
                        name: method.name.clone(),
                        params,
                        opt_params,
                        kw_params,
                        kwrest_param,
                        block_param: method.block_param.as_ref().map(|p| p.name.clone()),
                        rest_param,
                        anonymous_formal,
                        body,
                        renders: RenderTarget::Inferred,
                        effects: crate::effect::EffectSet::pure(),
                    },
                    leading_comments: Vec::new(),
                    leading_blank_line: false,
                });
            }
        }
        if filters.is_empty() && methods.is_empty() && actions.is_empty() {
            continue;
        }
        // Rails runs a concern's `included` block when its `include`
        // runs, so each statement's filters go in just ahead of it:
        // after the includer's own filters declared above it, before
        // those below. Then the routed public methods among the
        // includer's own public actions, then the rest behind a private
        // marker — they're helpers.
        let has_private_marker =
            controller.body.iter().any(|i| matches!(i, ControllerBodyItem::PrivateMarker { .. }));
        let mut actions = Some(actions);
        let mut filters = filters.into_iter().peekable();
        let mut body = Vec::new();
        for (index, item) in std::mem::take(&mut controller.body).into_iter().enumerate() {
            while let Some((_, module, spliced)) = filters.next_if(|(at, _, _)| *at == index) {
                // A block-form filter's provenance rides in the same map
                // the spliced methods use, under the sentinel name the
                // chain builder derives from the item's body index.
                if let ControllerBodyItem::Unknown { expr, .. } = &spliced {
                    if let crate::expr::ExprNode::Send { method, .. } = &*expr.node {
                        let sentinel = crate::ident::Symbol::from(format!(
                            "__{}_block_{}__",
                            method.as_str(),
                            body.len()
                        ));
                        spliced_origin.entry(controller.name.clone()).or_default().insert(sentinel, module);
                    }
                }
                body.push(spliced);
            }
            if matches!(item, ControllerBodyItem::PrivateMarker { .. }) {
                body.extend(actions.take().into_iter().flatten());
            }
            body.push(item);
        }
        body.extend(actions.take().into_iter().flatten());
        if !methods.is_empty() && !has_private_marker {
            body.push(ControllerBodyItem::PrivateMarker {
                leading_comments: Vec::new(),
                leading_blank_line: true,
            });
        }
        body.extend(methods);
        for (name, module) in method_origins {
            spliced_origin.entry(controller.name.clone()).or_default().insert(name, module);
        }
        controller.body = body;
    }
    // The `sig` above a module's `def` describes the COPY as well: the
    // includer's method is that `def`, and inference over the copy would
    // otherwise replace the declared return with whatever the body
    // happens to end in.
    for (controller, origins) in &spliced_origin {
        for (name, module) in origins {
            let Some(sig) = app.rbs_signatures.get(module).and_then(|m| m.get(name)).cloned()
            else {
                continue;
            };
            app.rbs_signatures
                .entry(controller.clone())
                .or_default()
                .entry(name.clone())
                .or_insert(sig);
        }
    }
    for lc in &mut app.library_classes {
        for m in &mut lc.methods {
            if m.unsupported_formals.is_none()
                && matches!(m.receiver, MethodReceiver::Instance)
                && posts_spliced.contains(&(lc.name.clone(), m.name.clone()))
            {
                m.unsupported_formals = Some(crate::dialect::UnsupportedFormal::ControllerPosts);
            }
        }
    }
    app.concern_spliced_actions = spliced_origin;
}

/// Controller -> the actions routes name on it or on any controller
/// below it in the inheritance chain (a subclass dispatches an action
/// its ancestor defines).
fn routed_actions_with_descendants(
    app: &App,
) -> HashMap<crate::ident::ClassId, crate::det::HashSet<crate::ident::Symbol>> {
    let parents: HashMap<&crate::ident::ClassId, &crate::ident::ClassId> = app
        .controllers
        .iter()
        .filter_map(|c| c.parent.as_ref().map(|p| (&c.name, p)))
        .collect();
    let mut out: HashMap<crate::ident::ClassId, crate::det::HashSet<crate::ident::Symbol>> =
        HashMap::default();
    for route in crate::lower::routes::flatten_routes(app) {
        let mut cur = Some(&route.controller);
        let mut depth = 0;
        while let Some(c) = cur {
            out.entry(c.clone()).or_default().insert(route.action.clone());
            depth += 1;
            if depth > 16 {
                break;
            }
            cur = parents.get(c).copied();
        }
    }
    out
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
    filter_registration_by_statement(include_groups, module_includes).into_iter().flatten().collect()
}

/// [`filter_registration_order`], split by the statement that registers
/// each module.
fn filter_registration_by_statement(
    include_groups: &[Vec<crate::ident::ClassId>],
    module_includes: &HashMap<crate::ident::ClassId, Vec<crate::ident::ClassId>>,
) -> Vec<Vec<crate::ident::ClassId>> {
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
        let mut registered = Vec::new();
        for m in group.iter().rev() {
            visit(m, module_includes, &mut seen, &mut registered);
        }
        out.push(registered);
    }
    out
}

/// One ancestry snapshot for both class-body expansion paths. Direct include
/// order is retained for the existing filter-macro lookup contract; transitive
/// membership serves finite class configuration. Instance methods of the same
/// name are a different method (`ParamKey` includes the receiver) and are not
/// part of this snapshot.
pub(super) struct ControllerConcernSurface {
    pub direct_includes: Vec<crate::ident::ClassId>,
    pub includes: Vec<crate::ident::ClassId>,
    pub inherited_includes: Vec<crate::ident::ClassId>,
}

pub(super) struct ControllerConcernSurfaces {
    pub module_includes: HashMap<crate::ident::ClassId, Vec<crate::ident::ClassId>>,
    pub controllers: HashMap<crate::ident::ClassId, ControllerConcernSurface>,
}

pub(super) fn controller_concern_surfaces(app: &App) -> ControllerConcernSurfaces {
    let mut module_includes: HashMap<_, Vec<_>> = HashMap::default();
    for lc in &app.library_classes {
        module_includes.entry(lc.name.clone()).or_default().extend(lc.includes.clone());
    }
    // A class can have several declarations, including an empty reopening
    // used only as a namespace. Its includes and superclass belong to the
    // whole class, not whichever declaration was visited last.
    let mut parts: HashMap<_, Vec<_>> = HashMap::default();
    for controller in &app.controllers {
        parts.entry(&controller.name).or_default().push(controller);
    }
    let mut controllers = HashMap::default();
    for name in parts.keys() {
        let mut direct_includes = Vec::new();
        let mut include_groups = Vec::new();
        let mut inherited_groups = Vec::new();
        let mut cur = Some(*name);
        let mut seen = crate::det::HashSet::default();
        while let Some(c) = cur {
            if !seen.insert(c) {
                break;
            }
            let Some(declarations) = parts.get(c) else { break };
            let mut groups = Vec::new();
            for part in declarations {
                for group in crate::analyze::controller_include_groups(part) {
                    for inc in &group {
                        if !direct_includes.contains(inc) {
                            direct_includes.push(inc.clone());
                        }
                    }
                    groups.push(group);
                }
            }
            // Ancestors register before descendants; each class's statements
            // retain source order, with only a statement's arguments reversed.
            if c != *name {
                inherited_groups.splice(0..0, groups.iter().cloned());
            }
            include_groups.splice(0..0, groups);
            cur = declarations.iter().find_map(|part| part.parent.as_ref());
        }
        controllers.insert((*name).clone(), ControllerConcernSurface {
            includes: filter_registration_order(&include_groups, &module_includes),
            inherited_includes: filter_registration_order(&inherited_groups, &module_includes),
            direct_includes,
        });
    }
    ControllerConcernSurfaces { module_includes, controllers }
}

#[cfg(test)]
mod controller_concern_surface_tests {
    use super::*;

    #[test]
    fn reopened_concern_filters_keep_include_statement_order() {
        for (initial, reopened, expected) in [
            ("include FirstConcern", "include SecondConcern", vec!["first", "second"]),
            ("include FirstConcern, SecondConcern", "include ThirdConcern", vec!["second", "first", "third"]),
        ] {
            let mut tree: crate::vfs::FileTree = [
                ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n".to_string()),
                ("app/controllers/a_order_controller.rb", format!("class OrderController < ActionController::Base\n  {initial}\nend\n")),
                ("app/controllers/b_order_extra.rb", format!("class OrderController\n  {reopened}\nend\n")),
                ("app/controllers/child_controller.rb", "class ChildController < OrderController\n  include ThirdConcern\nend\n".to_string()),
            ]
            .into_iter()
            .map(|(path, source)| (std::path::PathBuf::from(path), source.into_bytes()))
            .collect();
            for (name, target) in [("FirstConcern", "first"), ("SecondConcern", "second"), ("ThirdConcern", "third")] {
                tree.insert(
                    format!("app/controllers/concerns/{target}.rb").into(),
                    format!("module {name}\n  extend ActiveSupport::Concern\n  included {{ before_action :{target} }}\nend\n").into_bytes(),
                );
            }
            let app = crate::ingest::ingest_app_from_tree(tree).expect("ingest");
            let surfaces = controller_concern_surfaces(&app);
            let filters = |name: &str| -> Vec<_> {
                surfaces.controllers[&crate::ident::ClassId(name.into())]
                    .includes.iter()
                    .flat_map(|module| app.concern_filters.get(module).into_iter().flatten())
                    .map(|filter| filter.target.as_str())
                    .collect()
            };
            assert_eq!(filters("OrderController"), expected);
            let mut inherited = expected;
            if !inherited.contains(&"third") {
                inherited.push("third");
            }
            assert_eq!(filters("ChildController"), inherited);
        }
    }

    #[test]
    fn a_namespace_reopening_keeps_own_and_inherited_includes() {
        let tree = [
            ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
            ("app/controllers/base_controller.rb", "class BaseController < ActionController::Base\n  include ParentConcern\nend\n"),
            ("app/controllers/child_controller.rb", "class ChildController < BaseController\n  include ChildConcern\nend\n"),
        ]
        .into_iter()
        .map(|(path, source)| (std::path::PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
        let mut app = crate::ingest::ingest_app_from_tree(tree).expect("ingest");
        // Opening a class just to declare a nested constant contributes no
        // superclass or includes of its own.
        let mut reopening = app.controllers.iter().find(|c| c.name.0.as_str() == "BaseController").unwrap().clone();
        reopening.parent = None;
        reopening.parent_span = Default::default();
        reopening.body.clear();
        app.controllers.push(reopening);
        for _ in 0..2 {
            let surfaces = controller_concern_surfaces(&app);
            let parent = &surfaces.controllers[&crate::ident::ClassId("BaseController".into())];
            assert_eq!(parent.direct_includes, vec![crate::ident::ClassId("ParentConcern".into())]);
            let child = &surfaces.controllers[&crate::ident::ClassId("ChildController".into())];
            assert_eq!(child.direct_includes, vec![
                crate::ident::ClassId("ChildConcern".into()),
                crate::ident::ClassId("ParentConcern".into()),
            ]);
            assert_eq!(child.inherited_includes, vec![crate::ident::ClassId("ParentConcern".into())]);
            app.controllers.reverse();
        }
    }

    #[test]
    fn a_namespaced_source_reopening_keeps_own_and_inherited_includes() {
        let tree = [
            ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
            ("app/controllers/a_base_controller.rb", "module Review\n  class BaseController < ActionController::Base\n    include ParentConcern\n  end\nend\n"),
            ("app/controllers/b_base_extra.rb", "module Review\n  class BaseController\n    include ExtraConcern\n  end\nend\n"),
            ("app/controllers/c_base_nested.rb", "module Review\n  class BaseController\n    class Nested; end\n  end\nend\n"),
            ("app/controllers/d_child_controller.rb", "module Review\n  class ChildController < BaseController\n    include ChildConcern\n  end\nend\n"),
        ]
        .into_iter()
        .map(|(path, source)| (std::path::PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
        let mut app = crate::ingest::ingest_app_from_tree(tree).expect("ingest");
        let base = crate::ident::ClassId("Review::BaseController".into());
        let child = crate::ident::ClassId("Review::ChildController".into());
        // The namespace-only opening in `c_base_nested.rb` is dropped.
        assert_eq!(app.controllers.iter().filter(|c| c.name == base).count(), 2);
        for _ in 0..2 {
            let surfaces = controller_concern_surfaces(&app);
            let parent = &surfaces.controllers[&base];
            let child = &surfaces.controllers[&child];
            for included in ["ParentConcern", "ExtraConcern"] {
                let id = crate::ident::ClassId(included.into());
                assert!(parent.direct_includes.contains(&id));
                assert!(child.inherited_includes.contains(&id));
            }
            assert!(child.direct_includes.contains(&crate::ident::ClassId("ChildConcern".into())));
            app.controllers.reverse();
        }
    }
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
    consts: &crate::det::HashSet<crate::ident::Symbol>,
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
        let own: crate::det::HashSet<String> = model
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
                own: &crate::det::HashSet<String>,
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
    // Read by `view_paths`, which reports a form it cannot evaluate.
    "prepend_view_path",
    "append_view_path",
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
    // `ingest::inertia` records its own survey gap, with the reason, for
    // an `inertia_share` it cannot lower; the generic gap would count it
    // a second time without the reason.
    "inertia_share",
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
    // `with_options(kw) do … end` — `ingest::controller::
    // ingest_controller_with_options` claims every one of these at
    // ingest time, before this scan ever runs: composed, it folds into
    // the `Filter`/other body items its block held and never becomes an
    // `Unknown` at all; declined, it stays `Unknown` WITH its own
    // specific "with_options: …" gap already recorded (ALL-OR-NOTHING —
    // see that function's doc comment). Either way this generic bucket
    // (and the block-shaped one just below) would only double it.
    "with_options",
];

/// `using SomeRefinement` — Ruby's block-scoped monkey-patch
/// mechanism. Recognized so it earns its own ledger line rather than
/// the generic "not recognized" one, but never modeled: refinement
/// semantics (a method visible only within the `using` scope) have no
/// analogue in any target here, and pretending the refined methods
/// exist everywhere would be worse than not modeling them at all.
const REFINEMENT_MACROS: &[&str] = &["using"];

/// `wrap_parameters` in a form ParamsWrapper's lowering reads
/// (`false`, a name, a model, `format:`/`include:`/`exclude:`/`name:`):
/// consumed by `lower::controller_to_library::params_wrapper`, so not a
/// survey line. Any other form keeps the line, and the lowering does not
/// guess at it.
fn is_recognized_wrap_parameters(method: &str, args: &[crate::expr::Expr]) -> bool {
    method == "wrap_parameters"
        && crate::lower::controller_to_library::params_wrapper::is_recognized_wrap_parameters_call(args)
}

/// `serialization_scope nil` (active_model_serializers'
/// `ActionController::Serialization`): the serializers a render builds
/// get no scope, so `current_user` is never called for one. Its only
/// reader is a `render json: …, serializer:` render, which the lowering
/// refuses (`refuse_serializer_options`), so no lowered action depends
/// on it. A named scope (`serialization_scope :current_account`) stays
/// the gap.
fn is_serialization_scope_nil(method: &str, args: &[crate::expr::Expr]) -> bool {
    method == "serialization_scope"
        && matches!(args, [a] if matches!(&*a.node, crate::expr::ExprNode::Lit { value: crate::expr::Literal::Nil }))
}

/// Class-level `respond_to :json` is consumed as a no-op when nothing
/// in this controller's ANCESTOR CHAIN (self included) calls
/// `respond_with` — the only thing the responders gem's `respond_to`
/// declaration ever feeds. `respond_with` isn't modeled, so a
/// controller that actually calls it is left to fail there (an
/// ordinary unresolved-method error at analyze time), not silenced by
/// this check.
fn respond_to_macro_is_consumed(
    controller: &crate::dialect::Controller,
    all: &[crate::dialect::Controller],
) -> bool {
    use crate::det::HashSet;
    let mut seen: HashSet<crate::ident::ClassId> = HashSet::default();
    let mut cur = Some(controller);
    while let Some(c) = cur {
        if !seen.insert(c.name.clone()) {
            break;
        }
        if c.actions().any(|a| expr_calls_respond_with(&a.body)) {
            return false;
        }
        cur = c.parent.as_ref().and_then(|p| all.iter().find(|x| &x.name == p));
    }
    true
}

/// Does `expr` contain a receiverless (or `self.`) call to
/// `respond_with` anywhere in its tree?
fn expr_calls_respond_with(expr: &crate::expr::Expr) -> bool {
    use crate::expr::ExprNode;
    if let ExprNode::Send { recv, method, .. } = &*expr.node
        && method.as_str() == "respond_with"
        && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
    {
        return true;
    }
    let mut found = false;
    expr.node.for_each_child(&mut |c| {
        if !found && expr_calls_respond_with(c) {
            found = true;
        }
    });
    found
}

/// Rails' `ActionDispatch::ContentSecurityPolicy` directive names — the
/// methods a `content_security_policy do |p| … end` block calls on its
/// block parameter to build the policy Rails would send as a response
/// header.
const CSP_DIRECTIVES: &[&str] = &[
    "base_uri",
    "block_all_mixed_content",
    "child_src",
    "connect_src",
    "default_src",
    "font_src",
    "form_action",
    "frame_ancestors",
    "frame_src",
    "img_src",
    "manifest_src",
    "media_src",
    "navigate_to",
    "object_src",
    "plugin_types",
    "prefetch_src",
    "report_uri",
    "require_trusted_types_for",
    "sandbox",
    "script_src",
    "script_src_attr",
    "script_src_elem",
    "style_src",
    "style_src_attr",
    "style_src_elem",
    "trusted_types",
    "upgrade_insecure_requests",
    "worker_src",
];

/// The message `csp_block_is_unrecognized`'s SENDER (`report_unrecognized_
/// controller_macros` below) records for a `content_security_policy` block
/// this module cannot read — kept as a constant so
/// `project::futamura_whole_route`'s gap filter and this recorder can't
/// drift apart. See that filter's doc comment for why the message, not the
/// `IngestError` variant, is what tells the two kinds of record apart: both
/// are `IngestError::Unsupported`, and only one of them blocks a route.
pub(crate) const CSP_DIVERGENCE_MESSAGE: &str =
    "content_security_policy: not emitted, the lowered app sends no CSP header";

/// Is `blk` a `content_security_policy` block roundhouse can read well
/// enough to know it does nothing but build a policy object — every
/// statement a direct call on the block's own parameter, naming a known
/// CSP directive, with symbol/string/simple-literal arguments? Rails'
/// directive builders are themselves not modelled (the lowered app sends
/// no `Content-Security-Policy` header either way — tracked as a
/// TEMPORARY divergence on rubys/roundhouse#58 until CSP is modelled), so
/// this is never about faithfully building the policy; it's about telling
/// a block this predictable (no conditionals, no dynamic nonce generator,
/// no method-call argument) apart from one that might be doing something
/// this ingest gap would otherwise hide without a trace.
pub(crate) fn csp_block_is_recognized(blk: &crate::expr::Expr) -> bool {
    use crate::expr::ExprNode;
    let ExprNode::Lambda { params, rest_param, block_param, extra_params, body, .. } = &*blk.node
    else {
        return false;
    };
    if rest_param.is_some() || block_param.is_some() || !extra_params.is_empty() {
        return false;
    }
    let [p] = params.as_slice() else { return false };
    fn stmt_ok(stmt: &crate::expr::Expr, param: &crate::ident::Symbol) -> bool {
        use crate::expr::ExprNode;
        let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*stmt.node
        else {
            return false;
        };
        let ExprNode::Var { name, .. } = &*recv.node else { return false };
        name == param
            && CSP_DIRECTIVES.contains(&method.as_str())
            && args.iter().all(|a| matches!(&*a.node, ExprNode::Lit { .. }))
    }
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().all(|s| stmt_ok(s, p)),
        _ => stmt_ok(body, p),
    }
}

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
        // `sources::drain` already ran; use the App snapshot via the
        // canonical FileId helper (thread-local `sources::path_of` misses).
        let file_of = |file_id: crate::span::FileId| {
            crate::ide::source(app, file_id)
                .map(|s| s.path.clone())
                .unwrap_or_else(|| controller.name.0.as_str().to_string())
        };
        for item in &controller.body {
            let ControllerBodyItem::Unknown { expr, .. } = item else { continue };
            let ExprNode::Send { recv: None, method, args, block, .. } = &*expr.node else {
                continue;
            };
            if CONSUMED_CONTROLLER_MACROS.contains(&method.as_str()) {
                continue;
            }
            // A class-body call carrying a BLOCK that no lowering
            // claims — the same "vanished with no trace" shape #778
            // was: `around_block_filter` already claims a block-form
            // `around_action` (successfully lowered, so it is no
            // longer `Unknown` at all by the time this runs; or
            // refused, which records its OWN specific gap already —
            // either way this generic bucket must not ALSO flag it).
            // `rescue_from`/`helper_method`/`layout` (with or without a
            // block) are already excluded above via
            // `CONSUMED_CONTROLLER_MACROS`, since that list is keyed on
            // the method name alone. `respond_to` at class-body level
            // writes no block in any fixture here, but is excluded on
            // the same reasoning in case one ever does. Concern-only
            // shapes (`included do`, `class_methods do`) never reach
            // this loop at all — it walks `app.controllers`, and
            // those two live on the CONCERN module, consumed before
            // the splice ever copies anything controller-side.
            //
            // `before_action`/`after_action`/`prepend_before_action` are
            // NOT skipped by name alone: unlike `around_action` (whose
            // every refusal already records its own gap via
            // `around_block_filter`), `lambda_filter_target` can decline
            // one of these silently — e.g. a `before_action(&callback)`
            // forwarding an existing Proc, which `ir_lambda_body` does
            // not read (that slot holds a `Var`, not a `Lambda`) — and
            // #778 exists to stop exactly that kind of silent drop, not
            // just around_action's. So these three are claimed only
            // when `lambda_filter_target` actually recognizes the call,
            // or when it declined it for the ONE reason that already has
            // its own gap: a `next` the body can't restructure (#779,
            // `next_restructure_refusal`) — recording this generic gap
            // too would double it for the one statement.
            if block.is_some() {
                const NAMED_CLAIMED_CONTROLLER_BLOCKS: &[&str] = &["around_action", "respond_to"];
                let claimed = NAMED_CLAIMED_CONTROLLER_BLOCKS.contains(&method.as_str())
                    || (matches!(
                        method.as_str(),
                        "before_action" | "after_action" | "prepend_before_action"
                    ) && (super::controller::lambda_filter_target(expr).is_some()
                        || super::controller::next_restructure_refusal(expr)));
                if claimed {
                    continue;
                }
                // INTERIM (rubys/roundhouse#58, CSP option 2): a BARE
                // `content_security_policy do |p| … end` whose
                // directives this ingest can read is a recorded
                // DIVERGENCE, not a blocking gap — see
                // `csp_block_is_recognized` and
                // `CSP_DIVERGENCE_MESSAGE`. `args.is_empty()` excludes
                // `content_security_policy only: :player do …`: Rails
                // scopes that policy to one action, which this ingest
                // does not model at all (every action on the
                // controller would wrongly get the same policy, or
                // none), so it keeps today's gap regardless of how
                // plain the block is. Anything stranger (a dynamic
                // directive, conditionals, other code in the block)
                // also keeps today's gap below, unchanged.
                if method.as_str() == "content_security_policy"
                    && args.is_empty()
                    && block.as_ref().is_some_and(csp_block_is_recognized)
                {
                    survey::record(&IngestError::Unsupported {
                        file: file_of(expr.span.file),
                        message: CSP_DIVERGENCE_MESSAGE.to_string(),
                    });
                    continue;
                }
                survey::record(&IngestError::Unsupported {
                    file: file_of(expr.span.file),
                    message: format!(
                        "controller class-body block not recognized: `{}` (its effect is dropped from the output)",
                        method.as_str()
                    ),
                });
                continue;
            }
            // `const` / `prop` belong to a lowered `T::Struct` (or a
            // gem base the sidecar expands). On a real controller they
            // are leftover Sorbet props with no runtime in the emitted
            // tree — name that, rather than the generic "macro not
            // recognized" bucket that also covers unmodeled app DSL.
            if matches!(method.as_str(), "const" | "prop") {
                survey::record(&IngestError::Unsupported {
                    file: file_of(expr.span.file),
                    message: format!(
                        "Sorbet `{}` outside a lowered T::Struct (its effect is dropped from the output)",
                        method.as_str()
                    ),
                });
                continue;
            }
            // Class-level `respond_to :json` (the responders gem) — a
            // pure config declaration: it names the mimes `respond_with`
            // answers, and has no effect of its own. `respond_with`
            // itself isn't modeled, so a controller that actually
            // depends on it keeps failing loudly there (an ordinary
            // unresolved-method error); this macro just configures a
            // call nothing in the chain makes, so it's consumed as a
            // no-op rather than flagged as an unrecognized gap. Distinct
            // from the ACTION-body `respond_to do |format| … end`
            // block, which carries a block and never reaches this
            // (blockless) branch at all.
            if method.as_str() == "respond_to"
                && respond_to_macro_is_consumed(controller, &app.controllers)
            {
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
            if is_recognized_wrap_parameters(method.as_str(), args) {
                continue;
            }
            if is_serialization_scope_nil(method.as_str(), args) {
                continue;
            }
            let file = file_of(expr.span.file);
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
            // Two more causes that already recorded their OWN specific
            // gap for this exact method name — this generic bucket must
            // not ALSO flag the call, which would double it.
            // `stored_options_init`'s writer/reader split is one; a
            // macro whose body held a BLOCK-FORM
            // `around_action` that `controller::around_block_filter`
            // recognized but refused (`app::expand_macro_filters`'
            // `AroundRefusal`) is the other — the same exemption a
            // hand-written `around_action { ... }` refusal already gets
            // just above (`NAMED_CLAIMED_CONTROLLER_BLOCKS`), extended
            // to the macro path, which reaches this blockless-call
            // branch instead (the block lives in the macro's OWN body,
            // not on this call site).
            if survey::recorded().iter().any(|gap| {
                (gap.contains("class configuration") || gap.contains("holds an around_action block that"))
                    && gap.contains(method.as_str())
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
fn expand_class_body_macros(app: &mut App) -> IngestResult<()> {
    use crate::dialect::{ControllerBodyItem, MethodDef, MethodReceiver};
    use crate::expr::ExprNode;

    // No class-side method means no macro to expand. Checked before the
    // concern surface is built so an app with none skips that walk.
    if !app.library_classes.iter().any(|lc| {
        lc.methods.iter().any(|m| matches!(m.receiver, MethodReceiver::Class))
    }) {
        return Ok(());
    }

    let surfaces = controller_concern_surfaces(app);
    let inherited_class_methods: HashMap<_, crate::det::HashSet<_>> = app
        .controllers
        .iter()
        .map(|controller| {
            let mut methods = crate::det::HashSet::default();
            let mut current = Some(controller);
            let mut seen = crate::det::HashSet::default();
            while let Some(ancestor) = current {
                if !seen.insert(&ancestor.name) {
                    break;
                }
                methods.extend(ancestor.body.iter().filter_map(|item| match item {
                    ControllerBodyItem::ClassMethod { method, .. } => Some(method.name.clone()),
                    _ => None,
                }));
                current = ancestor
                    .parent
                    .as_ref()
                    .and_then(|parent| app.controllers.iter().find(|candidate| &candidate.name == parent));
            }
            (controller.name.clone(), methods)
        })
        .collect();

    // Class-side methods per library class — the macro table. Borrowed,
    // not copied: this indexes every library class, most of which no
    // controller includes, and a macro that expands substitutes from
    // the borrowed body. A later reopening of the same name replaces
    // the earlier def, as the owned table did. Library classes are only
    // read here; the loop below mutates controllers.
    let mut macros: HashMap<crate::ident::ClassId, Vec<&MethodDef>> = HashMap::default();
    for lc in &app.library_classes {
        for method in lc
            .methods
            .iter()
            .filter(|m| matches!(m.receiver, MethodReceiver::Class))
        {
            let methods = macros.entry(lc.name.clone()).or_default();
            if let Some(prior) = methods.iter_mut().find(|m| m.name == method.name) {
                *prior = method;
            } else {
                methods.push(method);
            }
        }
    }

    for controller in &mut app.controllers {
        let includes = &surfaces.controllers[&controller.name].direct_includes;
        if includes.is_empty() {
            continue;
        }
        let surface = &surfaces.controllers[&controller.name];
        let mut macro_definitions = HashMap::<crate::ident::Symbol, usize>::default();
        for included in &surface.includes {
            if let Some(methods) = macros.get(included) {
                for method in methods {
                    *macro_definitions.entry(method.name.clone()).or_default() += 1;
                }
            }
        }
        let mut shadowed_macros: crate::det::HashSet<_> = macro_definitions
            .into_iter()
            .filter_map(|(name, definitions)| (definitions > 1).then_some(name))
            .collect();
        shadowed_macros.extend(inherited_class_methods[&controller.name].iter().cloned());
        // Shared with this controller's own hand-written block-form
        // `around_action` pass (`ingest::controller::
        // ingest_controller`), which always runs first:
        // same owner-ident encoding, and a starting sequence number
        // picked up AFTER whatever it already placed, so a macro's own
        // block-form `around_action` never synthesizes the same method
        // name as a hand-written one on this controller.
        let around_block_owner_ident = super::controller::around_block_owner_ident(&controller.name);
        let mut around_block_seq =
            super::controller::next_around_block_seq(&controller.body, &around_block_owner_ident);
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
                    .and_then(|ms| ms.iter().copied().find(|m| &m.name == method))
                    .map(|m| (inc.clone(), m))
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
                other_method == method && stored_options_init(other_expr, macro_def).is_none() && !args.is_empty()
            });
            if method_stores_keyword_rest(macro_def) && !another_unreadable {
                if let Some(init) = stored_options_init(expr, macro_def) {
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
            // A keyword shape that can't be bound (see `substitute_params`) refuses the macro, like any other
            // statement that isn't filter DSL.
            let Some(body) = substitute_params(macro_def, args) else {
                survey::record(&IngestError::Unsupported {
                    file: format!("{}", controller.name.0.as_str()),
                    message: format!(
                        "class-body macro not expanded: `{}` from {} holds a statement that is not filter DSL",
                        method.as_str(),
                        module.0.as_str()
                    ),
                });
                expanded.push(item);
                continue;
            };
            let Some(body) = expand_nested_filter_macros(
                &body,
                &module,
                &macros,
                &shadowed_macros,
                &mut Vec::new(),
            )
            else {
                survey::record(&IngestError::Unsupported {
                    file: format!("{}", controller.name.0.as_str()),
                    message: format!(
                        "class-body macro not expanded: `{}` from {} holds a statement that is not filter DSL",
                        method.as_str(),
                        module.0.as_str()
                    ),
                });
                expanded.push(item);
                continue;
            };
            let param_names: Vec<crate::Symbol> =
                macro_def.params.iter().map(|p| p.name.clone()).collect();
            match expand_macro_filters(
                &body,
                &module,
                &param_names,
                &around_block_owner_ident,
                &mut around_block_seq,
            ) {
                Ok(items) => {
                    let mut comments = leading_comments.clone();
                    let mut blank = *leading_blank_line;
                    for macro_item in items {
                        match macro_item {
                            MacroFilterItem::Filter(filter) => {
                                expanded.push(ControllerBodyItem::Filter {
                                    filter,
                                    leading_comments: std::mem::take(&mut comments),
                                    leading_blank_line: std::mem::take(&mut blank),
                                });
                            }
                            // A block-form filter the macro body wraps —
                            // reconstructed in exactly the shape a
                            // hand-written `before_action(...) { ... }`
                            // would ingest as, so it stays `Unknown` and
                            // rides the SAME path through
                            // `build_filter_preamble` /
                            // `report_unrecognized_controller_macros` that
                            // a literal one takes (both read it via
                            // `lambda_filter_target`). No separate
                            // lowering to keep in step with that one.
                            MacroFilterItem::Block(expr) => {
                                expanded.push(ControllerBodyItem::Unknown {
                                    expr,
                                    leading_comments: std::mem::take(&mut comments),
                                    leading_blank_line: std::mem::take(&mut blank),
                                });
                            }
                            // A block-form `around_action` the macro body
                            // wraps — already fully lowered (synthesized
                            // method, named `Around` filter) by
                            // `controller::around_block_filter` itself,
                            // unlike `Block` above: registered on THIS
                            // controller (where the macro is called), same
                            // as a hand-written one would be.
                            MacroFilterItem::Around(filter, action) => {
                                expanded.push(ControllerBodyItem::Filter {
                                    filter,
                                    leading_comments: std::mem::take(&mut comments),
                                    leading_blank_line: std::mem::take(&mut blank),
                                });
                                expanded.push(ControllerBodyItem::Action {
                                    action,
                                    leading_comments: Vec::new(),
                                    leading_blank_line: false,
                                });
                            }
                        }
                    }
                }
                // A block-form filter's `next` that `restructure_next_in_
                // block` can't lower — narrow enough to name the actual
                // reason, and important enough to hard-fail strict mode
                // rather than silently accept an app whose filter never
                // actually runs: unlike `NotFilterDsl` just below,
                // `block_filter_from_macro_stmt` already accepted this
                // shape as a successfully expanded filter (the refusal
                // used to surface only much later, at lowering time, as
                // a silent drop with no gap at all). Survey mode keeps
                // the same shape every other refusal here has — record
                // and keep the macro call whole.
                Err(MacroExpansionRefusal::NextRestructure { method: blocked_method }) => {
                    let err = IngestError::Unsupported {
                        file: format!("{}", controller.name.0.as_str()),
                        message: format!(
                            "class-body macro not expanded: `{}` from {} holds a `{}` block whose `next` can't be restructured to an if/unless",
                            method.as_str(),
                            module.0.as_str(),
                            blocked_method.as_str()
                        ),
                    };
                    if survey::is_active() {
                        survey::record(&err);
                        expanded.push(item);
                    } else {
                        return Err(err);
                    }
                }
                // A block-form `around_action` `controller::
                // around_block_filter` recognized but refused — same
                // always-record-and-keep-whole shape as `NotFilterDsl`
                // just below (this macro call is one gap, not a hard
                // ingest failure), but with that function's own specific
                // reason instead of the generic message.
                Err(MacroExpansionRefusal::AroundRefusal(reason)) => {
                    survey::record(&IngestError::Unsupported {
                        file: format!("{}", controller.name.0.as_str()),
                        message: format!(
                            "class-body macro not expanded: `{}` from {} holds an around_action block that {reason}",
                            method.as_str(),
                            module.0.as_str()
                        ),
                    });
                    expanded.push(item);
                }
                Err(MacroExpansionRefusal::NotFilterDsl) => {
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
    Ok(())
}

/// Inline same-concern class-method calls inside a filter macro before
/// interpreting its body. Rails concerns commonly compose a macro from
/// another macro (`require_unauthenticated_access` calls
/// `allow_unauthenticated_access`, then adds its own redirect filter).
/// Each nested call is substituted with the same literal-argument rules as
/// the outer call; unknown calls, cycles, or non-filter statements remain a
/// fail-closed refusal in `expand_macro_filters`.
fn expand_nested_filter_macros(
    body: &crate::expr::Expr,
    module: &crate::ident::ClassId,
    macros: &HashMap<crate::ident::ClassId, Vec<&crate::dialect::MethodDef>>,
    shadowed: &crate::det::HashSet<crate::ident::Symbol>,
    stack: &mut Vec<crate::ident::Symbol>,
) -> Option<crate::expr::Expr> {
    use crate::expr::{Expr, ExprNode};

    const MAX_EXPANSION_STATEMENTS: usize = 4096;

    fn expand_statements(
        body: &Expr,
        module: &crate::ident::ClassId,
        macros: &HashMap<crate::ident::ClassId, Vec<&crate::dialect::MethodDef>>,
        shadowed: &crate::det::HashSet<crate::ident::Symbol>,
        stack: &mut Vec<crate::ident::Symbol>,
        remaining: &mut usize,
    ) -> Option<Vec<Expr>> {
        let statements: Vec<&Expr> = match &*body.node {
            ExprNode::Seq { exprs } => exprs.iter().collect(),
            _ => vec![body],
        };
        let mut out = Vec::new();
        for statement in statements {
            if *remaining == 0 {
                return None;
            }
            *remaining -= 1;
            let ExprNode::Send { recv: None, method, args, block: None, .. } = &*statement.node
            else {
                out.push(statement.clone());
                continue;
            };
            let Some(def) = macros
                .get(module)
                .and_then(|methods| methods.iter().find(|candidate| &candidate.name == method))
            else {
                out.push(statement.clone());
                continue;
            };
            // A call in a class method runs with the including controller
            // as `self`; another concern or the controller itself may
            // override this name. Inlining the lexical concern's version
            // would change Ruby's lookup result, so refuse the whole macro.
            if shadowed.contains(method) || stack.len() >= 32 || stack.contains(method) {
                return None;
            }
            stack.push(method.clone());
            let Some(nested) = substitute_params(def, args) else {
                stack.pop();
                return None;
            };
            let expanded =
                expand_statements(&nested, module, macros, shadowed, stack, remaining);
            stack.pop();
            out.extend(expanded?);
        }
        Some(out)
    }

    let mut remaining = MAX_EXPANSION_STATEMENTS;
    let exprs = expand_statements(body, module, macros, shadowed, stack, &mut remaining)?;
    Some(Expr::new(body.span, ExprNode::Seq { exprs }))
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
) -> Option<crate::expr::Expr> {
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
    // `options = actions.extract_options!` on the `*actions` parameter:
    // the trailing Hash of the call is the options, everything before it
    // the actions. Bound here because after substitution the receiver is
    // an Array literal and `actions` below must not still hold the Hash.
    let mut body = macro_def.body.clone();
    let mut extracted: Vec<(crate::ident::Symbol, crate::expr::Expr)> = Vec::new();
    if let (ExprNode::Seq { exprs }, Some((rest_index, rest))) = (
        &mut *body.node,
        macro_def.params.iter().enumerate().find(|(_, p)| p.rest),
    ) {
        let taken = match exprs.first().map(|e| &*e.node) {
            Some(ExprNode::Assign {
                target: crate::expr::LValue::Var { name, .. },
                value,
            }) => match &*value.node {
                ExprNode::Send { recv: Some(r), method, args: a, block: None, .. }
                    if method.as_str() == "extract_options!"
                        && a.is_empty()
                        && matches!(&*r.node, ExprNode::Var { name: n, .. } if n == &rest.name) =>
                {
                    Some(name.clone())
                }
                _ => None,
            },
            _ => None,
        };
        // `*%i[a b]` spreads its elements; any other splat's actions are
        // unknown here, so the statement stays and the macro is refused
        // rather than expanded with those actions missing.
        let spread: Option<Vec<crate::expr::Expr>> = args
            .get(rest_index..)
            .unwrap_or(&[])
            .iter()
            .map(|a| match &*a.node {
                ExprNode::Splat { value } => match &*value.node {
                    ExprNode::Array { elements, .. } => Some(elements.clone()),
                    _ => None,
                },
                _ => Some(vec![a.clone()]),
            })
            .collect::<Option<Vec<_>>>()
            .map(|groups| groups.concat());
        if let (Some(options_name), Some(mut positional)) = (taken, spread) {
            exprs.remove(0);
            let options = match positional.last().map(|a| match &*a.node {
                ExprNode::KeywordSplat { value } => matches!(&*value.node, ExprNode::Hash { .. }),
                ExprNode::Hash { .. } => true,
                _ => false,
            }) {
                Some(true) => {
                    let last = positional.pop().expect("checked");
                    match &*last.node {
                        ExprNode::KeywordSplat { value } => value.clone(),
                        _ => last,
                    }
                }
                _ => crate::expr::Expr::new(span, ExprNode::Hash { entries: vec![], kwargs: false }),
            };
            extracted.push((options_name, options));
            extracted.push((
                rest.name.clone(),
                crate::expr::Expr::new(
                    span,
                    ExprNode::Array { elements: positional, style: Default::default() },
                ),
            ));
        }
    }
    // A NAMED keyword param — one the source declared `name:`/`name:
    // default`, whether or not `library_class` kept it honest
    // (`p.keyword`) or flattened it to a positional-with-default
    // (`p.from_keyword`, see that flag's doc) — has to bind BY NAME from
    // the call's trailing keyword producer, not by position: positional
    // binding alone is what let `retire_endpoint '2022-11-14', only:
    // [:index]` (a `sunset: nil` keyword beside a forwarded `**kwargs`)
    // bind `sunset` to the whole `{only: [:index]}` options Hash and
    // `kwargs` to `{}`, losing the filter's scope and emitting Ruby that
    // does not even parse (`headers['Sunset'] = only: [:index] if
    // only: [:index]`). Every shape below this check still has zero or
    // one keyword-ish param (bare `**options`/`**kwargs`, no named
    // keyword beside it), where positional binding already does the
    // right thing — that branch is untouched so those shapes keep
    // emitting byte-for-byte what they did before.
    let has_named_keyword = macro_def.params.iter().any(|p| {
        !extracted.iter().any(|(n, _)| n == &p.name) && ((p.keyword && !p.rest) || p.from_keyword)
    });
    let mut bindings: Vec<(crate::ident::Symbol, crate::expr::Expr)> = if has_named_keyword {
        bind_params_by_name(macro_def, args, &extracted, span)?
    } else {
        macro_def
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if let Some((_, v)) = extracted.iter().find(|(n, _)| n == &p.name) {
                    return (p.name.clone(), v.clone());
                }
                let value = match args.get(i) {
                    Some(a) => match &*a.node {
                        ExprNode::KeywordSplat { value } => value.clone(),
                        _ => a.clone(),
                    },
                    None if p.default.is_some() => p.default.clone().expect("checked"),
                    // `p.rest` alone doesn't say which: `library_class`'s
                    // `body_forwards_rest` keeps a literally-forwarded
                    // `**kwrest` as `Param::keyword(name, None); p.rest =
                    // true` (NOT flattened to the `from_kwrest`
                    // positional-with-`{}`-default this match's `None =>`
                    // arm below exists for) when the def's own source calls
                    // `(**name)`/`, **name)` — exactly the
                    // `before_action(**kwargs) { ... }` shape a block-form
                    // filter macro forwards through. `p.keyword` is what
                    // tells the two apart: only a plain `*rest` (`keyword:
                    // false`) binds an empty Array when the caller omits
                    // it; a kept `**kwrest` (`keyword: true`) means `{}`,
                    // same as the flattened/defaulted shape just below.
                    None if p.rest && !p.keyword => crate::expr::Expr::new(
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
            .collect()
    };
    let extra: Vec<_> = extracted
        .into_iter()
        .filter(|(n, _)| !bindings.iter().any(|(b, _)| b == n))
        .collect();
    bindings.extend(extra);
    replace(&mut body, &bindings);
    fold_literal_hash_reads(&mut body);
    Some(body)
}

/// True for the call-site expression that supplies keyword arguments —
/// a trailing bare-kwargs Hash (`only: [:index]`, `KeywordHashNode` in
/// the Ruby parser) or an explicit `**`-splat. An explicit `{...} =>`
/// Hash literal (`kwargs: false`) is a plain positional argument, not
/// this — see `ExprNode::Hash`'s doc.
fn is_keyword_producer(e: &crate::expr::Expr) -> bool {
    matches!(
        &*e.node,
        crate::expr::ExprNode::Hash { kwargs: true, .. } | crate::expr::ExprNode::KeywordSplat { .. }
    )
}

/// `substitute_params`'s binding when the macro declares at least one
/// NAMED keyword parameter (kept honest or flattened — see
/// `has_named_keyword`'s comment at the call site). Binds the way Ruby
/// does: positionals from the call's positional args (required,
/// optional-with-default, and at most one `*rest` gathering the
/// middle); named keywords from the call's trailing keyword producer BY
/// NAME, applying defaults when the caller omits one; a trailing
/// `**rest` (kept honest or flattened to `from_kwrest`) collects
/// whatever keys the named params didn't consume.
///
/// Returns `None` — refuse the macro, caller keeps the gap — the moment
/// a shape can't be read back statically: a required keyword the call
/// omits, an unknown keyword with no `**rest` to catch it, a keyword
/// producer that isn't a literal Hash (names unreadable), a computed
/// key (might be any name), or too many/few positional arguments. Never
/// guesses.
fn bind_params_by_name(
    macro_def: &crate::dialect::MethodDef,
    args: &[crate::expr::Expr],
    extracted: &[(crate::ident::Symbol, crate::expr::Expr)],
    span: crate::span::Span,
) -> Option<Vec<(crate::ident::Symbol, crate::expr::Expr)>> {
    use crate::expr::{Expr, ExprNode, Literal};

    let remaining: Vec<&crate::dialect::Param> = macro_def
        .params
        .iter()
        .filter(|p| !extracted.iter().any(|(n, _)| n == &p.name))
        .collect();

    // A real `**kwrest` (`p.keyword && p.rest`) or one `library_class`
    // flattened to a positional default of `{}` (`p.from_kwrest`) — same
    // binding either way, only the emitted shape differs.
    let is_kwrest = |p: &crate::dialect::Param| (p.keyword && p.rest) || p.from_kwrest;
    // A named keyword (`sunset: nil`, required `at:`), kept honest or
    // flattened to a positional default — see `has_named_keyword`.
    let is_named_keyword = |p: &crate::dialect::Param| (p.keyword && !p.rest) || p.from_keyword;

    let positional_params: Vec<&crate::dialect::Param> = remaining
        .iter()
        .copied()
        .filter(|p| !is_kwrest(p) && !is_named_keyword(p))
        .collect();
    let named_keyword_params: Vec<&crate::dialect::Param> =
        remaining.iter().copied().filter(|p| is_named_keyword(p)).collect();
    let kwrest_param: Option<&crate::dialect::Param> = remaining.iter().copied().find(|p| is_kwrest(p));

    // The call's trailing keyword producer, split off the positional
    // args it does not belong to.
    let (positional_args, kw_source): (&[Expr], Option<&Expr>) = match args.last() {
        Some(last) if is_keyword_producer(last) => (&args[..args.len() - 1], Some(last)),
        _ => (args, None),
    };

    // A positional `*splat` at the call site (`retire_endpoint *dates,
    // only: [:index]`) has an element count that isn't known here —
    // binding it to a single param slot would guess (and the resulting
    // body would read `headers['Deprecation'] = *dates`, which isn't
    // what Ruby assigns). Refuse rather than guess; the only splat this
    // function spreads is the literal-array one `substitute_params`
    // already handles for the `extract_options!` rest, above.
    if positional_args.iter().any(|a| matches!(&*a.node, ExprNode::Splat { .. })) {
        return None;
    }

    // --- positional: required / optional-with-default / one `*rest` ---
    let rest_pos = positional_params.iter().position(|p| p.rest && !p.keyword);
    let mut positional_bindings: Vec<(crate::ident::Symbol, Expr)> = Vec::new();
    if let Some(rest_pos) = rest_pos {
        let leading = &positional_params[..rest_pos];
        let rest_param = positional_params[rest_pos];
        let posts = &positional_params[rest_pos + 1..];
        if positional_args.len() < posts.len() {
            return None; // not enough args left for the required posts
        }
        let split = positional_args.len() - posts.len();
        let (for_leading_and_rest, for_posts) = positional_args.split_at(split);
        let mut cursor = 0;
        for p in leading {
            if cursor < for_leading_and_rest.len() {
                positional_bindings.push((p.name.clone(), for_leading_and_rest[cursor].clone()));
                cursor += 1;
            } else if let Some(default) = &p.default {
                positional_bindings.push((p.name.clone(), default.clone()));
            } else {
                return None; // a required positional the call doesn't supply
            }
        }
        let rest_elements: Vec<Expr> = for_leading_and_rest[cursor..].to_vec();
        positional_bindings.push((
            rest_param.name.clone(),
            Expr::new(span, ExprNode::Array { elements: rest_elements, style: Default::default() }),
        ));
        for (p, a) in posts.iter().zip(for_posts.iter()) {
            positional_bindings.push((p.name.clone(), a.clone()));
        }
    } else {
        if positional_args.len() > positional_params.len() {
            return None; // too many positional arguments
        }
        let mut cursor = 0;
        for p in &positional_params {
            if cursor < positional_args.len() {
                positional_bindings.push((p.name.clone(), positional_args[cursor].clone()));
                cursor += 1;
            } else if let Some(default) = &p.default {
                positional_bindings.push((p.name.clone(), default.clone()));
            } else {
                return None; // a required positional the call doesn't supply
            }
        }
    }

    // --- keyword: named params by name, `**rest` gets the leftovers ---
    let entries: Vec<(Expr, Expr)> = match kw_source {
        None => Vec::new(),
        Some(src) => {
            let hash_entries = match &*src.node {
                ExprNode::Hash { entries, .. } => Some(entries),
                ExprNode::KeywordSplat { value } => match &*value.node {
                    ExprNode::Hash { entries, .. } => Some(entries),
                    // A forwarded, non-literal keyword bundle (`**var`):
                    // the names it carries aren't known here, and a
                    // named param needs its name — refuse rather than
                    // guess which key is which.
                    _ => None,
                },
                _ => None,
            };
            let Some(hash_entries) = hash_entries else { return None };
            // A computed key might be any name — same reason.
            if hash_entries
                .iter()
                .any(|(k, _)| !matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { .. } }))
            {
                return None;
            }
            hash_entries.clone()
        }
    };
    let key_name = |e: &Expr| match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        _ => None,
    };
    let mut consumed = vec![false; entries.len()];
    let mut keyword_bindings: Vec<(crate::ident::Symbol, Expr)> = Vec::new();
    for p in &named_keyword_params {
        // A repeated key's LAST entry wins, as Ruby's own Hash literal
        // construction already collapsed it by the time the call runs.
        let found = entries
            .iter()
            .enumerate()
            .rev()
            .find(|(i, (k, _))| !consumed[*i] && key_name(k).as_ref() == Some(&p.name));
        match found {
            Some((_, (_, v))) => {
                // Every entry sharing this name is spent, not just the
                // last one: Ruby's own Hash literal construction already
                // collapsed the earlier duplicates by the time the call
                // runs, so none of them is real data left over for
                // `**rest` — only the bound value is.
                for (j, (k, _)) in entries.iter().enumerate() {
                    if key_name(k).as_ref() == Some(&p.name) {
                        consumed[j] = true;
                    }
                }
                keyword_bindings.push((p.name.clone(), v.clone()));
            }
            None => match &p.default {
                Some(default) => keyword_bindings.push((p.name.clone(), default.clone())),
                None => return None, // a required keyword the call omits
            },
        }
    }
    let leftover: Vec<(Expr, Expr)> = entries
        .iter()
        .enumerate()
        .filter(|(i, _)| !consumed[*i])
        .map(|(_, kv)| kv.clone())
        .collect();
    match kwrest_param {
        Some(p) => {
            // An empty leftover still needs a value bound to the kwrest
            // param, but `kwargs: true` (bare trailing-kwargs form) on
            // zero entries emits nothing at all — wrapped in a `**`
            // splat at the call site, that's `before_action(**)`, which
            // `ruby -c` refuses to parse. `kwargs: false` emits the
            // explicit `{}` a double-splat can actually take.
            let kwargs = !leftover.is_empty();
            keyword_bindings
                .push((p.name.clone(), Expr::new(span, ExprNode::Hash { entries: leftover, kwargs })));
        }
        // A keyword the call passes that no named param claims, and
        // nothing left to catch it — the macro's `**rest` is gone, the
        // keyword wouldn't go anywhere real Ruby wouldn't error on too.
        None if !leftover.is_empty() => return None,
        None => {}
    }

    let mut bindings = positional_bindings;
    bindings.extend(keyword_bindings);
    Some(bindings)
}

/// `{only: [:a]}[:if]` → the value, or nil for an absent key — what the
/// read means once `options` is bound to the call's literal Hash.
fn fold_literal_hash_reads(expr: &mut crate::expr::Expr) {
    use crate::expr::{ExprNode, Literal};
    expr.node.for_each_child_mut(&mut |c| fold_literal_hash_reads(c));
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    let (ExprNode::Hash { entries, .. }, [key]) = (&*r.node, args.as_slice()) else { return };
    let ExprNode::Lit { value: Literal::Sym { value: k } } = &*key.node else { return };
    if method.as_str() != "[]" {
        return;
    }
    // A computed or `**` key might be this one: leave the read, and the
    // filter it feeds is refused instead of losing its guard.
    if entries.iter().any(|(ek, _)| !matches!(&*ek.node, ExprNode::Lit { .. })) {
        return;
    }
    // A repeated key reads its last value, as in Ruby.
    let found = entries.iter().rev().find_map(|(ek, ev)| match &*ek.node {
        ExprNode::Lit { value: Literal::Sym { value } } if value == k => Some(ev.clone()),
        _ => None,
    });
    *expr = found.unwrap_or_else(|| {
        crate::expr::Expr::new(expr.span, ExprNode::Lit { value: Literal::Nil })
    });
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
    // `only:` / `except:` actions, a Symbol or String each as Rails takes
    // them. None when any is something else: dropping it would narrow
    // the list, or empty it into an unscoped filter.
    let action_of = |e: &crate::expr::Expr| match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(crate::ident::Symbol::from(value.as_str())),
        _ => None,
    };
    let sym_list = |e: &crate::expr::Expr| -> Option<Vec<crate::ident::Symbol>> {
        match &*e.node {
            ExprNode::Array { elements, .. } => elements.iter().map(&action_of).collect(),
            _ => action_of(e).map(|a| vec![a]),
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
                Some("only") => only = sym_list(value)?,
                Some("except") => except = sym_list(value)?,
                // if:/unless: guards on a macro-expanded filter would
                // need the predicate to resolve in the INCLUDER; not
                // modeled, and silently dropping a guard changes when a
                // filter fires.
                Some("if") | Some("unless")
                    if !matches!(&*value.node, ExprNode::Lit { value: Literal::Nil }) =>
                {
                    return None
                }
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

/// One macro-body statement's expansion, in declaration order: a
/// Symbol-target filter `filter_from_send` already reads, or a
/// block-form filter `block_filter_from_macro_stmt` folded one down to.
/// Kept as two cases (not flattened to `Filter` alone) so a macro body
/// mixing both kinds — a `skip_before_action :x` beside a
/// `before_action(**kwargs) { ... }` — expands each statement to its own
/// shape without either one pretending to be the other.
enum MacroFilterItem {
    Filter(crate::dialect::Filter),
    Block(crate::expr::Expr),
    /// A block-form `around_action` the macro body wraps, already fully
    /// lowered by `controller::around_block_filter` — a named `Around`
    /// filter plus the synthesized method it dispatches to. Unlike
    /// `Block` (which rides the SAME later path a hand-written
    /// `before_action { ... }` does, via `lambda_filter_target`),
    /// `around_action`'s own lowering has no such shared path to defer
    /// to: `is_lambda_filter_macro` excludes it on purpose (see that
    /// function's doc comment), so this crate's one around-action
    /// lowering is called here directly instead of being duplicated.
    Around(crate::dialect::Filter, crate::dialect::Action),
}

/// Every statement of a (parameter-substituted) macro body, expanded to a
/// `MacroFilterItem`, or `None` if any statement is neither a Symbol-target
/// filter nor a recognized block-form one — the all-or-nothing contract
/// `expand_class_body_macros` relies on to decide whether to expand the
/// whole macro or keep it whole and ledgered.
///
/// A PREFIX of plain-local assignments (`stamp = "@#{date.to_datetime.to_i}"`,
/// `sunset = sunset&.to_date&.httpdate` — the latter reassigning, and so
/// shadowing, a bound parameter of the same name) is folded away before any
/// statement is tried as a filter: each is evaluated with
/// `fold_local_value` against the locals already folded, and the result —
/// always a LITERAL, never executed at runtime (this crate's Spinel target
/// has no `to_datetime`/`httpdate` to call) — both drops the statement and
/// extends the environment later statements (including the filter's own
/// block body, via `substitute_locals`) read it back from. Two
/// environments, not one: `by_name` for a genuinely new local (`stamp` is
/// never a parameter, so its `Var` reads survive substitution untouched);
/// `by_span` for one that shadows a parameter (`sunset` IS one, so
/// `substitute_params` already replaced every read of it, body-wide, with
/// a clone of the call's value — carrying that clone's span — before this
/// function ever ran; see `by_span`'s own doc at its declaration). Which
/// case applies is decided by `param_names` — the macro's OWN declared
/// parameter names, threaded in from the call site's `macro_def` — never
/// by shape alone: a local's value can reach a parameter's literal clone
/// mid-expression (`stamp`'s `date.to_datetime.to_i` does, for the date
/// parameter) without the ASSIGNMENT itself reassigning that parameter,
/// and `by_span` must stay empty for that local or a later, unrelated
/// read sharing the same span (`date` read again in the filter body)
/// would be wrongly rewritten to `stamp`'s value instead of its own. The
/// first statement that is not a plain-local assignment is tried as the
/// filter the existing two branches already read, with every local folded
/// so far substituted in first; anything after it — another local
/// assignment or not — hits the same `return None` an unrecognized
/// statement always has, since neither branch accepts an `Assign`. A
/// local whose value cannot be reduced to a literal (an unknown method, a
/// non-literal date) refuses the whole macro the same way, rather than
/// leave the free variable for
/// `filter_from_send`/`block_filter_from_macro_stmt` to choke on.
///
/// A macro with no such prefix (every macro this crate recognized before
/// this environment existed) never populates either environment, so every
/// statement is tried exactly as before — `substitute_locals` is not even
/// called — which is what keeps #680's and afc2462f's expansions
/// byte-identical.
/// Why `expand_macro_filters` refused a macro body. `expand_class_body_
/// macros` reads this to pick the right message: `NextRestructure` is
/// narrow enough to name the actual reason (and, unlike every other
/// refusal here, to hard-fail strict mode — see its call site), while
/// `NotFilterDsl` keeps the generic message and always-keep-whole
/// behavior every other refusal already had.
enum MacroExpansionRefusal {
    /// A block-form filter statement whose body holds a `next`
    /// `restructure_next_in_block` can't lower to an if/unless — the
    /// same cause `ingest::controller::next_restructure_refusal` names
    /// for a hand-written block, here reached through a concern macro's
    /// own `before_action`/`after_action`/`prepend_before_action`
    /// (`method`) instead. Narrow: never raised for any OTHER reason
    /// `block_filter_from_macro_stmt` refuses a block (an unreadable
    /// `only:`/`except:`/`if:`/`unless:` entry, a shadowed block
    /// parameter, …) — those stay `NotFilterDsl`, same as before this
    /// variant existed.
    NextRestructure { method: crate::Symbol },
    /// A block-form `around_action` statement `controller::
    /// around_block_filter` recognized but refused — the continuation
    /// used for something other than a bare `.call`, 3+ block params, or
    /// an unrepresentable option. Carries that function's own `reason`
    /// string verbatim, the same specific wording a hand-written
    /// `around_action { ... }` refusal already surfaces, rather than
    /// folding it into `NotFilterDsl`'s generic message.
    AroundRefusal(String),
    /// Every other refusal this function already had: an unreducible
    /// local, a statement that's neither Symbol-target nor block-form
    /// filter DSL, one after the filter, or no filter at all.
    NotFilterDsl,
}

fn expand_macro_filters(
    body: &crate::expr::Expr,
    module: &crate::ident::ClassId,
    param_names: &[crate::Symbol],
    owner_ident: &str,
    around_seq: &mut u32,
) -> Result<Vec<MacroFilterItem>, MacroExpansionRefusal> {
    use crate::expr::{ExprNode, LValue};

    let mut out = Vec::new();
    let statements: Vec<&crate::expr::Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };
    let mut by_name: Vec<(crate::Symbol, crate::expr::Expr)> = Vec::new();
    // A local that REASSIGNS a macro parameter (`sunset = sunset&.…`)
    // shadows the bound value — but `substitute_params` already replaced
    // every read of that parameter, body-wide, with a CLONE of its
    // call-site value, before this function ever runs (and must keep
    // doing so unmodified — see this function's own doc). A clone keeps
    // its original's span, and two DISTINCT source literals never share
    // one, so every surviving occurrence of "a read of the shadowed
    // parameter" in a LATER statement is identifiable by span alone:
    // `const_fold_expr` records the span of every `Lit` leaf it folds
    // through, and the ones gathered while folding a reassignment's
    // value are exactly the parameter's pre-fold occurrences, wherever
    // they still appear. `by_name` alone (a `Var` named `sunset`) finds
    // nothing there, because there is no `Var` left to find.
    let mut by_span: Vec<(crate::span::Span, crate::expr::Expr)> = Vec::new();
    let mut seen_filter = false;
    for stmt in statements {
        if !seen_filter {
            if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*stmt.node {
                let mut consumed_spans = Vec::new();
                match fold_local_value(value, &by_name, &mut consumed_spans) {
                    Some(lit) => {
                        by_name.retain(|(n, _)| n != name);
                        by_name.push((name.clone(), lit.clone()));
                        // Only a local that REASSIGNS a macro parameter
                        // (its name is one of `param_names`) has anything
                        // to find by span: `substitute_params` replaced
                        // every read of THAT parameter, body-wide, with a
                        // clone of its call-site value before this
                        // function ever ran, so matching by span finds
                        // those surviving clones. A plain new local
                        // (`stamp`, never a parameter) binds by name only
                        // — its value's `consumed_spans` are some OTHER
                        // read's literal (e.g. `date`'s), and registering
                        // them here would wrongly rewrite that other
                        // read's later occurrences to `stamp`'s value.
                        if param_names.contains(name) {
                            for span in consumed_spans {
                                by_span.retain(|(s, _)| *s != span);
                                by_span.push((span, lit.clone()));
                            }
                        }
                        continue;
                    }
                    // A local that doesn't reduce to a literal is a gap,
                    // same as any other statement that isn't filter DSL:
                    // refuse the whole macro rather than leave it a free
                    // variable downstream.
                    None => return Err(MacroExpansionRefusal::NotFilterDsl),
                }
            }
        }
        let substituted;
        let candidate: &crate::expr::Expr = if by_name.is_empty() && by_span.is_empty() {
            stmt
        } else {
            substituted = {
                let mut s = stmt.clone();
                substitute_locals(&mut s, &by_name, &by_span);
                s
            };
            &substituted
        };
        if let Some(filters) = filter_from_send(candidate, module) {
            out.extend(filters.into_iter().map(MacroFilterItem::Filter));
            seen_filter = true;
            continue;
        }
        // `block_filter_from_macro_stmt` doesn't itself check whether the
        // block's `next` (if any) can be restructured — that's
        // `lambda_filter_target`'s job, reached only later at lowering
        // time, by which point `block_filter_from_macro_stmt` has
        // already succeeded at reconstructing the block and this macro
        // would otherwise be accepted as expanded with no gap recorded
        // at all. Catch that one cause here, BEFORE accepting the
        // block, narrowly, so it refuses with a located message instead
        // of silently riding through as a successfully expanded filter.
        if let ExprNode::Send { recv: None, method, block: Some(_), .. } = &*candidate.node {
            if super::controller::is_lambda_filter_macro(method.as_str())
                && super::controller::next_restructure_refusal(candidate)
            {
                return Err(MacroExpansionRefusal::NextRestructure { method: method.clone() });
            }
            // `around_action`'s block form is excluded from `is_lambda_
            // filter_macro` on purpose (see its doc comment): a
            // before/after block inlines as a plain conditional
            // statement, but an around has to run the rest of the
            // filter chain from inside its own body, which that path
            // can't express. `controller::around_block_filter` is the
            // ONE lowering for that shape, reused here rather than
            // duplicated — same refusals, same synthesized-method
            // machinery, just reached through a macro's (already
            // parameter-substituted) body instead of a controller's own
            // source. `raw_text: None`: this statement came from the
            // macro's IR, not from source text in THIS file, so the one
            // check that reads source text (the `.()` sugar spelling —
            // see `around_block_filter`'s doc comment) is skipped.
            if method.as_str() == "around_action" {
                match super::controller::around_block_filter(candidate, None, owner_ident, around_seq)
                {
                    Some(Ok((filter, action))) => {
                        out.push(MacroFilterItem::Around(filter, action));
                        seen_filter = true;
                        continue;
                    }
                    Some(Err(reason)) => return Err(MacroExpansionRefusal::AroundRefusal(reason)),
                    // Not actually this shape (shouldn't happen given the
                    // `method.as_str() == "around_action"` check above) —
                    // fall through to the generic refusal rather than
                    // silently drop the statement.
                    None => return Err(MacroExpansionRefusal::NotFilterDsl),
                }
            }
        }
        if let Some(block) = block_filter_from_macro_stmt(candidate) {
            out.push(MacroFilterItem::Block(block));
            seen_filter = true;
            continue;
        }
        return Err(MacroExpansionRefusal::NotFilterDsl);
    }
    if out.is_empty() { Err(MacroExpansionRefusal::NotFilterDsl) } else { Ok(out) }
}

/// Replace every `Var` read in `expr` naming one of `by_name`'s locals,
/// and every `Lit` whose span is one of `by_span`'s, with the literal it
/// folded to — the same blind, scope-unaware full-tree rewrite
/// `substitute_params::replace` already does for macro parameters, run a
/// second time for the locals `expand_macro_filters` folds on top of
/// that substitution. Safe for the same reason `replace` is: `by_name`'s
/// names are the macro body's own locals, not ones a nested block could
/// re-declare out from under it in any of the shapes this crate expands
/// (a reused block-param name would shadow correctly in real Ruby and
/// wrongly here, same pre-existing caveat `rewrite_var_to_self_ref`
/// documents); `by_span`'s spans each identify one SOURCE location — two
/// distinct ones never collide, and two clones of the same one (which is
/// exactly what a shadowed parameter's surviving occurrences are) always
/// do.
fn substitute_locals(
    expr: &mut crate::expr::Expr,
    by_name: &[(crate::Symbol, crate::expr::Expr)],
    by_span: &[(crate::span::Span, crate::expr::Expr)],
) {
    use crate::expr::ExprNode;
    match &*expr.node {
        ExprNode::Var { name, .. } => {
            if let Some((_, value)) = by_name.iter().rev().find(|(n, _)| n == name) {
                *expr = value.clone();
                return;
            }
        }
        ExprNode::Lit { .. } => {
            if let Some((_, value)) = by_span.iter().rev().find(|(s, _)| *s == expr.span) {
                *expr = value.clone();
                return;
            }
        }
        _ => {}
    }
    expr.node.for_each_child_mut(&mut |c| substitute_locals(c, by_name, by_span));
}

/// A local assignment's value, folded to a literal against the locals
/// already bound in `by_name` — `None` if any part of it cannot be
/// reduced, which `expand_macro_filters` reads as "refuse the macro".
/// Every `Lit` leaf the fold passes through (its own value, or one
/// reached via a chained method call) is appended to `consumed_spans` —
/// see `by_span`'s doc at its declaration in `expand_macro_filters` for
/// why that identifies a shadowed parameter's other occurrences.
fn fold_local_value(
    value: &crate::expr::Expr,
    by_name: &[(crate::Symbol, crate::expr::Expr)],
    consumed_spans: &mut Vec<crate::span::Span>,
) -> Option<crate::expr::Expr> {
    let mut scratch: Vec<(crate::Symbol, FoldedValue)> =
        by_name.iter().map(|(n, e)| (n.clone(), FoldedValue::Lit(e.clone()))).collect();
    match const_fold_expr(value, &mut scratch, consumed_spans)? {
        FoldedValue::Lit(e) => Some(e),
        // `.to_datetime`/`.to_date` with no following `.to_i`/`.httpdate`
        // to finish the chain: not a value this macro-folding DSL
        // understands as terminal, so the local can't be bound.
        FoldedValue::DateTimeStr(_) | FoldedValue::DateStr(_) => None,
    }
}

/// An intermediate result of folding a macro-local's value at compile
/// time. `Lit` is a real, emittable Ruby literal; the other two variants
/// exist only to carry a validated `YYYY-MM-DD` string between the two
/// links of the one two-call chain each models (`.to_datetime.to_i`,
/// `.to_date.httpdate`) — see `fold_date_method`. Neither survives past
/// `fold_local_value`: a local whose value is still one of them (the
/// chain's second call never came) refuses the macro rather than bind a
/// value this crate has no literal shape for.
#[derive(Clone)]
enum FoldedValue {
    Lit(crate::expr::Expr),
    DateTimeStr(String),
    DateStr(String),
}

impl FoldedValue {
    /// Ruby truthiness: `nil` and `false` are the only falsy values,
    /// which only `Lit` can ever hold (the date-chain intermediates are
    /// always truthy, same as any other real object).
    fn is_truthy(&self) -> bool {
        use crate::expr::{ExprNode, Literal};
        match self {
            FoldedValue::Lit(e) => !matches!(
                &*e.node,
                ExprNode::Lit { value: Literal::Nil } | ExprNode::Lit { value: Literal::Bool { value: false } }
            ),
            FoldedValue::DateTimeStr(_) | FoldedValue::DateStr(_) => true,
        }
    }
}

/// Reduce `expr` to a `FoldedValue` at compile time, reading `env` (both
/// the macro's folded locals and, mid-expression, the safe-navigation
/// temps `&.` desugars to — see `ingest::expr`'s `__safe_nav<pos>`
/// locals) for `Var` reads, and pushing new ones for `Assign`. Only the
/// shapes `fold_local_value`'s two callers actually need are modeled —
/// everything else is a refusal, never a guess, so a date-folding mistake
/// is a missed expansion rather than a wrong `Deprecation`/`Sunset`
/// header value.
///
/// Every `Lit` leaf reached — whether written directly in the macro
/// source or substituted in for a parameter read — appends its span to
/// `consumed_spans`; see `by_span`'s doc at its declaration in
/// `expand_macro_filters` for why.
fn const_fold_expr(
    expr: &crate::expr::Expr,
    env: &mut Vec<(crate::Symbol, FoldedValue)>,
    consumed_spans: &mut Vec<crate::span::Span>,
) -> Option<FoldedValue> {
    use crate::expr::{BoolOpKind, ExprNode, LValue};

    match &*expr.node {
        ExprNode::Lit { .. } => {
            consumed_spans.push(expr.span);
            Some(FoldedValue::Lit(expr.clone()))
        }
        ExprNode::Var { name, .. } => {
            env.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.clone())
        }
        ExprNode::StringInterp { parts } => {
            let mut out = String::new();
            for part in parts {
                match part {
                    crate::expr::InterpPart::Text { value } => out.push_str(value),
                    crate::expr::InterpPart::Expr { expr } => {
                        let FoldedValue::Lit(lit) = const_fold_expr(expr, env, consumed_spans)? else {
                            // An interpolated `.to_date`/`.to_datetime`
                            // with no finishing call — not a string this
                            // DSL can render.
                            return None;
                        };
                        out.push_str(&literal_to_s(&lit)?);
                    }
                }
            }
            Some(FoldedValue::Lit(crate::expr::Expr::new(
                expr.span,
                ExprNode::Lit { value: crate::expr::Literal::Str { value: out } },
            )))
        }
        // `nil&.x` / `false&.x` (Ruby's only safe-nav-guarded values) —
        // `ingest::expr` desugars `a&.b` to `a && a.b` (or
        // `(tmp = a) && tmp.b` for a non-plain-read receiver), so a
        // literal-nil/false left operand already means the right-hand
        // call never ran; short-circuit the same way here rather than
        // fold into a call on nil.
        ExprNode::BoolOp { op: BoolOpKind::And, left, right, .. } => {
            let l = const_fold_expr(left, env, consumed_spans)?;
            if !l.is_truthy() {
                return Some(l);
            }
            const_fold_expr(right, env, consumed_spans)
        }
        // The safe-nav temp's own binding (`__safe_nav17 = <recv>`):
        // fold the value and extend the SCRATCH env the same way a
        // macro local extends the persistent one, so the paired read a
        // few nodes over resolves.
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            let v = const_fold_expr(value, env, consumed_spans)?;
            env.push((name.clone(), v.clone()));
            Some(v)
        }
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. } if args.is_empty() => {
            let recv = const_fold_expr(recv, env, consumed_spans)?;
            fold_date_method(&recv, method.as_str(), expr.span)
        }
        _ => None,
    }
}

/// The one method pair each of `FoldedValue::DateTimeStr`/`DateStr`
/// exists for: `'YYYY-MM-DD'.to_datetime.to_i` (Integer Unix seconds at
/// UTC midnight — what `String#to_datetime` parses a date-only string
/// to, offset-less so UTC, per Rails 8.1/ActiveSupport) and
/// `'YYYY-MM-DD'.to_date.httpdate` (`Date#httpdate`'s RFC 2822 string,
/// always `00:00:00 GMT` — a `Date` carries no time of day). Both calls
/// on anything else — a non-literal receiver, extra args, a method this
/// chain doesn't use — refuse rather than guess.
fn fold_date_method(recv: &FoldedValue, method: &str, span: crate::span::Span) -> Option<FoldedValue> {
    use crate::expr::{ExprNode, Literal};

    match (recv, method) {
        (FoldedValue::Lit(e), "to_datetime") => {
            let ExprNode::Lit { value: Literal::Str { value } } = &*e.node else { return None };
            parse_ymd(value)?;
            Some(FoldedValue::DateTimeStr(value.clone()))
        }
        (FoldedValue::Lit(e), "to_date") => {
            let ExprNode::Lit { value: Literal::Str { value } } = &*e.node else { return None };
            parse_ymd(value)?;
            Some(FoldedValue::DateStr(value.clone()))
        }
        (FoldedValue::DateTimeStr(date), "to_i") => {
            let (y, m, d) = parse_ymd(date)?;
            let seconds = days_from_civil(y, m, d) * 86_400;
            Some(FoldedValue::Lit(crate::expr::Expr::new(
                span,
                ExprNode::Lit { value: Literal::Int { value: seconds } },
            )))
        }
        (FoldedValue::DateStr(date), "httpdate") => {
            let (y, m, d) = parse_ymd(date)?;
            Some(FoldedValue::Lit(crate::expr::Expr::new(
                span,
                ExprNode::Lit { value: Literal::Str { value: format_httpdate(y, m, d) } },
            )))
        }
        _ => None,
    }
}

/// A Ruby literal's `to_s` — what a `#{}` interpolation actually renders.
/// Only the shapes `fold_date_method`'s own results (and a literal
/// already in the body) can produce: `Integer#to_s` and (vacuously)
/// `String#to_s`. Not a general `Object#to_s`: a receiver this crate
/// cannot fold has already refused the macro higher up.
fn literal_to_s(e: &crate::expr::Expr) -> Option<String> {
    use crate::expr::{ExprNode, Literal};
    match &*e.node {
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Int { value } } => Some(value.to_string()),
        ExprNode::Lit { value: Literal::Nil } => Some(String::new()),
        ExprNode::Lit { value: Literal::Bool { value } } => Some(value.to_string()),
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        _ => None,
    }
}

/// Strict `YYYY-MM-DD` parse (four digits, `-`, two digits, `-`, two
/// digits — nothing else, no the loose leading-digits `String#to_i`
/// would accept) plus calendar validity (real month, real day for that
/// month/year, leap years included). `retire_endpoint`'s date arrives as
/// a string literal from the controller's call site; a value this loose
/// about its own shape is exactly where "fold a wrong header value"
/// would come from; refusing is always the safe alternative.
fn parse_ymd(s: &str) -> Option<(i64, u32, u32)> {
    let bytes = s.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        if s[r.clone()].bytes().all(|b| b.is_ascii_digit()) { s[r].parse().ok() } else { None }
    };
    let year = digits(0..4)? as i64;
    let month = digits(5..7)?;
    let day = digits(8..10)?;
    if !(1..=12).contains(&month) {
        return None;
    }
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }
    Some((year, month, day))
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: u32) -> u32 {
    const DAYS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if month == 2 && is_leap_year(year) { 29 } else { DAYS[(month - 1) as usize] }
}

/// Days since the Unix epoch (1970-01-01) for a proleptic-Gregorian
/// civil date — Howard Hinnant's `days_from_civil`
/// (<https://howardhinnant.github.io/date_algorithms.html>, public
/// domain), the standard exact algorithm so this doesn't quietly drift
/// from ActiveSupport on some year this crate's test dates don't cover.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (i64::from(m) + 9) % 12; // [0, 11]
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// `Date#httpdate`'s RFC 2822 rendering — `Thu, 01 Oct 2026 00:00:00
/// GMT` — always midnight: a `Date` has no time component to carry.
fn format_httpdate(y: i64, m: u32, d: u32) -> String {
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    // 1970-01-01 (day 0 of `days_from_civil`) was a Thursday.
    const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    let days = days_from_civil(y, m, d);
    let weekday = WEEKDAYS[(days.rem_euclid(7)) as usize];
    format!("{weekday}, {d:02} {month} {y:04} 00:00:00 GMT", month = MONTHS[(m - 1) as usize])
}

/// Try a macro-body statement (already parameter-substituted) as a
/// BLOCK-FORM `before_action` / `after_action` / `prepend_before_action` —
/// the shape `filter_from_send` cannot read (it accepts only a Symbol
/// target), but which `ingest::controller::lambda_filter_target` already
/// recognizes on a hand-written controller body. Mastodon's `vary_by`
/// (and this crate's reduced fixture, `stamp_header`) write exactly this
/// idiom in a concern's `class_methods do`, to let the macro accept either
/// a literal value or a lambda evaluated against the controller:
///
/// ```text
/// def stamp_header(value, **kwargs)
///   before_action(**kwargs) do |controller|
///     response.headers['X-Stamp'] =
///       value.respond_to?(:call) ? controller.instance_exec(&value) : value
///   end
/// end
/// ```
///
/// After `substitute_params`, `value` is a literal or a `Lambda` — never a
/// free variable — so `value.respond_to?(:call)` is statically decidable.
/// Folding it collapses the ternary to one branch; when that branch is the
/// `instance_exec` one, inlining a zero-param, control-flow-free lambda's
/// body in its place leaves an ordinary block-form filter. It is
/// reconstructed in the exact shape `lambda_filter_target` reads off a
/// hand-written one, so it rides the SAME path through
/// `build_filter_preamble` (and is excluded from
/// `report_unrecognized_controller_macros` the same way) — no separate
/// lowering for this one to fall out of step with.
///
/// A block declaring its own `|controller|` parameter is folded to use
/// `self` instead: Rails runs an arity-1 filter block as
/// `instance_exec(controller)`, which rebinds `self` to the controller
/// AND passes it as the block's first argument — the same value twice.
/// `ir_lambda_body` (which every consumer of a block-form filter's body
/// reads it through) keeps only the body, discarding the declared
/// parameter list entirely, so a reference to that parameter by name has
/// to become a `self`-based one or it would dangle.
///
/// A folded-away `nil` (the lambda guard returns false, so the ternary's
/// literal `value` — not the lambda — would have run, except here the
/// LAMBDA branch is the one that ran, and ITS body returned nil) means
/// the filter sets the header to nil. The runtime's HeaderStore drops a
/// nil-valued header, where Rails/Puma would still send an (empty-valued)
/// one; accepted here as a pre-existing runtime gap, not something this
/// expansion introduces or could reasonably paper over.
///
/// ALL-OR-NOTHING, same direction as `filters_from_macro_body`: `None` on
/// a block declaring 2+ params or a rest/block param (no
/// `instance_exec(controller, ...)` convention to fall back on), on a
/// `respond_to?(:call)` whose receiver isn't statically a literal or a
/// `Lambda` (can't fold it, so the ternary can't collapse, so there is no
/// safe single filter body to emit), or on an `instance_exec` that isn't
/// this exact zero-param / `return`-`next`-`break`-free shape (those exit
/// the ENCLOSING macro method in source, not the filter — inlining the
/// body would change what they do).
fn block_filter_from_macro_stmt(stmt: &crate::expr::Expr) -> Option<crate::expr::Expr> {
    use crate::expr::{Expr, ExprNode, Literal};

    let ExprNode::Send { recv: None, method, args, block: Some(blk), parenthesized } = &*stmt.node
    else {
        return None;
    };
    if !super::controller::is_lambda_filter_macro(method.as_str()) {
        return None;
    }
    let ExprNode::Lambda { params, rest_param, block_param, extra_params, body, block_style } = &*blk.node
    else {
        return None;
    };
    // Only options may ride beside a block target, and only options
    // `lambda_filter_target` can actually read back off the expanded
    // block-form filter it reconstructs below. A positional arg that is
    // neither a bare options Hash nor a `**`-splatted one is a shape
    // `before_action`'s block form never takes; refused outright.
    //
    // Beyond the Hash shape, every ENTRY is checked too — this is the
    // difference from `filter_from_send`'s Symbol-target path, which
    // never had this gap because `lambda_filter_target` only loosely
    // parses `only:`/`except:`/`if:`/`unless:` (and silently ignores any
    // other key): a value shape it can't read back doesn't fail, it
    // just quietly becomes "no scope"/"no guard", which would make the
    // expanded filter run with the WRONG scope or guard instead of not
    // expanding — a correctness regression the all-or-nothing contract
    // exists to prevent. So refuse here, unexpanded, unless every entry
    // is one `lambda_filter_target` is known to read faithfully:
    //   * key is a Symbol literal in {only, except, if, unless};
    //   * only/except: a Symbol literal, or an Array of Symbol literals;
    //   * if/unless: a Symbol literal, or exactly the lambda shapes
    //     `ir_lambda_body` reads (`-> { … }` / `lambda { … }` /
    //     `proc { … }`).
    // `lambda_filter_target` itself keeps its existing loose parsing —
    // that's the pre-existing gap for a HAND-WRITTEN
    // `before_action(if: 'cond') { … }`, a separate issue.
    for a in args {
        let unwrapped = match &*a.node {
            ExprNode::KeywordSplat { value } => value,
            _ => a,
        };
        let ExprNode::Hash { entries, .. } = &*unwrapped.node else {
            return None;
        };
        for (k, v) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                return None;
            };
            let shape_ok = match key.as_str() {
                "only" | "except" => is_symbol_or_symbol_array(v),
                "if" | "unless" => {
                    super::controller::ir_symbol(v).is_some()
                        || super::controller::ir_lambda_body(v).is_some()
                }
                _ => false,
            };
            if !shape_ok {
                return None;
            }
        }
    }
    if rest_param.is_some() || block_param.is_some() || !extra_params.is_empty() || params.len() > 1 {
        return None;
    }

    let mut body = body.clone();
    if let [self_param] = params.as_slice() {
        // `rewrite_var_to_self_ref` is a blind full-tree rewrite: it
        // turns every `Var` read matching `self_param`'s name into
        // `SelfRef`, with no notion of scope. Refused, rather than
        // risked, whenever `body_shadows_param` finds either a nested
        // block/lambda that redeclares the same name as its own
        // parameter (`items.map { |controller| controller.name }`
        // would wrongly become `items.map { |controller| self.name }`)
        // or a plain reassignment of that name anywhere in the body
        // (`controller = nil`) — no scope-aware rewrite
        // exists here to tell any of those apart from a genuine read of
        // the filter block's own parameter.
        if body_shadows_param(&body, self_param) {
            return None;
        }
        rewrite_var_to_self_ref(&mut body, self_param);
    }

    let mut ok = true;
    // Three SEPARATE full-tree passes, strictly in this order. Folding
    // `respond_to?(:call)` first, then collapsing the now-literal-cond
    // `If`, means a dead branch's `instance_exec` is physically gone from
    // the tree before the instance_exec pass ever looks at it — the
    // literal-value case's `value.instance_exec(&"a string")`-shaped
    // dead code never has to be (and cannot be) folded, only discarded.
    fold_respond_to_call(&mut body, &mut ok);
    fold_literal_if(&mut body);
    fold_self_instance_exec(&mut body, &mut ok);
    if !ok {
        return None;
    }

    Some(Expr::new(
        stmt.span,
        ExprNode::Send {
            recv: None,
            method: method.clone(),
            args: args.clone(),
            block: Some(Expr::new(
                blk.span,
                ExprNode::Lambda {
                    params: vec![],
                    rest_param: None,
                    block_param: None,
                    extra_params: Vec::new(),
                    body,
                    block_style: block_style.clone(),
                },
            )),
            parenthesized: *parenthesized,
        },
    ))
}

/// `only:`/`except:` shape `lambda_filter_target` reads faithfully via
/// `ir_symbol_list`: a bare Symbol literal, or an Array whose elements
/// are ALL Symbol literals. Stricter than `ir_symbol_list` itself, which
/// silently drops any element that isn't one (so `only: ['index']`
/// would read back as an EMPTY list, scoping the filter to no actions
/// at all rather than refusing) — exactly the loose-parsing gap this
/// function exists to keep `block_filter_from_macro_stmt` out of.
pub(super) fn is_symbol_or_symbol_array(e: &crate::expr::Expr) -> bool {
    use crate::expr::ExprNode;
    match &*e.node {
        ExprNode::Array { elements, .. } => {
            elements.iter().all(|el| super::controller::ir_symbol(el).is_some())
        }
        _ => super::controller::ir_symbol(e).is_some(),
    }
}

/// `true` if `expr` holds, ANYWHERE at any depth, either of two things
/// that make rewriting every `name`-named `Var` read to `self` unsafe:
///
///   * a Lambda node — a nested block or a nested `->`/`lambda`/`proc`
///     literal — that redeclares `name` as one of its own parameters
///     (required, rest, or block-capture). That nested scope shadows
///     the outer binding of the same name, so its body's reads of
///     `name` mean ITS OWN parameter, not the filter block's.
///   * an assignment (`Assign`, `OpAssign`, or a `MultiAssign` target)
///     whose target is a local variable named `name`. Even a PLAIN
///     reassignment in the SAME scope (`controller = nil`) means every
///     read after that point is of whatever was assigned, not the
///     filter block's own parameter — rewriting it to `self` would be
///     just as wrong as the lambda-shadowing case, only without a new
///     scope to blame.
///
/// A nested block-local (`|x; name|`) needs no check here: ingest has
/// already renamed it apart (`scope_block_locals` in `ingest/expr.rs`),
/// so its reads and writes no longer carry `name`.
///
/// See `block_filter_from_macro_stmt`'s call site for why this refuses
/// rather than rewriting scope-aware — no scope-aware rewrite exists
/// here to tell a shadowed/reassigned binding from the filter block's
/// own parameter.
pub(super) fn body_shadows_param(expr: &crate::expr::Expr, name: &crate::ident::Symbol) -> bool {
    use crate::expr::{ExprNode, LValue};
    match &*expr.node {
        ExprNode::Lambda { params, rest_param, block_param, .. } => {
            if params.iter().any(|p| p == name)
                || rest_param.as_ref() == Some(name)
                || block_param.as_ref() == Some(name)
            {
                return true;
            }
        }
        ExprNode::Assign { target: LValue::Var { name: n, .. }, .. }
        | ExprNode::OpAssign { target: LValue::Var { name: n, .. }, .. }
            if n == name =>
        {
            return true;
        }
        ExprNode::MultiAssign { targets, .. }
            if targets.iter().any(|t| matches!(t, LValue::Var { name: n, .. } if n == name)) =>
        {
            return true;
        }
        _ => {}
    }
    let mut found = false;
    expr.node.for_each_child(&mut |c| {
        if !found && body_shadows_param(c, name) {
            found = true;
        }
    });
    found
}

/// Rewrite every read of `name` (a block's own declared parameter) to
/// `SelfRef` — see `block_filter_from_macro_stmt`'s doc comment for why.
pub(super) fn rewrite_var_to_self_ref(expr: &mut crate::expr::Expr, name: &crate::ident::Symbol) {
    use crate::expr::ExprNode;
    if let ExprNode::Var { name: n, .. } = &*expr.node {
        if n == name {
            *expr = crate::expr::Expr::new(expr.span, ExprNode::SelfRef);
            return;
        }
    }
    expr.node.for_each_child_mut(&mut |c| rewrite_var_to_self_ref(c, name));
}

/// `<X>.respond_to?(:call)` → `true`/`false` when `X` is statically
/// knowable (a `Lambda`, or any other literal-ish value a lambda never
/// is); leaves it alone and sets `*ok = false` otherwise. Receiver
/// shapes besides `Lambda` (a `Var`, another `Send`, a `Const`, ...)
/// can't be judged without running the program, so the whole macro is
/// refused rather than guessed at.
fn fold_respond_to_call(expr: &mut crate::expr::Expr, ok: &mut bool) {
    use crate::expr::{ExprNode, Literal};
    expr.node.for_each_child_mut(&mut |c| fold_respond_to_call(c, ok));
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "respond_to?" {
        return;
    }
    let [sym] = args.as_slice() else { return };
    let ExprNode::Lit { value: Literal::Sym { value } } = &*sym.node else { return };
    if value.as_str() != "call" {
        return;
    }
    let callable = match &*r.node {
        ExprNode::Lambda { .. } | ExprNode::MethodRef { .. } => true,
        ExprNode::Lit { .. } | ExprNode::Array { .. } | ExprNode::Hash { .. } | ExprNode::StringInterp { .. } => {
            false
        }
        _ => {
            *ok = false;
            return;
        }
    };
    *expr = crate::expr::Expr::new(expr.span, ExprNode::Lit { value: Literal::Bool { value: callable } });
}

/// `If { cond: Lit::Bool(b), then_branch, else_branch }` → whichever
/// branch `b` selects. Bottom-up so a nested `If` this same fold already
/// collapsed is visible to its parent.
fn fold_literal_if(expr: &mut crate::expr::Expr) {
    use crate::expr::{ExprNode, Literal};
    expr.node.for_each_child_mut(&mut |c| fold_literal_if(c));
    let ExprNode::If { cond, then_branch, else_branch } = &*expr.node else {
        return;
    };
    // Ruby's only two falsy values: `nil` is what a folded-away `sunset:`
    // local leaves behind in `… if sunset` once `expand_macro_filters`
    // has substituted the literal in — a guard that never had an `unless`
    // counterpart to confuse it with.
    let truthy = match &*cond.node {
        ExprNode::Lit { value: Literal::Bool { value } } => *value,
        ExprNode::Lit { value: Literal::Nil } => false,
        _ => return,
    };
    *expr = if truthy { then_branch.clone() } else { else_branch.clone() };
}

/// `self.instance_exec(&<lambda>)` (receiver `SelfRef`, or implicit self)
/// → the lambda's body, when the lambda takes no parameters and its body
/// has no `return`/`next`/`break` (those would exit the enclosing macro
/// METHOD in the original source, not this filter, so inlining them
/// would change what they do). Sets `*ok = false` on any other
/// `instance_exec` shape — a non-self receiver is left alone (not this
/// pattern), everything else means the call survived folding and cannot
/// be executed symbolically.
fn fold_self_instance_exec(expr: &mut crate::expr::Expr, ok: &mut bool) {
    use crate::expr::ExprNode;
    expr.node.for_each_child_mut(&mut |c| fold_self_instance_exec(c, ok));
    let ExprNode::Send { recv, method, args, block: Some(blk), .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "instance_exec" || !args.is_empty() {
        return;
    }
    let recv_is_self = match recv {
        None => true,
        Some(r) => matches!(&*r.node, ExprNode::SelfRef),
    };
    if !recv_is_self {
        return;
    }
    let ExprNode::Lambda { params, rest_param, block_param, body: lam_body, .. } = &*blk.node else {
        *ok = false;
        return;
    };
    if !params.is_empty() || rest_param.is_some() || block_param.is_some() {
        *ok = false;
        return;
    }
    if contains_return_next_break(lam_body) {
        *ok = false;
        return;
    }
    *expr = lam_body.clone();
}

/// Deep scan for `return`/`next`/`break` anywhere in `expr` — see
/// `fold_self_instance_exec`.
fn contains_return_next_break(expr: &crate::expr::Expr) -> bool {
    use crate::expr::ExprNode;
    if matches!(&*expr.node, ExprNode::Return { .. } | ExprNode::Next { .. } | ExprNode::Break { .. }) {
        return true;
    }
    let mut found = false;
    expr.node.for_each_child(&mut |c| {
        if !found && contains_return_next_break(c) {
            found = true;
        }
    });
    found
}

/// Copy each concern's `enum` columns onto the models that include it,
/// the same way the DSL splice copies its `included do` items. Campfire
/// declares `enum :role` in `User::Role`, and `User::Bot` — a different
/// concern — queries it with `where(role: :bot)`, so the table has to be
/// whole before any label can be mapped.
fn fold_concern_enums_into_models(
    app: &mut App,
    concern_enums: &[(crate::ident::ClassId, Vec<ConcernEnumDecl>)],
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
            for decl in decls {
                model
                    .enums
                    .entry(decl.column.clone())
                    .or_insert_with(|| decl.mapping.clone());
                if let Some(d) = &decl.default {
                    model
                        .enum_defaults
                        .entry(decl.column.clone())
                        .or_insert_with(|| d.clone());
                }
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
        HashMap::default();
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
            if !polymorphic_targets.is_empty() {
                continue;
            }
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

/// Set the target of each `has_many …, through:` association that no
/// `class_name:` or `source_type:` names to the class of its source
/// association, as Rails' `ThroughReflection#derive_class_name` does.
/// `has_many :article_remarks, through: :articles, source: :remarks`,
/// with `Article` `has_many :remarks, class_name: "Comment"`, reads
/// `Comment` rows. Ingest guessed `Remark` from the name.
///
/// [`Model::through_source`](crate::dialect::Model::through_source)
/// finds the source association. The guess stays when the app does not
/// have the through model or the source association, or when the source
/// is a polymorphic `belongs_to` (only `source_type:` names that class).
/// The pass repeats so that a source association that is itself a
/// `through:` gives its resolved class.
fn resolve_through_targets(app: &mut App) {
    use crate::dialect::{Association, ModelBodyItem};
    use crate::ident::ClassId;

    fn target_of(assoc: &Association) -> Option<&ClassId> {
        match assoc {
            Association::BelongsTo { polymorphic: true, .. } => None,
            Association::BelongsTo { target, .. }
            | Association::HasMany { target, .. }
            | Association::HasOne { target, .. }
            | Association::HasAndBelongsToMany { target, .. } => Some(target),
        }
    }

    for _ in 0..app.models.len() {
        let mut updates: Vec<(usize, usize, ClassId)> = Vec::new();
        for (i, model) in app.models.iter().enumerate() {
            for (j, item) in model.body.iter().enumerate() {
                let ModelBodyItem::Association { assoc, .. } = item else { continue };
                let Association::HasMany {
                    target, through: Some(through), class_explicit: false, ..
                } = assoc
                else {
                    continue;
                };
                let resolved = model
                    .associations()
                    .filter(|a| a.name() == through)
                    .last()
                    .and_then(target_of)
                    .and_then(|class| app.models.iter().find(|m| &m.name == class))
                    .and_then(|through_model| through_model.through_source(assoc))
                    .and_then(target_of);
                if let Some(resolved) = resolved.filter(|r| *r != target) {
                    updates.push((i, j, resolved.clone()));
                }
            }
        }
        if updates.is_empty() {
            return;
        }
        for (i, j, resolved) in updates {
            if let ModelBodyItem::Association {
                assoc: Association::HasMany { target, .. }, ..
            } = &mut app.models[i].body[j]
            {
                *target = resolved;
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
                // `.css.erb` joins them too: a CSS responder with no
                // HTML sibling (Mastodon's `custom_css#show`, served
                // through `render content_type: "text/css"` with no
                // template name — `resolve_formats_only_render` binds
                // that to THIS action's own template). Lowered as
                // `<action>_css`, same naming answer.
                if stem.ends_with(".html")
                    || !stem.contains('.')
                    || matches!(
                        format,
                        Some("turbo_stream" | "svg" | "text" | "json" | "js" | "pdf" | "csv" | "txt" | "css" | "md")
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
                    record_skipped_view(&path, &format!("{e} (non-html format)"))?;
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
            // Template engines we don't ingest yet — they hold Ruby the
            // analyzer never sees (`.rabl`). Record
            // so the hole is visible to `--continue` and the LSP/MCP.
            // Moving one of these into `ViewEngine::from_extension` (above)
            // is the whole walker-side change to support a new engine.
            Some("rabl") => {
                record_skipped_view(&path, ext.expect("matched a Some arm"))?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Record an un-ingested view template as a survey gap under
/// `--continue` (and the LSP/MCP, which now ingest in survey mode) —
/// makes the HAML / `.text.erb` / `.ruby` coverage hole visible instead
/// of letting whole template files vanish without a trace. STRICT mode
/// now aborts instead of silently dropping the template: it used to
/// call `survey::record` directly, a no-op outside survey, so a
/// still-unsupported format (`.rabl`, or any non-html ERB
/// format this allow-list doesn't carry) emitted the controller with
/// no view and reported no error — the one silent-drop `check`'s error
/// gate is supposed to rule out.
fn record_skipped_view(path: &Path, engine: &str) -> IngestResult<()> {
    survey::continue_or_fail(IngestError::Unsupported {
        file: path.display().to_string(),
        message: format!("view template not ingested: {engine}"),
    })
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
/// The Ruby an app runs at boot, before it loads models and draws
/// routes: `config/application.rb`, `config/environment.rb`,
/// `config/boot.rb`, each `config/environments/*.rb` (whichever one
/// runs), every `config/initializers` file, and each file one of those
/// loads by a literal top-level `require_relative`. A file already
/// recorded as an input is left out, so its writes are not counted twice.
/// The targets of `file`'s literal top-level `require_relative` calls,
/// under [`boot_required_files`]' checks: an existing `.rb` file inside the
/// app, reached without a symbolic link, other than the boot chain itself.
/// Silent: a computed or outside path is simply not followed.
fn literal_require_relative_targets<V: Vfs + ?Sized>(vfs: &V, dir: &Path, file: &Path) -> Vec<PathBuf> {
    let config_dir = dir.join("config");
    let boot_chain = ["boot.rb", "application.rb", "environment.rb"].map(|name| config_dir.join(name));
    let Some(base) = file.parent() else { return Vec::new() };
    let Ok(source) = vfs.read(file) else { return Vec::new() };
    let parsed = super::prism::parse_silent(&source);
    let Some(program) = parsed.node().as_program_node() else { return Vec::new() };
    let mut out = Vec::new();
    for statement in program.statements().body().iter() {
        let Some(call) = statement.as_call_node() else { continue };
        if call.receiver().is_some()
            || call.block().is_some()
            || super::util::constant_id_str(&call.name()) != "require_relative"
        {
            continue;
        }
        let arguments: Vec<_> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        let Some(mut target) = (arguments.len() == 1)
            .then(|| super::util::string_value(&arguments[0]))
            .flatten()
            .and_then(|request| normalize_engine_library_require(dir, base, &request))
        else {
            continue;
        };
        if target.extension().is_none() {
            target.set_extension("rb");
        }
        if !boot_chain.contains(&target)
            && target.extension().is_some_and(|e| e == "rb")
            && vfs.exists(&target)
            && !vfs.is_dir(&target)
            && !path_has_symlink_component(vfs, dir, &target)
            && !out.contains(&target)
        {
            out.push(target);
        }
    }
    out
}

fn boot_source_files<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    inputs: &crate::det::HashSet<PathBuf>,
) -> IngestResult<Vec<PathBuf>> {
    let config = dir.join("config");
    let mut files: Vec<PathBuf> = ["application.rb", "environment.rb", "boot.rb"]
        .iter()
        .map(|name| config.join(name))
        .filter(|path| vfs.exists(path) && !vfs.is_dir(path))
        .collect();
    for sub in ["environments", "initializers"] {
        let sub_dir = config.join(sub);
        if vfs.is_dir(&sub_dir) {
            files.extend(read_rb_files(vfs, &sub_dir)?);
        }
    }
    // What the boot chain, an environment file or an initializer loads by
    // a literal `require_relative` runs at boot too. Followed here with
    // `boot_required_files`' checks, silently: that function reports the
    // paths it can't follow for `application.rb` / `environment.rb` once,
    // and feeds the support-root batch, which this list does not.
    // To the end of each chain (`a.rb` requires `b.rb`), each file once.
    let mut visited: crate::det::HashSet<PathBuf> = files.iter().cloned().collect();
    let mut pending: Vec<PathBuf> = files.clone();
    while let Some(file) = pending.pop() {
        for target in literal_require_relative_targets(vfs, dir, &file) {
            if visited.insert(target.clone()) {
                files.push(target.clone());
                pending.push(target);
            }
        }
    }
    files.retain(|file| !inputs.contains(file));
    files.sort();
    files.dedup();
    Ok(files)
}

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
pub(super) fn path_has_symlink_component<V: Vfs + ?Sized>(vfs: &V, root: &Path, path: &Path) -> bool {
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

/// The app's `roundhouse.yml`. An absent file is the default configuration.
#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RoundhouseConfig {
    test_paths: Vec<PathBuf>,
    /// Extension directories (`crate::extensions`), in listing order.
    extensions: Vec<PathBuf>,
    spinel: SpinelConfig,
}

#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SpinelConfig {
    int_overflow: Option<String>,
}

/// Read and parse the app's `roundhouse.yml`.
fn roundhouse_config<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<RoundhouseConfig> {
    let config_path = dir.join("roundhouse.yml");
    let text = match vfs.read_to_string(&config_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(RoundhouseConfig::default()),
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
    Ok(config.unwrap_or_default())
}

/// A configured directory as a literal app-relative path, `.` segments
/// dropped. `None` for an empty path, an absolute one, or one with `..`.
fn app_relative_dir(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!normalized.as_os_str().is_empty()).then_some(normalized)
}

/// Additional test roots from the app's `roundhouse.yml`.
fn additional_test_paths(dir: &Path, config: &RoundhouseConfig) -> IngestResult<Vec<PathBuf>> {
    let config_path = dir.join("roundhouse.yml");
    let mut paths = Vec::new();
    for path in &config.test_paths {
        let Some(normalized) = app_relative_dir(path) else {
            return Err(IngestError::Parse {
                file: config_path.display().to_string(),
                message: format!(
                    "test_paths entries must be non-empty app-relative directories without '..': {path:?}"
                ),
            });
        };
        paths.push(normalized);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// The facts of the extensions `roundhouse.yml` lists, loaded for this app
/// alone (`crate::extensions`). Entries follow the `test_paths` rules, and
/// listing order is load order, so an entry listed twice is refused rather
/// than silently loaded twice or deduplicated.
fn project_extensions<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    config: &RoundhouseConfig,
) -> IngestResult<crate::extensions::Extensions> {
    let config_path = dir.join("roundhouse.yml");
    let refuse = |message: String| IngestError::Parse { file: config_path.display().to_string(), message };
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in &config.extensions {
        let Some(relative) = app_relative_dir(entry) else {
            return Err(refuse(format!(
                "extensions entries must be non-empty app-relative directories without '..': {entry:?}"
            )));
        };
        let resolved = dir.join(relative);
        if path_has_symlink_component(vfs, dir, &resolved) {
            return Err(refuse(format!("extensions entries must not contain symbolic links: {entry:?}")));
        }
        if dirs.contains(&resolved) {
            return Err(refuse(format!("extensions entries must be listed once: {entry:?}")));
        }
        dirs.push(resolved);
    }
    crate::extensions::load(vfs, &dirs).map_err(|error| IngestError::Parse { file: error.file, message: error.message })
}

/// The classes from `classes` that are NESTED under `outer` — the ones
/// a model's or controller's own body walk skipped. The outer class
/// itself is ingested by its own pass and must not be registered twice.
/// Add one file's concern items to the app's, per module. A concern can
/// be reopened in another file, and every `included do` block it has runs
/// for the includer, so a second file's items are appended to the first's
/// rather than replacing them.
fn append_concern_items(
    app: &mut App,
    items: Vec<(crate::ident::ClassId, Vec<crate::dialect::ModelBodyItem>)>,
    guarded: super::library_class::ConcernGuardedItems,
    keep: impl Fn(&crate::ident::ClassId) -> bool,
) {
    // A guarded item's `at` counts the unconditional items before it in
    // its own file; offset it by what earlier files already hold.
    for (id, items) in guarded.into_iter().filter(|(id, _)| keep(id)) {
        let offset = app.concern_model_items.get(&id).map_or(0, Vec::len);
        app.concern_guarded_items.entry(id).or_default().extend(
            items.into_iter().map(|g| crate::app::ConcernGuardedItem { at: g.at + offset, ..g }),
        );
    }
    for (id, items) in items.into_iter().filter(|(id, _)| keep(id)) {
        app.concern_model_items.entry(id).or_default().extend(items);
    }
}

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

/// Files `config/application.rb` and `config/environment.rb` load with a
/// literal top-level `require_relative`, in file order.
///
/// Rails runs both files top to bottom before any initializer, so a
/// constant defined in a file they load is defined for the whole process.
/// An app that keeps boot-time settings in `config/site_settings.rb` and
/// writes `require_relative "site_settings"` beside `require "rails/all"`
/// has nothing in an autoload path, and no support root reached the file:
/// every reference to its constants reported `constant not supported`
/// (#1013).
///
/// `boot`, `application` and `environment` are the boot chain itself, not
/// support code. A target must be an existing `.rb` file inside the app,
/// reached without a symbolic link. A computed path, or one outside the
/// app, is not followed; survey mode records it as a gap.
fn boot_required_files<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> Vec<PathBuf> {
    let config_dir = dir.join("config");
    let boot_chain = ["boot.rb", "application.rb", "environment.rb"].map(|name| config_dir.join(name));
    let mut out = Vec::new();
    for name in ["application.rb", "environment.rb"] {
        let file = config_dir.join(name);
        let Ok(source) = vfs.read(&file) else { continue };
        let parsed = super::prism::parse_silent(&source);
        let Some(program) = parsed.node().as_program_node() else { continue };
        for statement in program.statements().body().iter() {
            let Some(call) = statement.as_call_node() else { continue };
            if call.receiver().is_some()
                || call.block().is_some()
                || super::util::constant_id_str(&call.name()) != "require_relative"
            {
                continue;
            }
            let arguments: Vec<_> =
                call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
            let target = (arguments.len() == 1)
                .then(|| super::util::string_value(&arguments[0]))
                .flatten()
                .and_then(|request| normalize_engine_library_require(dir, &config_dir, &request))
                .map(|mut target| {
                    if target.extension().is_none() {
                        target.set_extension("rb");
                    }
                    target
                });
            match target {
                Some(target) if boot_chain.contains(&target) => {}
                Some(target)
                    if target.extension().is_some_and(|e| e == "rb")
                        && vfs.exists(&target)
                        && !vfs.is_dir(&target)
                        && !path_has_symlink_component(vfs, dir, &target) =>
                {
                    if !out.contains(&target) {
                        out.push(target);
                    }
                }
                _ => {
                    if survey::is_active() {
                        let location = statement.location();
                        let text = String::from_utf8_lossy(location.as_slice());
                        survey::record(&super::IngestError::Unsupported {
                            file: file.display().to_string(),
                            message: format!(
                                "require_relative not followed (not a literal path to a Ruby file inside the app): `{text}`"
                            ),
                        });
                    }
                }
            }
        }
    }
    out
}

/// The gem-source files the boot loads (`App::boot_gem_entries`), in
/// load order: config/boot.rb, config/application.rb and
/// config/environment.rb, statement by statement at their top level. A
/// literal `require "x"` is the gem file `lib/x.rb` of a PATH or source
/// gem that has it; `Bundler.require` is each such gem the lockfile
/// names, in lock order, by what its Gemfile `require:` says or else by
/// its name (`act-kit`, then `act/kit`). A `require: false` gem loads
/// nothing.
fn boot_gem_entries<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    lock: Option<&crate::gems::Lockfile>,
    path_gems: &[PathBuf],
) -> Vec<String> {
    let libs: Vec<PathBuf> = path_gems.iter().map(|gem| gem.join("lib")).collect();
    let feature_file = |feature: &str| -> Option<PathBuf> {
        let file = if feature.ends_with(".rb") { feature.to_string() } else { format!("{feature}.rb") };
        libs.iter().map(|lib| lib.join(&file)).find(|path| vfs.exists(path))
    };
    let bundled = || -> Vec<PathBuf> {
        let Some(lock) = lock else { return Vec::new() };
        let mut out = Vec::new();
        // A dependency compiled from source has its directory among the
        // path gems: a PATH gem's, or a source gem's mount.
        for name in &lock.dependencies {
            let version = lock.version_of(name).unwrap_or_default();
            let Some(gem) = path_gems.iter().find(|gem| {
                gem.file_name().and_then(|n| n.to_str()).is_some_and(|base| {
                    base == name.as_str()
                        || base == format!("{name}-{version}")
                        || base.strip_prefix(name.as_str()).is_some_and(|rest| rest.starts_with('-'))
                })
            }) else {
                continue;
            };
            let lib = gem.join("lib");
            let features: Vec<String> = match lock.gemfile_requires.get(name) {
                Some(features) => features.clone(),
                None => [name.clone(), name.replace('-', "/")]
                    .into_iter()
                    .filter(|f| vfs.exists(&lib.join(format!("{f}.rb"))))
                    .take(1)
                    .collect(),
            };
            for feature in features {
                let file = lib.join(if feature.ends_with(".rb") { feature } else { format!("{feature}.rb") });
                if vfs.exists(&file) {
                    out.push(file);
                }
            }
        }
        out
    };
    let mut out: Vec<String> = Vec::new();
    for name in ["boot.rb", "application.rb", "environment.rb"] {
        let Ok(source) = vfs.read(&dir.join("config").join(name)) else { continue };
        let parsed = super::prism::parse_silent(&source);
        let Some(program) = parsed.node().as_program_node() else { continue };
        for statement in program.statements().body().iter() {
            let Some(call) = statement.as_call_node() else { continue };
            let method = super::util::constant_id_str(&call.name());
            let on_bundler = call
                .receiver()
                .and_then(|r| r.as_constant_read_node())
                .is_some_and(|r| super::util::constant_id_str(&r.name()) == "Bundler");
            let found: Vec<PathBuf> = if on_bundler && method == "require" {
                bundled()
            } else if call.receiver().is_none() && method == "require" {
                let args: Vec<_> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
                match args.as_slice() {
                    [one] => super::util::string_value(one).and_then(|f| feature_file(&f)).into_iter().collect(),
                    _ => Vec::new(),
                }
            } else {
                Vec::new()
            };
            for file in found {
                let file = file.display().to_string();
                if !out.contains(&file) {
                    out.push(file);
                }
            }
        }
    }
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
    /// The selected path-gem directories, sorted and deduplicated for
    /// [`PathGemVfs::within_dirs`].
    dirs: Vec<PathBuf>,
    /// `linked` per path: the tree does not change during an ingest, and a
    /// `require` is probed against every load path.
    linked_paths: std::cell::RefCell<crate::det::HashMap<PathBuf, bool>>,
}

impl<'a, V: Vfs + ?Sized> PathGemVfs<'a, V> {
    fn new(inner: &'a V, root: &'a Path, dirs: &[PathBuf]) -> Self {
        let mut dirs = dirs.to_vec();
        dirs.sort();
        dirs.dedup();
        Self { inner, root, dirs, linked_paths: Default::default() }
    }
}

impl<V: Vfs + ?Sized> PathGemVfs<'_, V> {
    /// Whether `path` is one of the selected path-gem directories or below one:
    /// some prefix of it is in `dirs`, which is sorted.
    fn within_dirs(&self, path: &Path) -> bool {
        let mut prefix = PathBuf::new();
        path.components().any(|component| {
            prefix.push(component);
            self.dirs.binary_search_by(|dir| dir.as_path().cmp(prefix.as_path())).is_ok()
        })
    }

    /// Apply the path-gem boundary to direct reads as well as directory walks.
    fn linked(&self, path: &Path) -> bool {
        if let Some(&known) = self.linked_paths.borrow().get(path) {
            return known;
        }
        let linked = self.within_dirs(path) && path_has_symlink_component(self.inner, self.root, path);
        self.linked_paths.borrow_mut().insert(path.to_path_buf(), linked);
        linked
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
        if self.within_dirs(path) {
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
pub(super) fn declares_rails_engine<V: Vfs + ?Sized>(vfs: &V, lib_dir: &Path) -> bool {
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
            let parsed = super::prism::parse_silent(&source);
            let mut visitor = EngineVisitor { found: false };
            ruby_prism::Visit::visit(&mut visitor, &parsed.node());
            visitor.found
        })
    })
}

/// Exact source-backed, isolated engines available to a literal `mount`.
/// Only the PATH roots already confined by [`path_gem_dirs`] participate;
/// each engine class must have one unambiguous `Rails::Engine` declaration,
/// one direct literal `isolate_namespace`, and an in-tree routes file.
fn engine_route_sources<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    path_gems: &[PathBuf],
) -> HashMap<String, EngineRouteSource> {
    struct Declaration {
        engine_dir: PathBuf,
        namespace: Option<String>,
        is_rails_engine: bool,
        plain_source_file: bool,
    }

    let mut declarations: HashMap<String, Vec<Declaration>> = HashMap::default();
    for engine_dir in path_gems {
        if !vfs.is_dir(&engine_dir.join("app")) {
            continue;
        }
        let lib_dir = engine_dir.join("lib");
        let Ok(mut files) = read_rb_files(vfs, &lib_dir) else { continue };
        let app_dir = engine_dir.join("app");
        if let Ok(app_files) = read_rb_files(vfs, &app_dir) {
            files.extend(app_files);
        }
        files.sort();
        files.dedup();
        for file in files {
            let Ok(source) = vfs.read(&file) else { continue };
            let parsed = super::prism::parse_silent(&source);
            if parsed.errors().next().is_some() {
                continue;
            }
            for (scope, class) in super::util::find_all_classes_with_scope(&parsed.node()) {
                let Some(mut class_path) = super::util::class_name_path(&class) else { continue };
                let mut qualified = scope;
                qualified.append(&mut class_path);
                let class_name = qualified.join("::");
                // Count all `...::Engine` class declarations, not just the
                // original Rails subclass. Ruby class reopenings commonly
                // omit the superclass; accepting only the first matching
                // subclass would miss later engine-name or initialization
                // changes in another in-tree source file.
                if !class_name.ends_with("::Engine") {
                    continue;
                }
                let is_rails_engine = class.superclass().is_some_and(|parent| {
                    super::util::constant_path_of(&parent)
                        .is_some_and(|path| path.len() == 2 && path[0] == "Rails" && path[1] == "Engine")
                });
                let namespace = is_rails_engine.then(|| literal_isolated_namespace(&class)).flatten();
                let plain_source_file = is_rails_engine
                    && file.starts_with(&lib_dir)
                    && plain_engine_declaration_file(&parsed.node(), &class_name);
                declarations.entry(class_name).or_default().push(Declaration {
                    engine_dir: engine_dir.clone(),
                    namespace,
                    is_rails_engine,
                    plain_source_file,
                });
            }
        }
    }

    let mut sources = HashMap::default();
    for (class_name, declarations) in declarations {
        if declarations.len() != 1 {
            continue;
        }
        let declaration = declarations.into_iter().next().expect("one declaration");
        if !declaration.is_rails_engine || !declaration.plain_source_file {
            continue;
        }
        let Some(namespace) = declaration.namespace else { continue };
        let engine_dir = declaration.engine_dir;
        // For this slice the engine's isolating module must be exactly the
        // namespace that owns its `Engine` class (`Catalog::Engine` →
        // `isolate_namespace Catalog`). Other arrangements need Rails'
        // own constant/proxy resolution rules and are not inferred here.
        if class_name.strip_suffix("::Engine") != Some(namespace.as_str()) {
            continue;
        }
        // The PATH gem's Ruby entrypoint runs before Rails mounts the
        // engine. Admit only in-tree literal requires and declarations in
        // lib/; executable file/class-body calls can register initializers,
        // middleware, or route mutations outside the Engine class file.
        if !plain_engine_library_sources(vfs, &engine_dir, &class_name) {
            continue;
        }
        // Railties run files in config/initializers even when the Engine
        // class itself is otherwise plain. This slice does not replay
        // those side effects (middleware, route prepends, auth setup, …),
        // so an engine carrying any such files remains an explicit mount
        // gap. Checking directory presence also avoids following a hidden
        // symlinked initializer through the original Rails runtime.
        let initializers = engine_dir.join("config/initializers");
        if vfs.is_dir(&initializers)
            || path_has_symlink_component(vfs, &engine_dir, &initializers)
        {
            continue;
        }
        let Ok(relative) = engine_dir.strip_prefix(dir) else { continue };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let routes_path = engine_dir.join("config/routes.rb");
        let Ok(source) = vfs.read(&routes_path) else { continue };
        let app_root = engine_dir.join("app");
        sources.insert(
            class_name,
            EngineRouteSource {
                source,
                file: routes_path.display().to_string(),
                namespace,
                app_root: app_root.display().to_string(),
            },
        );
    }
    sources
}

/// Sources where route helpers can be called. This is collected on the first
/// accepted source-backed engine mount only, and only from the app roots the
/// normal walker already treats as live code. Engine origin is retained for
/// the helper boundary check so a host `root_path` can remain valid while an
/// engine's same-named helper is rejected.
fn route_helper_sources<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    path_gems: &[PathBuf],
    engines: &HashMap<String, EngineRouteSource>,
    roots: &[PathBuf],
    additional_test_paths: &[PathBuf],
    lib_ignores: &[String],
) -> Vec<RouteHelperSource> {
    if engines.is_empty() {
        return Vec::new();
    }
    let mut sources = Vec::new();
    let mut rb_files = std::collections::BTreeSet::new();
    let mut mapped_view_files = std::collections::BTreeMap::new();
    let mut jbuilder_files = std::collections::BTreeSet::new();
    let mut roots_to_scan: Vec<PathBuf> = roots
        .iter()
        .flat_map(|root| ["models", "controllers", "helpers"].map(|layer| dir.join(root).join(layer)))
        .collect();
    roots_to_scan.extend(
        support_roots(vfs, dir, roots, path_gems, lib_ignores)
            .into_iter()
            .map(|root| dir.join(root)),
    );
    // These are the non-app source roots that the normal Rails walker
    // ingests as Ruby. Include the configured test roots too: a helper
    // proxy used there still must not bind to a host helper accidentally.
    roots_to_scan.extend([
        dir.join("config"),
    ]);
    roots_to_scan.sort();
    roots_to_scan.dedup();
    for root in roots_to_scan {
        if vfs.is_dir(&root) {
            if let Ok(files) = read_rb_files(vfs, &root) {
                rb_files.extend(files);
            }
        }
    }
    let mut test_roots: Vec<PathBuf> = [
        "test/models",
        "test/controllers",
        "test/helpers",
        "test/channels",
        "test/lib",
        "test/test_helpers",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    test_roots.extend(additional_test_paths.iter().cloned());
    test_roots.sort();
    test_roots.dedup();
    for test_root in test_roots {
        let test_dir = dir.join(&test_root);
        if !path_has_symlink_component(vfs, dir, &test_dir) && vfs.is_dir(&test_dir) {
            if let Ok(files) = read_test_rb_files(vfs, dir, &test_dir) {
                rb_files.extend(files);
            }
        }
    }
    let test_helper = dir.join("test/test_helper.rb");
    if !path_has_symlink_component(vfs, dir, &test_helper) && vfs.exists(&test_helper) {
        rb_files.insert(test_helper);
    }
    for app_root in roots {
        let views = dir.join(app_root).join("views");
        if vfs.is_dir(&views) {
            if let Ok(files) = read_erb_files(vfs, &views) {
                for (file, engine) in files {
                    // The name is historical; the walker returns every
                    // supported mapped text-template engine (ERB, HAML,
                    // Slim, Builder and Raw). The boundary scan needs the
                    // same compiled Ruby and source map the view ingester
                    // uses, or helper calls in a supported template could
                    // fall back to a host helper silently.
                    mapped_view_files.insert(file, engine);
                }
            }
            if let Ok(files) = read_jbuilder_files(vfs, &views) {
                jbuilder_files.extend(files);
            }
        }
    }

    // Engine route files are parsed separately for composition, so they do
    // not appear under the host `config/` source walk. Include them here as
    // engine-origin Ruby too: route options can evaluate helpers at draw
    // time, and those calls must not fall through to a host helper.
    for (engine_class, engine) in engines {
        sources.push(RouteHelperSource {
            source: engine.source.clone(),
            original: String::from_utf8_lossy(&engine.source).into_owned(),
            file: engine.file.clone(),
            engine_class: Some(engine_class.clone()),
            erb_map: None,
        });
    }

    let mut engine_roots: Vec<(usize, String, PathBuf)> = engines
        .iter()
        .filter_map(|(class, engine)| {
            let root = Path::new(&engine.app_root).parent()?.to_path_buf();
            Some((root.components().count(), class.clone(), root))
        })
        .collect();
    engine_roots.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.2.cmp(&right.2)));
    let engine_class = |path: &Path| {
        engine_roots
            .iter()
            .find(|(_, _, root)| path.starts_with(root))
            .map(|(_, class, _)| class.clone())
    };
    let ignored_lib_file = |entry: &Path| {
        entry.strip_prefix(dir.join("lib")).is_ok_and(|rel| {
            rel.components().next().is_some_and(|component| {
                lib_ignores.iter().any(|ignored| component.as_os_str() == ignored.as_str())
            })
        })
    };
    for file in rb_files {
        if ignored_lib_file(&file) {
            continue;
        }
        let Ok(source) = vfs.read(&file) else { continue };
        let original = String::from_utf8_lossy(&source).into_owned();
        sources.push(RouteHelperSource {
            source,
            original,
            file: file.display().to_string(),
            engine_class: engine_class(&file),
            erb_map: None,
        });
    }
    for (file, engine) in mapped_view_files {
        let Ok(original) = vfs.read_to_string(&file) else { continue };
        let (ruby, erb_map) = engine.compile_fn()(&original);
        sources.push(RouteHelperSource {
            source: ruby.into_bytes(),
            original,
            file: file.display().to_string(),
            engine_class: engine_class(&file),
            erb_map: Some(erb_map),
        });
    }
    for file in jbuilder_files {
        let Ok(source) = vfs.read(&file) else { continue };
        let original = String::from_utf8_lossy(&source).into_owned();
        sources.push(RouteHelperSource {
            source,
            original,
            file: file.display().to_string(),
            engine_class: engine_class(&file),
            erb_map: None,
        });
    }
    sources
}

/// A class-local direct `isolate_namespace Some::Literal` call. Calls through
/// receivers, dynamic arguments, and duplicate declarations are unavailable.
fn literal_isolated_namespace(class: &ruby_prism::ClassNode<'_>) -> Option<String> {
    let body = class.body()?;
    let statements = super::util::flatten_statements(body);
    if statements.len() != 1 {
        return None;
    }
    let call = statements[0].as_call_node()?;
    if super::util::constant_id_str(&call.name()) != "isolate_namespace"
        || call.receiver().is_some()
        || call.block().is_some()
    {
        return None;
    }
    let args = call.arguments()?;
    let values: Vec<_> = args.arguments().iter().collect();
    let namespace = (values.len() == 1)
        .then(|| super::util::constant_path_of(&values[0]))
        .flatten()
        .map(|parts| parts.join("::"))?;
    Some(namespace)
}

/// The engine class must be the only declaration in its source file, nested
/// through plain namespace modules. This prevents a top-level side effect or
/// a sibling declaration in `engine.rb` from changing the loaded engine
/// after the route-only composition check.
fn plain_engine_declaration_file(root: &Node<'_>, engine_name: &str) -> bool {
    fn one_statement<'pr>(node: &Node<'pr>) -> Option<Node<'pr>> {
        let statements = if let Some(program) = node.as_program_node() {
            program.statements()
        } else {
            node.as_statements_node()?
        };
        let mut body = statements.body().iter();
        let statement = body.next()?;
        body.next().is_none().then_some(statement)
    }

    fn follow_chain<'pr>(node: Node<'pr>, scope: &mut Vec<String>, engine_name: &str) -> bool {
        if let Some(module) = node.as_module_node() {
            let Some(name) = super::util::module_name_path(&module) else { return false };
            let old_len = scope.len();
            scope.extend(name);
            let valid = module.body().and_then(|body| one_statement(&body))
                .is_some_and(|statement| follow_chain(statement, scope, engine_name));
            scope.truncate(old_len);
            return valid;
        }
        let Some(class) = node.as_class_node() else { return false };
        let Some(mut path) = super::util::class_name_path(&class) else { return false };
        let mut qualified = scope.clone();
        qualified.append(&mut path);
        let Some(parent) = class.superclass() else { return false };
        qualified.join("::") == engine_name
            && super::util::constant_path_of(&parent)
                .is_some_and(|parts| parts.len() == 2 && parts[0] == "Rails" && parts[1] == "Engine")
            && literal_isolated_namespace(&class).is_some()
    }

    one_statement(root).is_some_and(|statement| follow_chain(statement, &mut Vec::new(), engine_name))
}

/// The engine library's source-level loading surface must be plain before a
/// route-only mount is admitted. Literal requires may target another Ruby
/// file under this same `lib/` tree (which is checked in this walk), plus the
/// Rails engine bootstrap that supplies `Rails::Engine`. At file level, only
/// those requires are allowed. Inside the engine's namespace, files may
/// define modules, classes without custom superclasses, instance methods,
/// ordinary `self` methods, and scalar constants. Ruby load-hook methods,
/// aliases, other calls, conditionals, and computed constants remain an
/// explicit mount gap.
fn plain_engine_library_sources<V: Vfs + ?Sized>(
    vfs: &V,
    engine_dir: &Path,
    engine_name: &str,
) -> bool {
    let lib_dir = engine_dir.join("lib");
    let Ok(files) = read_rb_files(vfs, &lib_dir) else { return false };
    let lib_files: HashSet<PathBuf> = files.iter().cloned().collect();
    files.iter().all(|file| {
        if path_has_symlink_component(vfs, &lib_dir, file) {
            return false;
        }
        let Ok(source) = vfs.read(file) else { return false };
        let parsed = super::prism::parse_silent(&source);
        if parsed.errors().next().is_some() {
            return false;
        }
        let Some(program) = parsed.node().as_program_node() else { return false };
        let mut scope = Vec::new();
        program.statements().body().iter().all(|statement| {
            plain_engine_library_statement(
                vfs,
                &lib_dir,
                file,
                &lib_files,
                engine_name,
                &mut scope,
                &statement,
                true,
            )
        })
    })
}

/// Admit one statement from the engine library's narrow load-time grammar.
/// Namespace modules may lead to the engine owner; executable definitions,
/// constants, and the Engine declaration itself are checked at their exact
/// namespace. Unknown AST shapes fail closed so boot effects are not skipped.
fn plain_engine_library_statement<V: Vfs + ?Sized>(
    vfs: &V,
    lib_dir: &Path,
    file: &Path,
    lib_files: &HashSet<PathBuf>,
    engine_name: &str,
    scope: &mut Vec<String>,
    statement: &Node<'_>,
    top_level: bool,
) -> bool {
    if let Some(module) = statement.as_module_node() {
        if module.constant_path().as_constant_path_node()
            .is_some_and(|path| super::util::constant_path_is_rooted(&path))
        {
            return false;
        }
        let Some(name) = super::util::module_name_path(&module) else { return false };
        let old_len = scope.len();
        scope.extend(name);
        // A nested engine can be declared under a chain of namespace
        // modules (for example `Catalog::Admin::Engine`). Admit only the
        // exact ancestor modules needed to reach its owner; executable
        // declarations inside those ancestors remain rejected below.
        let valid = engine_library_module_scope_is_admitted(scope, engine_name)
            && module.body().is_none_or(|body| {
                super::util::flatten_statements(body).iter().all(|inner| {
                    plain_engine_library_statement(
                        vfs, lib_dir, file, lib_files, engine_name, scope, inner, false,
                    )
                })
            });
        scope.truncate(old_len);
        return valid;
    }
    if let Some(class) = statement.as_class_node() {
        if class.constant_path().as_constant_path_node()
            .is_some_and(|path| super::util::constant_path_is_rooted(&path))
        {
            return false;
        }
        let Some(name) = super::util::class_name_path(&class) else { return false };
        let mut qualified = scope.clone();
        qualified.extend(name.iter().cloned());
        if qualified.join("::") == engine_name {
            let is_rails_engine = class.superclass().is_some_and(|parent| {
                super::util::constant_path_of(&parent)
                    .is_some_and(|parts| parts.len() == 2 && parts[0] == "Rails" && parts[1] == "Engine")
            });
            let expected_namespace = engine_name.strip_suffix("::Engine");
            return is_rails_engine
                && expected_namespace.is_some_and(|expected| {
                    literal_isolated_namespace(&class).as_deref() == Some(expected)
                });
        }
        if !engine_library_scope_belongs_to_owner(&qualified, engine_name) {
            return false;
        }
        // A superclass can run its `inherited` hook as this class is loaded.
        // The first slice keeps library declarations to classes without one.
        if class.superclass().is_some() {
            return false;
        }
        let old_len = scope.len();
        scope.extend(name);
        let valid = class.body().is_none_or(|body| {
            super::util::flatten_statements(body).iter().all(|inner| {
                plain_engine_library_statement(
                    vfs, lib_dir, file, lib_files, engine_name, scope, inner, false,
                )
            })
        });
        scope.truncate(old_len);
        return valid;
    }
    if let Some(def) = statement.as_def_node() {
        if top_level || !engine_library_scope_belongs_to_owner(scope, engine_name) {
            return false;
        }
        if def.receiver().is_none() {
            return true;
        }
        // Ordinary singleton helpers are safe declarations, but the Ruby
        // loading callbacks can run while later engine classes are defined.
        let method = super::util::constant_id_str(&def.name());
        return def.receiver().is_some_and(|receiver| receiver.as_self_node().is_some())
            && !engine_library_load_hook(method);
    }
    if let Some(write) = statement.as_constant_write_node() {
        if !engine_library_scope_belongs_to_owner(scope, engine_name) {
            return false;
        }
        let value = write.value();
        return value.as_string_node().is_some()
            || value.as_symbol_node().is_some()
            || value.as_integer_node().is_some()
            || value.as_float_node().is_some()
            || value.as_true_node().is_some()
            || value.as_false_node().is_some()
            || value.as_nil_node().is_some();
    }
    if top_level {
        if let Some(call) = statement.as_call_node() {
            return engine_library_require_is_admitted(vfs, lib_dir, file, lib_files, &call);
        }
    }
    false
}

/// Return whether methods, constants, or ordinary classes are scoped at the
/// engine's owning namespace or below it.
fn engine_library_scope_belongs_to_owner(scope: &[String], engine_name: &str) -> bool {
    let Some(owner) = engine_name.strip_suffix("::Engine") else { return false };
    let name = scope.join("::");
    name == owner || name.strip_prefix(owner).is_some_and(|suffix| suffix.starts_with("::"))
}

/// Namespace wrappers may also occupy a strict ancestor on the path to the
/// engine owner; their bodies are still checked statement by statement.
fn engine_library_module_scope_is_admitted(scope: &[String], engine_name: &str) -> bool {
    if engine_library_scope_belongs_to_owner(scope, engine_name) {
        return true;
    }
    let Some(owner) = engine_name.strip_suffix("::Engine") else { return false };
    let name = scope.join("::");
    !name.is_empty() && owner.strip_prefix(&name).is_some_and(|suffix| suffix.starts_with("::"))
}

/// Ruby callbacks that can run while classes or modules are loaded or changed.
/// Their definitions are excluded because the mount slice does not execute or
/// model callback effects.
fn engine_library_load_hook(method: &str) -> bool {
    matches!(
        method,
        "inherited"
            | "included"
            | "extended"
            | "prepended"
            | "append_features"
            | "prepend_features"
            | "extend_object"
            | "method_added"
            | "method_removed"
            | "method_undefined"
            | "singleton_method_added"
            | "singleton_method_removed"
            | "singleton_method_undefined"
            | "const_added"
            | "const_missing"
    )
}

/// Admit only literal requires whose resolved file is among the checked Ruby
/// files under this engine's `lib/`, plus the Rails engine bootstrap require.
fn engine_library_require_is_admitted<V: Vfs + ?Sized>(
    vfs: &V,
    lib_dir: &Path,
    file: &Path,
    lib_files: &HashSet<PathBuf>,
    call: &ruby_prism::CallNode<'_>,
) -> bool {
    if call.receiver().is_some() || call.block().is_some() {
        return false;
    }
    let method = super::util::constant_id_str(&call.name());
    let require_relative = match method {
        "require" => false,
        "require_relative" => true,
        _ => return false,
    };
    let Some(arguments) = call.arguments() else { return false };
    let arguments: Vec<_> = arguments.arguments().iter().collect();
    let Some(request) = (arguments.len() == 1)
        .then(|| super::util::string_value(&arguments[0]))
        .flatten()
    else {
        return false;
    };
    if !require_relative && matches!(request.as_str(), "rails/engine" | "rails/engine.rb") {
        return true;
    }
    let base = if require_relative {
        let Some(parent) = file.parent() else { return false };
        parent
    } else {
        lib_dir
    };
    let Some(mut target) = normalize_engine_library_require(lib_dir, base, &request) else {
        return false;
    };
    if target.extension().is_none() {
        target.set_extension("rb");
    }
    lib_files.contains(&target)
        && !path_has_symlink_component(vfs, lib_dir, &target)
        && vfs.exists(&target)
}

/// Normalize a literal require path while keeping every intermediate step
/// inside `lib/`, preventing an escape through an unchecked path or symlink
/// before a later `..` returns to an in-tree destination.
fn normalize_engine_library_require(
    lib_dir: &Path,
    base: &Path,
    request: &str,
) -> Option<PathBuf> {
    let request = Path::new(request);
    if request.is_absolute() {
        return None;
    }
    let mut normalized = base.to_path_buf();
    if !normalized.starts_with(lib_dir) {
        return None;
    }
    for component in request.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                // Keep every intermediate path inside the checked tree.
                // A lexical detour above lib/ could traverse an unchecked
                // symlink and then return to a path that starts with lib/.
                if normalized == lib_dir || !normalized.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
        if !normalized.starts_with(lib_dir) {
            return None;
        }
    }
    Some(normalized)
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

/// Roots an app adds to `config.autoload_paths`, `config.eager_load_paths`
/// or `config.autoload_once_paths` in `config/application.rb`, as paths
/// relative to the app root:
/// `config.eager_load_paths << Rails.root.join("extras")` → `extras`,
/// `config.autoload_paths += %w[app/lib]` → `app/lib`,
/// `config.autoload_once_paths << "#{root}/lib/shared"` → `lib/shared`.
///
/// Line-scanned rather than parsed, like the `autoload_lib` and
/// `time_zone` readers next to it: the file is railtie soup ingest does
/// not model, and the generator ships the `<<` form commented out, so
/// comment lines have to stay unmatched.
pub(super) fn extract_autoload_path_roots(source: &[u8]) -> Vec<String> {
    let source = String::from_utf8_lossy(source);
    let mut roots = Vec::new();
    for line in source.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        if !AUTOLOAD_PATH_SETTINGS.iter().any(|setting| t.contains(setting)) {
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
                if let Some(root) = app_relative_path(value) {
                    roots.push(root.to_string());
                }
            }
        }
    }
    roots
}

const AUTOLOAD_PATH_SETTINGS: &[&str] =
    &["config.autoload_paths", "config.eager_load_paths", "config.autoload_once_paths"];

fn app_relative_path(value: &str) -> Option<&str> {
    let path = match value.strip_prefix("#{") {
        Some(interpolated) => match interpolated.split_once('}')? {
            ("root" | "Rails.root" | "config.root", path) => path.strip_prefix('/')?,
            _ => return None,
        },
        None => value,
    };
    (!path.is_empty()).then_some(path)
}

/// `get "up" => "rails/health#show"` — every `rails new` app's health
/// check (what a Kamal proxy probes) — names Rails' OWN
/// `Rails::HealthController`, which no app tree holds: the route
/// dispatched to nothing and `/up` answered 404. Written here as the
/// controller Rails ships (8.1: a `respond_to` answering the green page
/// to HTML and `{"status":"up","timestamp":"<iso8601>"}` to JSON; the
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
    // The JSON arm is Rails' `render json: { status: "up", timestamp:
    // Time.current.iso8601 }` written out as the encoded text it
    // renders: an inline `render json:` arm of a `respond_to` reaches
    // the runtime encoder, which only the CRuby breadth keeps, and the
    // Spinel emit would drop the arm.
    let src = r##"class Rails::HealthController < ActionController::Base
  def show
    respond_to do |format|
      format.html { render html: "<!DOCTYPE html><html><body style=\"background-color: green\"></body></html>".html_safe }
      format.json { render plain: "{\"status\":\"up\",\"timestamp\":\"#{Time.current.iso8601}\"}", content_type: "application/json" }
    end
  end
end
"##;
    if let Ok(Some(controller)) = super::controller::ingest_controller(src.as_bytes(), "<rails/health>") {
        app.controllers.push(controller);
    }
}

/// The controller the `to: redirect(...)` routes dispatch to: one
/// action per redirect, each answering the location Rails' routing
/// redirect would. The options form (`redirect(path: "/x")`) also
/// carries the request's query over, read off the action's `request`;
/// the positional `redirect("/x")` does not, in Rails either. How the
/// query is joined to the path is the one deliberate divergence; see
/// the comment in the body.
fn synthesize_redirect_controller(
    redirects: &[crate::dialect::RedirectRoute],
) -> crate::dialect::Controller {
    use crate::dialect::{Action, ControllerBodyItem, RenderTarget};
    use crate::expr::{Expr, ExprNode, Literal};
    use crate::span::Span;

    let mut body: Vec<_> = redirects
        .iter()
        .map(|redirect| {
            // Built from Ruby source so the action body is ingested the
            // way a hand-written `redirect_to` would be.
            let location = if let Some(expression) = redirect.location.strip_prefix('\u{0}') {
                expression.to_string()
            } else if redirect.location_is_expression {
                redirect.location.clone()
            } else if redirect.keep_query {
                // The options form keeps the request query, read off the
                // action's own `query_string` (set by every target's
                // dispatcher from the raw request query). An empty query
                // leaves the location unchanged; a path that already has a
                // `?` is joined with `&`; the query goes ahead of a
                // fragment, so `/login#step` plus `x=1` is `/login?x=1#step`.
                //
                // A deliberate divergence from Rails, which appends
                // `"?" + query` and nothing else (MEASURED, 8.1.4 and
                // main): `/a?b=1` plus `x=1` is `/a?b=1?x=1`, where `b` reads
                // as "1?x=1", and `/a#top` plus `x=1` is `/a#top?x=1`, where
                // the query sits inside the fragment and never reaches the
                // server. Neither is a URL the route's author could have
                // meant. Rails also re-encodes the query from the parsed
                // params (keys sorted, a space as `+`); this passes it
                // through as received.
                //
                // Only `path:` keeps the query, and its location is a
                // string literal, so the separator and the fragment are
                // decided here. A `%{name}` cannot bring a `?` or `#` of
                // its own: `redirect_location_source` path-escapes it.
                // Uses the controller attribute rather than
                // `request.query_string` so C# and Elixir (which do not
                // yet wire a full Request receiver) still preserve the
                // query.
                let (path, fragment) = redirect.location.split_once('#').unwrap_or((&redirect.location, ""));
                let separator = if path.contains('?') { '&' } else { '?' };
                let fragment = if fragment.is_empty() {
                    String::new()
                } else {
                    format!(" + {}", redirect_location_source(&format!("#{fragment}")))
                };
                format!(
                    "q = query_string\n    q == \"\" ? {} : {} + q{fragment}",
                    redirect_location_source(&redirect.location),
                    redirect_location_source(&format!("{path}{separator}")),
                )
            } else {
                redirect_location_source(&redirect.location)
            };
            let src = if location.contains('\n') || location.contains(';') {
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
                    rest_param: None,
                    anonymous_formal: None,
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

    // Routing redirects do not use ActionController's open-redirect policy.
    // Keep the exception on this controller instead of weakening redirect_to.
    let policy = super::controller::ingest_controller(
        b"class RoundhouseRedirectsController < ActionController::Base\nprivate\ndef same_host_location(location)\n  location\nend\ndef absolute_redirect_location(location)\n  location\nend\nend\n",
        "<routing-redirect>",
    )
    .expect("routing redirect policy parses")
    .expect("routing redirect policy is a controller");
    body.extend(policy.body);

    crate::dialect::Controller {
        name: crate::ident::ClassId(Symbol::from(super::routes::REDIRECT_CONTROLLER)),
        // `ActionController::Base`, not the app's `ApplicationController`:
        // a routing redirect never enters the app's controller stack, so
        // the synthesized action must not pick up its filters either. An
        // app that authenticates in `ApplicationController` would
        // otherwise start challenging a redirect Rails answers
        // unconditionally.
        parent: Some(crate::ident::ClassId(Symbol::from("ActionController::Base"))),
        parent_span: Default::default(),
        body,
        layout: crate::dialect::LayoutDecl::default(),
        sibling_classes: Vec::new(),
    }
}

/// A routing redirect's target as a Ruby string literal. Rails'
/// `redirect("/~%{username}")` fills each `%{name}` from the matched
/// path parameters, path-escaped as Rails does
/// (`Journey::Router::Utils.escape_path`), so the placeholder becomes
/// `#{ActionDispatch::Router.escape_path(params[:name].to_s)}`. The
/// router decodes a capture (`Router.decode_capture`), so a `#` or `?`
/// in it is escaped back and stays in the path.
fn redirect_location_source(location: &str) -> String {
    let mut out = String::from("\"");
    let mut rest = location;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("%{") {
            if let Some(close) = after.find('}') {
                let name = &after[..close];
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    out.push_str(&format!("#{{ActionDispatch::Router.escape_path(params[:{name}].to_s)}}"));
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
pub(super) fn extract_autoload_lib_ignores(source: &[u8]) -> Vec<String> {
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
pub(super) const FRAMEWORK_CONFIG_KEYS: &[&str] = &[
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
pub(super) fn extract_time_formats(source: &[u8], file: &str) -> Vec<(String, TimeFormatSource)> {
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
pub(super) fn extract_module_mixins(source: &[u8], file: &str) -> Vec<crate::app::ModuleMixin> {
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
fn content_helper_attribute_additions<V: Vfs + ?Sized>(vfs: &V, dir: &Path, app: &App) -> Vec<String> {
    let mut out = content_helper_allow_list_additions(
        vfs,
        dir,
        app,
        "allowed_attributes=",
        &["allowed_attributes", "sanitizer_allowed_attributes"],
        "allowed_attributes",
    );
    if app.gem_lock.as_ref().is_some_and(|l| l.has("lexxy")) {
        for n in ["controls", "poster", "data-language", "style", "value", "start"] {
            if !out.iter().any(|x| x == n) {
                out.push(n.to_string());
            }
        }
    }
    out
}

/// Same shape as attributes: campfire unions
/// `ContentFilters::EDITOR_FORMATTING_TAGS` into `allowed_tags` at boot.
/// Sanitize-on-render consults this list, so the additions must land in
/// the emitted runtime (unlike the prior storage-only path).
fn content_helper_tag_additions<V: Vfs + ?Sized>(vfs: &V, dir: &Path, app: &App) -> Vec<String> {
    content_helper_allow_list_additions(
        vfs,
        dir,
        app,
        "allowed_tags=",
        &["allowed_tags", "sanitizer_allowed_tags"],
        "allowed_tags",
    )
}

fn content_helper_allow_list_additions<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    app: &App,
    assignment: &str,
    base_methods: &[&str],
    field: &str,
) -> Vec<String> {
    use crate::expr::{ExprNode, Literal};

    let mut out: Vec<String> = Vec::new();
    let add = |names: Vec<String>, out: &mut Vec<String>| {
        for n in names {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    };

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
    fn is_base(e: &crate::expr::Expr, base_methods: &[&str]) -> bool {
        match &*e.node {
            ExprNode::Send { method, .. } => base_methods.iter().any(|m| method.as_str() == *m),
            ExprNode::BoolOp { left, right, .. } => {
                is_base(left, base_methods) && is_base(right, base_methods)
            }
            ExprNode::Seq { exprs } if exprs.len() == 1 => is_base(&exprs[0], base_methods),
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
            if super::util::constant_id_str(&call.name()) != assignment {
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
                if is_base(op, base_methods) {
                    continue;
                }
                match resolve(op) {
                    Some(list) => add(list, &mut out),
                    None => survey::record(&IngestError::Unsupported {
                        file: file.clone(),
                        message: format!(
                            "ActionText::ContentHelper.{field} addition is not a literal list or a constant holding one; the {field} it adds are not allowed by the emitted sanitizer"
                        ),
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
pub(super) fn initializer_statements<'a>(program: &ruby_prism::ProgramNode<'a>) -> Vec<Node<'a>> {
    let mut stmts: Vec<Node> = Vec::new();
    for stmt in program.statements().body().iter() {
        match initializer_statements_of(&stmt) {
            Some(body) => stmts.extend(body),
            None => stmts.push(stmt),
        }
    }
    stmts
}

/// The statements of a top-level `to_prepare` / `to_run` block.
pub(super) fn initializer_statements_of<'a>(stmt: &Node<'a>) -> Option<Vec<Node<'a>>> {
    let call = stmt.as_call_node()?;
    let name = super::util::constant_id_str(&call.name());
    if !matches!(name, "to_prepare" | "to_run") {
        return None;
    }
    let body = call.block()?.as_block_node()?.body()?.as_statements_node()?;
    Some(body.body().iter().collect())
}

/// `def name` at an initializer's top level (lobsters'
/// `do_heinous_inline_partial_replacement`).
pub(super) fn extract_initializer_defs(source: &[u8], file: &str) -> Vec<crate::ident::Symbol> {
    let result = super::prism::parse(source, file);
    let root = result.node();
    let Some(program) = root.as_program_node() else { return Vec::new() };
    initializer_statements(&program)
        .iter()
        .filter_map(|stmt| stmt.as_def_node())
        .filter(|def| def.receiver().is_none())
        .map(|def| crate::ident::Symbol::from(super::util::constant_id_str(&def.name())))
        .collect()
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
pub(super) fn extract_initializer_filters(source: &[u8], file: &str) -> Vec<crate::app::InitializerFilter> {
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
        let only = match options {
            Some(options) => match filter_only(options) {
                Some(only) => only,
                None => continue,
            },
            None => Vec::new(),
        };
        out.push(InitializerFilter { target: Symbol::from(target), method: Symbol::from(method), only });
    }
    out
}

/// The `only:` of a filter's options, `None` when the options say
/// anything else (`except:`, `if:`, a computed list): a filter recorded
/// without them would run where the app said it must not.
fn filter_only(options: &Node<'_>) -> Option<Vec<Symbol>> {
    let hash = options.as_keyword_hash_node()?;
    let mut only = Vec::new();
    for element in hash.elements().iter() {
        let pair = element.as_assoc_node()?;
        if super::util::symbol_value(&pair.key()).as_deref() != Some("only") {
            return None;
        }
        let value = pair.value();
        if let Some(one) = super::util::symbol_value(&value) {
            only.push(Symbol::from(one));
        } else {
            for e in value.as_array_node()?.elements().iter() {
                only.push(Symbol::from(super::util::symbol_value(&e)?));
            }
        }
    }
    (!only.is_empty()).then_some(only)
}

/// `X.after_action[(only: …)] { response.set_header("K", "v") }` in a
/// `config/initializers/` file, bare, inside `to_prepare`, or inside
/// `ActiveSupport.on_load(:hook) { … }` (which runs once the hook's class
/// loads, before any request). campfire's `active_storage.rb` marks the
/// disk service's file responses cacheable this way.
///
/// Only a block whose every statement writes one literal header is read
/// (`response.set_header`, `response.headers[…] =`, `headers[…] =`):
/// the runtime writes those itself, so nothing in the block goes
/// unexecuted. An `on_load` block is read only when every statement in
/// it is such a filter.
pub(super) fn extract_initializer_response_headers(
    source: &[u8],
    file: &str,
) -> Vec<crate::app::InitializerResponseHeader> {
    let result = super::prism::parse(source, file);
    let root = result.node();
    let src = String::from_utf8_lossy(source).into_owned();
    let Some(program) = root.as_program_node() else { return Vec::new() };
    let mut out = Vec::new();
    for stmt in initializer_statements(&program) {
        if let Some(body) = on_load_body(&stmt) {
            let mut inner = Vec::new();
            if body.iter().all(|s| response_header_filter(s, &src, &mut inner)) {
                out.extend(inner);
            }
        } else {
            response_header_filter(&stmt, &src, &mut out);
        }
    }
    out
}

/// The statements of `ActiveSupport.on_load(:sym) do … end`.
fn on_load_body<'a>(node: &Node<'a>) -> Option<Vec<Node<'a>>> {
    let call = node.as_call_node()?;
    if super::util::constant_id_str(&call.name()) != "on_load" {
        return None;
    }
    let recv = call.receiver()?;
    if super::util::constant_path_of(&recv)? != ["ActiveSupport"] {
        return None;
    }
    let args: Vec<_> = call.arguments()?.arguments().iter().collect();
    if args.len() != 1 || super::util::symbol_value(&args[0]).is_none() {
        return None;
    }
    let body = call.block()?.as_block_node()?.body()?.as_statements_node()?;
    Some(body.body().iter().collect())
}

/// Append the headers of one `X.after_action { … }` to `out`; false (and
/// nothing appended) when `node` is not one this reader takes whole.
fn response_header_filter(
    node: &Node<'_>,
    src: &str,
    out: &mut Vec<crate::app::InitializerResponseHeader>,
) -> bool {
    let Some(call) = node.as_call_node() else { return false };
    if super::util::constant_id_str(&call.name()) != "after_action" {
        return false;
    }
    let Some(target) = call.receiver().and_then(|r| constant_text(&r, src)) else { return false };
    let args: Vec<_> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
    let only = match args.as_slice() {
        [] => Vec::new(),
        [options] => match filter_only(options) {
            Some(only) => only,
            None => return false,
        },
        _ => return false,
    };
    let Some(block) = call.block().and_then(|b| b.as_block_node()) else { return false };
    if block.parameters().is_some() {
        return false;
    }
    let Some(body) = block.body().and_then(|b| b.as_statements_node()) else { return false };
    let mut headers = Vec::new();
    for stmt in body.body().iter() {
        match literal_header_write(&stmt) {
            Some(pair) => headers.push(pair),
            None => return false,
        }
    }
    if headers.is_empty() {
        return false;
    }
    for (name, value) in headers {
        out.push(crate::app::InitializerResponseHeader {
            target: Symbol::from(target.as_str()),
            only: only.clone(),
            name,
            value,
        });
    }
    true
}

/// `response.set_header("K", "v")`, `response.headers["K"] = "v"` or
/// `headers["K"] = "v"`, with both Strings literal.
fn literal_header_write(node: &Node<'_>) -> Option<(String, String)> {
    let call = node.as_call_node()?;
    let args: Vec<_> = call.arguments()?.arguments().iter().collect();
    let [name, value] = args.as_slice() else { return None };
    let is_response = |n: &Node<'_>| {
        n.as_call_node().is_some_and(|c| {
            c.receiver().is_none()
                && c.arguments().is_none()
                && c.block().is_none()
                && super::util::constant_id_str(&c.name()) == "response"
        })
    };
    let is_headers = |n: &Node<'_>| {
        n.as_call_node().is_some_and(|c| {
            c.arguments().is_none()
                && c.block().is_none()
                && super::util::constant_id_str(&c.name()) == "headers"
                && c.receiver().is_none_or(|r| is_response(&r))
        })
    };
    let recv = call.receiver()?;
    let shape = match super::util::constant_id_str(&call.name()) {
        "set_header" => is_response(&recv),
        "[]=" => is_headers(&recv),
        _ => false,
    };
    if !shape {
        return None;
    }
    Some((super::util::string_value(name)?, super::util::string_value(value)?))
}

/// `ActionView::PartialRenderer.collection_cache = <Const>.<reader>` in a
/// `config/initializers/` file, bare or inside `to_prepare` (campfire's
/// `fragment_cache.rb` points it at `FragmentCache.store`). The same
/// discipline as the mixin reader: a constant receiver and an
/// argument-less, blockless method on it, readable off the parse.
/// Anything else (a computed store, `ActiveSupport::Cache.lookup_store`
/// with arguments) is not recorded, and the initializer ledger reports
/// the statement. The last assignment wins, as at boot.
pub(super) fn extract_collection_cache_store(source: &[u8], file: &str) -> Option<crate::app::CollectionCacheStore> {
    let result = super::prism::parse(source, file);
    let root = result.node();
    let src = String::from_utf8_lossy(source).into_owned();
    let program = root.as_program_node()?;

    let mut out = None;
    for stmt in initializer_statements(&program) {
        let Some(call) = stmt.as_call_node() else { continue };
        if super::util::constant_id_str(&call.name()) != "collection_cache=" {
            continue;
        }
        let Some(recv) = call.receiver() else { continue };
        if constant_text(&recv, &src).as_deref() != Some("ActionView::PartialRenderer") {
            continue;
        }
        let Some(args) = call.arguments() else { continue };
        let args: Vec<_> = args.arguments().iter().collect();
        let [value] = args.as_slice() else { continue };
        let Some(read) = value.as_call_node() else { continue };
        if read.arguments().is_some() || read.block().is_some() {
            continue;
        }
        let Some(store_recv) = read.receiver() else { continue };
        let Some(receiver) = constant_text(&store_recv, &src) else { continue };
        out = Some(crate::app::CollectionCacheStore {
            receiver: Symbol::from(receiver),
            reader: Symbol::from(super::util::constant_id_str(&read.name())),
        });
    }
    out
}

/// The source text of a node that is a constant or a constant path
/// (`Foo`, `Turbo::StreamsChannel`), or None for anything else. Read as
/// TEXT rather than resolved: the receiver may name a class this tree
/// does not define, and reporting that gap needs the app's spelling.
pub(super) fn constant_text(node: &Node, src: &str) -> Option<String> {
    if node.as_constant_read_node().is_none() && node.as_constant_path_node().is_none() {
        return None;
    }
    let loc = node.location();
    Some(src[loc.start_offset()..loc.end_offset()].to_string())
}

/// A registered format as its initializer spells it, before the lambda
/// form is parsed into IR.
pub(super) enum TimeFormatSource {
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
/// A framework setting the app itself reads back (lobsters'
/// `Routes.default_url_options` is `Rails.application.config
/// .action_mailer.default_url_options`) becomes a `Rails::Application`
/// reader answering what the running environment's file assigned, over
/// application.rb and the initializers. Only a READ setting is lifted:
/// every other framework assignment configures machinery the emit replaces.
fn lift_read_framework_settings<V: Vfs + ?Sized>(vfs: &V, dir: &Path, app: &mut App) {
    let reads = framework_config_reads(app);
    if reads.is_empty() {
        return;
    }
    let mut sources: Vec<(String, Vec<u8>)> = Vec::new();
    for path in [dir.join("config/application.rb")] {
        if let Ok(bytes) = vfs.read(&path) {
            sources.push((path.display().to_string(), bytes));
        }
    }
    let init_dir = dir.join("config/initializers");
    if vfs.is_dir(&init_dir) {
        for entry in read_rb_files(vfs, &init_dir).unwrap_or_default() {
            if let Ok(bytes) = vfs.read(&entry) {
                sources.push((entry.display().to_string(), bytes));
            }
        }
    }
    let mut assignments: Vec<(Vec<String>, String)> = Vec::new();
    for (framework, leaf) in reads {

        let wanted = [framework.clone(), leaf.clone()];
        let assigned = |bytes: &[u8], path: &str| {
            extract_framework_config_assignments(bytes, path)
                .into_iter()
                .filter(|(segments, _)| segments.as_slice() == wanted.as_slice())
                .last()
                .map(|(_, value)| value)
        };
        let base = sources.iter().filter_map(|(path, bytes)| assigned(bytes, path)).last();
        let mut arms = String::new();
        for env in ["development", "production", "test"] {
            let file = dir.join(format!("config/environments/{env}.rb"));
            let Ok(bytes) = vfs.read(&file) else { continue };
            // Not the assignment alone: ActionMailer's railtie adds `protocol: "https"` (`||=`) where `force_ssl` is on, whoever assigned the options.
            let force_ssl = framework == "action_mailer"
                && leaf == "default_url_options"
                && extract_framework_config_assignments(&bytes, &file.display().to_string())
                    .into_iter()
                    .any(|(segments, value)| segments == ["force_ssl"] && value.trim() == "true");
            let value = assigned(&bytes, &file.display().to_string()).or_else(|| force_ssl.then(|| base.clone()).flatten());
            if let Some(value) = value {
                let value = if force_ssl { format!("{{ protocol: \"https\" }}.merge({value})") } else { value };
                arms.push_str(&format!("when {env:?}\n  {value}\n"));
            }
        }
        if arms.is_empty() && base.is_none() {
            continue;
        }
        let fallback = base.unwrap_or_else(|| "nil".to_string());
        let value = if arms.is_empty() {
            fallback
        } else {
            format!("case Rails.env.to_s\n{arms}else\n  {fallback}\nend")
        };
        assignments.push((wanted.to_vec(), value));
    }
    if assignments.is_empty() {
        return;
    }
    let rails_application = app.rails_application.get_or_insert_with(|| crate::dialect::LibraryClass {
        name: crate::ident::ClassId(crate::ident::Symbol::from("Rails::Application")),
        is_module: false,
        parent: None,
        parent_span: Default::default(),
        includes: Vec::new(),
        include_spans: Vec::new(),
        methods: Vec::new(),
        nullable_columns: Vec::new(),
        origin: None,
        constants: Vec::new(),
        conditional_definitions: Vec::new(),
        unknown_calls: Vec::new(),
        class_ivar_initializers: Vec::new(),
    });
    let mut constants = String::new();
    if !rails_application.constants.iter().any(|(name, _)| name.as_str() == "CONFIG_LOCK") {
        constants.push_str("CONFIG_LOCK = Mutex.new\n");
    }
    for (segments, value) in &assignments {
        let name = segments.join("_");
        if rails_application.methods.iter().any(|m| m.name.as_str() == name) {
            continue;
        }
        let slot = format!("{}_SLOT", name.to_uppercase());
        if let Ok(mut synth) = crate::runtime_src::parse_methods(&format!(
            "def {name}__build\n  {value}\nend\n\
             def {name}\n  slot = {slot}\n  CONFIG_LOCK.synchronize do\n    slot.push({name}__build) if slot.empty?\n  end\n  slot[0]\nend\n"
        )) {
            rails_application.methods.append(&mut synth);
            constants.push_str(&format!("{slot} = []\n"));
        }
    }
    if let Ok(mut synth) = crate::runtime_src::parse_module_constant_exprs(&constants) {
        rails_application.constants.append(&mut synth);
    }
}

pub(super) fn extract_config_assignments(source: &[u8], file: &str) -> Vec<(Vec<String>, String)> {
    config_assignments(source, file, false)
}

/// [`extract_config_assignments`] with the framework subsections kept
/// (`config.action_mailer.default_url_options = …`).
fn extract_framework_config_assignments(source: &[u8], file: &str) -> Vec<(Vec<String>, String)> {
    config_assignments(source, file, true)
}

/// Every `Rails.application.config.<framework>.<setting>` an app body reads.
fn framework_config_reads(app: &App) -> std::collections::BTreeSet<(String, String)> {
    use crate::expr::ExprNode;
    fn is_config_root(e: &crate::expr::Expr) -> bool {
        let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node else { return false };
        if !args.is_empty() {
            return false;
        }
        let rails = |e: &crate::expr::Expr| matches!(&*e.node, ExprNode::Const { path } if path.last().is_some_and(|p| p.as_str() == "Rails"));
        match method.as_str() {
            "config" => matches!(&*r.node, ExprNode::Send { recv: Some(rr), method, .. } if method.as_str() == "application" && rails(rr)),
            "configuration" => rails(r),
            _ => false,
        }
    }
    fn visit(e: &crate::expr::Expr, out: &mut std::collections::BTreeSet<(String, String)>) {
        if let ExprNode::Send { recv: Some(r), method: leaf, args, .. } = &*e.node
            && args.is_empty()
            && let ExprNode::Send { recv: Some(root), method: framework, args: fargs, .. } = &*r.node
            && fargs.is_empty()
            && FRAMEWORK_CONFIG_KEYS.contains(&framework.as_str())
            && is_config_root(root)
        {
            out.insert((framework.as_str().to_string(), leaf.as_str().to_string()));
        }
        e.node.for_each_child(&mut |c| visit(c, out));
    }
    let mut out = std::collections::BTreeSet::new();
    crate::lower::for_each_emit_body_ref(app, &mut |e| visit(e, &mut out));
    out
}

fn config_assignments(source: &[u8], file: &str, framework: bool) -> Vec<(Vec<String>, String)> {
    fn walk(stmts: Vec<ruby_prism::Node<'_>>, src: &str, framework: bool, out: &mut Vec<(Vec<String>, String)>) {
        for stmt in stmts {
            // Config lines sit at the top level of an initializer and
            // inside the Application class body; descend through both
            // rather than the whole expression tree, which is where
            // every other config reader in this file draws the line.
            if let Some(class) = stmt.as_class_node() {
                walk(class.body().map(super::util::flatten_statements).unwrap_or_default(), src, framework, out);
                continue;
            }
            if let Some(module) = stmt.as_module_node() {
                walk(module.body().map(super::util::flatten_statements).unwrap_or_default(), src, framework, out);
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
                        walk(super::util::flatten_statements(body), src, framework, out);
                        continue;
                    }
                }
            }
            let Some(call) = stmt.as_call_node() else { continue };
            // `x.y = v` parses as a call named `y=`.
            let name = super::util::constant_id_str(&call.name()).to_string();
            let Some(base) = name.strip_suffix('=') else { continue };
            let prefix = if framework {
                call.receiver().as_ref().and_then(|r| config_path_of(r, 0))
            } else {
                config_receiver_path(&call.receiver())
            };
            let Some(prefix) = prefix else {
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
    walk(stmts, &src, framework, &mut out);
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

/// A Symbol source as Rails' `ContentSecurityPolicy::MAPPINGS` writes it
/// (actionpack 8.1.4).
pub(crate) fn csp_source_mapping(symbol: &str) -> Option<&'static str> {
    Some(match symbol {
        "self" => "'self'",
        "unsafe_eval" => "'unsafe-eval'",
        "wasm_unsafe_eval" => "'wasm-unsafe-eval'",
        "unsafe_hashes" => "'unsafe-hashes'",
        "unsafe_inline" => "'unsafe-inline'",
        "none" => "'none'",
        "http" => "http:",
        "https" => "https:",
        "data" => "data:",
        "mediastream" => "mediastream:",
        "allow_duplicates" => "'allow-duplicates'",
        "blob" => "blob:",
        "filesystem" => "filesystem:",
        "report_sample" => "'report-sample'",
        "script" => "'script'",
        "strict_dynamic" => "'strict-dynamic'",
        "ws" => "ws:",
        "wss" => "wss:",
        _ => return None,
    })
}

/// `config.content_security_policy do |policy| policy.script_src :self end`,
/// including the `unless ENV["DISABLE_CONTENT_SECURITY_POLICY"]` guard
/// when the file has one. `None` when the block names no literal source.
pub(super) fn content_security_policy_method(source: &str) -> Option<String> {
    let start = source.find("content_security_policy")?;
    let mut directives: Vec<(String, Vec<String>)> = Vec::new();
    for line in source[start..].lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with("end") {
            break;
        }
        let Some(rest) = line.strip_prefix("policy.") else { continue };
        let name_end = rest.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(rest.len());
        let name = &rest[..name_end];
        if name.is_empty() {
            continue;
        }
        let args = rest[name_end..].trim().trim_end_matches(')');
        let mut sources = Vec::new();
        for arg in args.split(',') {
            let arg = arg.trim().trim_end_matches(',');
            if arg.is_empty() {
                continue;
            }
            let quoted = if let Some(sym) = arg.strip_prefix(':') {
                match csp_source_mapping(sym) {
                    Some(source) => source.to_string(),
                    // An unknown symbol is not a source we can quote.
                    // Dropping it keeps the directive honest; inventing
                    // the bare word would ship a policy the app did not write.
                    None => continue,
                }
            } else if arg.len() >= 2
                && ((arg.starts_with('"') && arg.ends_with('"'))
                    || (arg.starts_with('\'') && arg.ends_with('\'')))
            {
                arg[1..arg.len() - 1].to_string()
            } else {
                continue;
            };
            sources.push(quoted);
        }
        if !sources.is_empty() {
            directives.push((name.replace('_', "-"), sources));
        }
    }
    if directives.is_empty() {
        return None;
    }
    let guarded = source.contains("DISABLE_CONTENT_SECURITY_POLICY");
    let mut body = String::from("def content_security_policy\n");
    if guarded {
        body.push_str("  return nil if ENV[\"DISABLE_CONTENT_SECURITY_POLICY\"] == \"true\"\n");
    }
    body.push_str("  policy = ActionDispatch::ContentSecurityPolicy.new\n");
    for (name, sources) in directives {
        let list = sources.iter().map(|s| format!("{s:?}")).collect::<Vec<_>>().join(", ");
        body.push_str(&format!("  policy.set_directive({name:?}, [{list}])\n"));
    }
    body.push_str("  policy\nend\n");
    Some(body)
}

/// Pretty `{"imports":{…}}` for `Rails.application.importmap_json`.
///
/// The bytes are the script body `javascript_importmap_tags` embeds
/// (`action_view/view_helpers.rb`: 2-space indent, one pin per line,
/// `": "` between name and path). Campfire hashes that body into
/// `script-src`. A compact encoding is a different hash, and the
/// browser then blocks the page's own import map. Pin paths are
/// already `/assets/…` from importmap ingest, in the same order
/// `Importmap.pins` emits.
pub(super) fn importmap_json_method(pins: &[crate::app::ImportmapPin]) -> Option<String> {
    if pins.is_empty() {
        return None;
    }
    let mut json = String::from("{\n  \"imports\": {\n");
    for (i, pin) in pins.iter().enumerate() {
        if i > 0 {
            json.push_str(",\n");
        }
        json.push_str("    \"");
        json.push_str(&json_escape(&pin.name));
        json.push_str("\": \"");
        json.push_str(&json_escape(&pin.path));
        json.push('"');
    }
    json.push_str("\n  }\n}");
    Some(format!("def importmap_json\n  {json:?}\nend\n"))
}

fn json_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// Result of scanning an initializer for `video_preview_arguments`.
pub(super) enum VideoPreviewArgsExtract {
    Absent,
    Value(String),
    /// Assignment present but not a pure string-literal concatenation.
    Unsupported,
}

/// `config.active_storage.video_preview_arguments = "…" \ "…"` —
/// concatenated quoted string literals after the `=` (double or single),
/// the way campfire tip writes the `-vf … -frames:v 1 -f image2` argv.
/// Returns the joined runtime string. Computed RHS forms (`+ ENV…`) are
/// `Unsupported` rather than a wrong joined literal.
pub(super) fn extract_video_preview_arguments(source: &[u8]) -> VideoPreviewArgsExtract {
    let source = String::from_utf8_lossy(source);
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        let Some(idx) = t.find("active_storage.video_preview_arguments") else {
            continue;
        };
        let rest = t[idx + "active_storage.video_preview_arguments".len()..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let mut text = rest.to_string();
        // Line continuations (`\`), RHS starting on the next line, and
        // further quoted pieces until a blank / next config assignment.
        while text.trim_end().ends_with('\\')
            || ruby_string_literals_unclosed(&text)
            || (!text.contains('"') && !text.contains('\'') && lines.peek().is_some())
        {
            let Some(next) = lines.next() else { break };
            let n = next.trim();
            if n.is_empty() || n.starts_with("config.") {
                break;
            }
            text.push(' ');
            text.push_str(n);
        }
        if !rhs_is_only_string_literals_and_continuations(&text) {
            return VideoPreviewArgsExtract::Unsupported;
        }
        let out = join_ruby_string_literals(&text);
        if !out.is_empty() {
            return VideoPreviewArgsExtract::Value(out);
        }
        // Recognized the assignment but found no quoted pieces.
        return VideoPreviewArgsExtract::Unsupported;
    }
    VideoPreviewArgsExtract::Absent
}

/// True when `text` is only `'…'` / `"…"` literals, whitespace, and `\`.
fn rhs_is_only_string_literals_and_continuations(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut saw_literal = false;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() || b == b'\\' {
            i += 1;
            continue;
        }
        if b != b'"' && b != b'\'' {
            return false;
        }
        saw_literal = true;
        let q = b;
        i += 1;
        while i < bytes.len() {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if bytes[i] == q {
                i += 1;
                break;
            }
            i += 1;
        }
    }
    saw_literal
}

/// True when `text` ends inside an unclosed `'…'` or `"…"` literal.
fn ruby_string_literals_unclosed(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut open: Option<u8> = None;
    while i < bytes.len() {
        if let Some(q) = open {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if bytes[i] == q {
                open = None;
            }
            i += 1;
            continue;
        }
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            open = Some(bytes[i]);
        }
        i += 1;
    }
    open.is_some()
}

/// Join adjacent `'…'` / `"…"` literals in an RHS.
/// Double-quoted: `\\X` → `X`. Single-quoted: only `\\` → `\` and
/// `\'` → `'`; other backslashes (e.g. `\,` in ffmpeg filters) stay.
fn join_ruby_string_literals(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let q = bytes[i];
        if q != b'"' && q != b'\'' {
            i += 1;
            continue;
        }
        i += 1;
        while i < bytes.len() {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                let next = bytes[i + 1];
                if q == b'\'' {
                    if next == b'\\' || next == b'\'' {
                        out.push(next as char);
                        i += 2;
                        continue;
                    }
                    out.push('\\');
                    out.push(next as char);
                    i += 2;
                    continue;
                }
                out.push(next as char);
                i += 2;
                continue;
            }
            if bytes[i] == q {
                i += 1;
                break;
            }
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Replacement Const from
/// `config.active_storage.previewers = ….map` that swaps
/// `ActiveStorage::Previewer::VideoPreviewer` for another class
/// (`previewer == …VideoPreviewer ? Replacement : previewer`).
/// Returns the replacement's written name (e.g. `TimeLimitedVideoPreviewer`).
/// Full-line comments are stripped so a commented-out swap cannot match;
/// the assignment + `.map` form is required (not a bare class mention).
pub(super) fn extract_video_previewer_replacement(source: &[u8]) -> Option<String> {
    let source = String::from_utf8_lossy(source);
    let active: String = source
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let Some(assign_at) = active.find("active_storage.previewers") else {
        return None;
    };
    let after_name = active[assign_at + "active_storage.previewers".len()..].trim_start();
    let Some(after_eq) = after_name.strip_prefix('=') else {
        return None;
    };
    if !after_eq.contains(".map") {
        return None;
    }
    let marker = "ActiveStorage::Previewer::VideoPreviewer";
    let Some(idx) = after_eq.find(marker) else {
        return None;
    };
    let after = after_eq[idx + marker.len()..].trim_start();
    let after = after.strip_prefix('?')?.trim_start();
    // `? TimeLimitedVideoPreviewer : previewer` or multiline.
    let name: String = after
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == ':' || *c == '_')
        .collect();
    if name.is_empty() || name == "previewer" {
        return None;
    }
    Some(name)
}

/// The MIME types a `config.active_storage.variable_content_types -=
/// %w[…]` line subtracts, in source order. Line-scanned like the
/// other config reads; the `%w[` literal may start on the next line
/// (campfire breaks after the `-=`), so the scan continues to the
/// closing bracket. Only the subtractive form: an app REPLACING the
/// list (`= %w[…]`) is not read, and keeps Rails' default.
pub(super) fn extract_variable_content_type_exclusions(source: &[u8]) -> Vec<String> {
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

/// One `config.filter_parameters` statement, as
/// [`extract_filter_parameters`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FilterParametersLine {
    /// `+= [...]` (`replace: false`) or `= [...]` (`replace: true`) of
    /// Symbol, String and Regexp literals, each as the Ruby source of the
    /// filter: a Symbol or String as its String literal (`"passw"`), a
    /// Regexp as written (`/\bapi_key/i`).
    Literal { replace: bool, filters: Vec<String> },
    /// Any other operand: an interpolated Regexp, a Proc, a computed
    /// list. `replace` is true for a plain `=`, which still discards the
    /// filters before it.
    Computed { replace: bool },
}

/// The `config.filter_parameters` statements of a file, in source order
/// (`Rails.application.config.filter_parameters += [:passw, :email]` in
/// `filter_parameter_logging.rb`, or `config.filter_parameters += [...]`
/// in the Application body). Line-scanned like the other config reads;
/// the operand may span lines, so it grows a line at a time until Prism
/// parses it whole (comments and trailing commas inside the list are
/// Ruby's to skip). `%i[...]` and `%w[...]` lists count as literals.
pub(crate) fn extract_filter_parameters(source: &[u8]) -> Vec<FilterParametersLine> {
    let source = String::from_utf8_lossy(source);
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        i += 1;
        if t.starts_with('#') {
            continue;
        }
        let Some(idx) = t.find("config.filter_parameters") else { continue };
        let rest = t[idx + "config.filter_parameters".len()..].trim_start();
        let (replace, rest) = if let Some(r) = rest.strip_prefix("+=") {
            (false, r)
        } else if let Some(r) = rest.strip_prefix('=').filter(|r| !r.starts_with('=')) {
            (true, r)
        } else if filter_parameters_mutation(rest) {
            out.push(FilterParametersLine::Computed { replace: false });
            continue;
        } else {
            // Not a write: an initializer that only reads the list (lobsters' lograge `ParameterFilter.new(…filter_parameters)`)
            continue;
        };
        // The shortest run of lines that parses as one operand. Blank
        // and comment-only lines before it (`+=` ending its line) parse
        // cleanly but hold no operand, so the run grows past them. A
        // statement that never parses stays Computed, and the scan goes
        // on from the next line.
        let mut text = rest.to_string();
        let mut filters = None;
        for end in i..=lines.len() {
            let found = {
                let parsed = ruby_prism::parse(text.as_bytes());
                let has_operand = parsed
                    .node()
                    .as_program_node()
                    .is_some_and(|p| p.statements().body().iter().next().is_some());
                (has_operand && parsed.errors().next().is_none())
                    .then(|| filter_parameter_list(&parsed, &text))
            };
            if found.is_some() {
                filters = found;
                i = end;
                break;
            }
            if end < lines.len() {
                text.push('\n');
                text.push_str(lines[end]);
            }
        }
        out.push(match filters.flatten() {
            Some(filters) => FilterParametersLine::Literal { replace, filters },
            None => FilterParametersLine::Computed { replace },
        });
    }
    out
}

/// Whether what follows `config.filter_parameters` changes the list in
/// place (`<<`, `.push(…)`, `.concat(…)`, a bang method) rather than reads it.
fn filter_parameters_mutation(rest: &str) -> bool {
    if rest.starts_with("<<") {
        return true;
    }
    let Some(call) = rest.strip_prefix('.') else { return false };
    let name: String = call.chars().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '!' | '?')).collect();
    name.ends_with('!')
        || matches!(name.as_str(), "push" | "append" | "concat" | "unshift" | "prepend" | "insert" | "replace" | "clear" | "delete" | "delete_at" | "delete_if" | "keep_if" | "pop" | "shift" | "fill")
}

/// The filters of an operand that is one `[...]`, `%i[...]` or `%w[...]`
/// literal of Symbols, Strings and plain Regexps, as Ruby source. A
/// Symbol or String is kept only when it is a plain name (letters,
/// digits, `_`, `.`), since it is written back as a String literal.
fn filter_parameter_list(parsed: &ruby_prism::ParseResult<'_>, text: &str) -> Option<Vec<String>> {
    let program = parsed.node();
    let statements: Vec<_> = program.as_program_node()?.statements().body().iter().collect();
    let [operand] = statements.as_slice() else { return None };
    let array = operand.as_array_node()?;
    array
        .elements()
        .iter()
        .map(|element| {
            if element.as_regular_expression_node().is_some() {
                let loc = element.location();
                return Some(text[loc.start_offset()..loc.end_offset()].to_string());
            }
            let name = super::util::symbol_or_string_value(&element)?;
            let plain = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
            plain.then(|| format!("{name:?}"))
        })
        .collect()
}

/// `(Vips.block_untrusted(true) seen, operations named by Vips.block(
/// "<name>", true))` from an initializer. Line-shaped like the
/// content-type trim above: the two calls are one statement each, and
/// only the `true` arms are policy (`false` is libvips' default).
/// Parens are optional (`Vips.block_untrusted true`); a `#` comment
/// does not count. Named blocks accept double or single quotes.
pub(super) fn extract_vips_loader_policy(source: &[u8]) -> (bool, Vec<String>) {
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
/// one winning as it does when Ruby runs the block. `Kaminari.configure`
/// is an input spelling of the default page size, not the feature name.
/// Read off the parse rather than the lines, so an assignment in another
/// config block of the same file (`Rails.application.configure do
/// |config|`) is not mistaken for this default.
///
/// A value that is not a positive Integer literal (a constant, an ENV
/// read) is recognized but not evaluated: it is a survey gap, and this
/// file contributes no page size rather than a guessed one.
fn extract_default_per_page(source: &[u8], file: &str) -> Option<u64> {
    kaminari_default_per_page(source, file).1
}

/// `(recognized, value)`: `recognized` is true when the source holds a
/// `Kaminari.configure do |c| … c.default_per_page = … end` this reader
/// reads (the initializer ledger's definition of "consumed"), whether or
/// not its value is a positive Integer literal.
pub(super) fn kaminari_default_per_page(source: &[u8], file: &str) -> (bool, Option<u64>) {
    let src = String::from_utf8_lossy(source);
    // Cheap skip before parsing: this runs over every initializer, and
    // most never name Kaminari.
    if !src.contains("Kaminari") {
        return (false, None);
    }
    let result = super::prism::parse(source, file);
    let root = result.node();
    let Some(program) = root.as_program_node() else { return (false, None) };
    let mut found = None;
    let mut recognized = false;
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
            recognized = true;
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
                        "default_per_page is not a positive Integer literal (`{}`); the emitted app does not apply it",
                        &src[loc.start_offset()..loc.end_offset()]
                    ),
                });
            }
            found = literal;
        }
    }
    (recognized, found)
}

/// `forgery_protection_verification_strategy`, app-wide, in Rails'
/// precedence: `config.load_defaults` 8.2+ sets `:header_only`; an
/// explicit `config.action_controller.forgery_protection_verification_
/// strategy = :x` wins over that; and `protect_from_forgery using: :x`
/// in a controller or concern sets it for the controllers that run it.
///
/// Rails keeps the last as a per-controller class attribute. This
/// runtime has one forgery check, so the lift is app-wide: it is
/// exact for the shape apps write (ApplicationController, or a concern
/// it includes — campfire's `Authentication`), and two controllers
/// asking for different strategies leave the default in place rather
/// than pick one. Framework controllers configured from an initializer
/// (`ActiveStorage::BaseController.forgery_…= :x`) are not the app's
/// controllers and are not read.
///
/// `None` = Rails' class default (`header_or_legacy_token`).
/// Does a module under `app/helpers` define `token_tag` as exactly `""`?
fn app_helpers_blank_token_tag<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> bool {
    fn blank_token_tag(node: &ruby_prism::Node<'_>) -> bool {
        if let Some(def) = node.as_def_node() {
            if def.name().as_slice() == b"token_tag" {
                let Some(body) = def.body() else { return false };
                let stmts: Vec<_> = match body.as_statements_node() {
                    Some(s) => s.body().iter().collect(),
                    None => vec![body],
                };
                return stmts.len() == 1
                    && stmts[0].as_string_node().is_some_and(|s| s.unescaped().is_empty());
            }
            return false;
        }
        let mut found = false;
        if let Some(m) = node.as_module_node() {
            if let Some(b) = m.body() {
                found = blank_token_tag(&b);
            }
        } else if let Some(c) = node.as_class_node() {
            if let Some(b) = c.body() {
                found = blank_token_tag(&b);
            }
        } else if let Some(s) = node.as_statements_node() {
            found = s.body().iter().any(|n| blank_token_tag(&n));
        } else if let Some(p) = node.as_program_node() {
            found = blank_token_tag(&p.statements().as_node());
        }
        found
    }
    let helpers = dir.join("app/helpers");
    if !vfs.is_dir(&helpers) {
        return false;
    }
    let Ok(files) = read_rb_files(vfs, &helpers) else { return false };
    files.iter().any(|file| {
        let Ok(source) = vfs.read(file) else { return false };
        let result = super::prism::parse(&source, &file.display().to_string());
        blank_token_tag(&result.node())
    })
}

/// `:header_only` / `:header_or_legacy_token` at the start of `text`.
fn forgery_strategy_value(text: &str) -> Option<String> {
    let v = text.trim().trim_start_matches(':');
    let name: String =
        v.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    matches!(name.as_str(), "header_only" | "header_or_legacy_token").then_some(name)
}

/// The strategy an explicit `config.action_controller.forgery_protection_
/// verification_strategy = :x` line (already trimmed, not a comment) sets.
pub(super) fn forgery_strategy_line(line: &str) -> Option<String> {
    let rest =
        line.strip_prefix("config.action_controller.forgery_protection_verification_strategy")?;
    rest.trim().strip_prefix('=').and_then(forgery_strategy_value)
}

/// The version a `config.load_defaults 8.1` / `config.load_defaults(8.1)`
/// line (already trimmed, not a comment) names.
pub(super) fn load_defaults_version(line: &str) -> Option<f64> {
    let rest = line.strip_prefix("config.load_defaults")?;
    let version: String = rest
        .trim()
        .trim_start_matches('(')
        .trim_matches(|c| c == '"' || c == '\'')
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    version.parse::<f64>().ok()
}

fn read_forgery_verification_strategy<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> Option<String> {
    let strategy_value = forgery_strategy_value;
    let code_lines = |bytes: &[u8]| -> Vec<String> {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.starts_with('#'))
            .collect()
    };
    let mut strategy: Option<String> = None;
    let mut config_files: Vec<Vec<String>> = Vec::new();
    if let Ok(source) = vfs.read(&dir.join("config/application.rb")) {
        let lines = code_lines(&source);
        for line in &lines {
            if let Some(v) = load_defaults_version(line) {
                strategy = (v >= 8.2).then(|| "header_only".to_string());
            }
        }
        config_files.push(lines);
    }
    for sub in ["config/environments/production.rb", "config/initializers"] {
        let path = dir.join(sub);
        let paths = if vfs.is_dir(&path) {
            read_rb_files(vfs, &path).unwrap_or_default()
        } else {
            vec![path]
        };
        for p in paths {
            if let Ok(source) = vfs.read(&p) {
                config_files.push(code_lines(&source));
            }
        }
    }
    for line in config_files.iter().flatten() {
        if let Some(v) = forgery_strategy_line(line) {
            strategy = Some(v);
        }
    }
    let controllers = dir.join("app/controllers");
    if vfs.is_dir(&controllers) {
        let mut asked: Option<String> = None;
        for p in read_rb_files(vfs, &controllers).unwrap_or_default() {
            let Ok(source) = vfs.read(&p) else { continue };
            for line in code_lines(&source) {
                if !line.starts_with("protect_from_forgery") {
                    continue;
                }
                let Some(at) = line.find("using:") else { continue };
                let Some(v) = strategy_value(&line[at + "using:".len()..]) else { continue };
                match &asked {
                    Some(prev) if *prev != v => return strategy.filter(|s| s != "header_or_legacy_token"),
                    _ => asked = Some(v),
                }
            }
        }
        if asked.is_some() {
            strategy = asked;
        }
    }
    strategy.filter(|s| s != "header_or_legacy_token")
}

/// Whether Rails wraps a JSON body for every controller by default
/// (`App::wrap_parameters_by_default`). Railtie soup, read by line scan
/// as `extract_config_time_zone` reads it, in Rails' own precedence:
/// `config.load_defaults` 7.0+ switches the key on; an explicit
/// `config.action_controller.wrap_parameters_by_default = …` (application
/// or an initializer) wins over that; and the pre-7.0 generator's
/// initializer, `wrap_parameters format: [:json]` inside
/// `on_load(:action_controller)`, wraps for every controller too.
fn read_wrap_parameters_by_default<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> bool {
    let code_lines = |bytes: &[u8]| -> Vec<String> {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.starts_with('#'))
            .collect()
    };
    let mut files: Vec<Vec<String>> = Vec::new();
    let mut default = false;
    if let Ok(source) = vfs.read(&dir.join("config/application.rb")) {
        let lines = code_lines(&source);
        for line in &lines {
            // `config.load_defaults 8.1` and `config.load_defaults(8.1)`.
            if let Some(v) = load_defaults_version(line) {
                default = v >= 7.0;
            }
        }
        files.push(lines);
    }
    let init_dir = dir.join("config/initializers");
    if vfs.is_dir(&init_dir) {
        if let Ok(paths) = read_rb_files(vfs, &init_dir) {
            for path in paths {
                if let Ok(source) = vfs.read(&path) {
                    files.push(code_lines(&source));
                }
            }
        }
    }
    for line in files.iter().flatten() {
        if let Some(v) = wrap_parameters_line(line) {
            default = v;
        }
    }
    default
}

/// The adapter a trimmed, non-comment config line sets
/// `config.active_job.queue_adapter` to, as a name: `= :sidekiq` and
/// `= "sidekiq"` both give `"sidekiq"`. `Some(None)` when the line sets
/// it to something that isn't a literal (`ENV.fetch(...)`), `None` when
/// the line doesn't set it. One definition for the lift and the
/// initializer ledger.
pub(super) fn queue_adapter_line(line: &str) -> Option<Option<String>> {
    let rest = line
        .strip_prefix("Rails.application.config.active_job.queue_adapter")
        .or_else(|| line.strip_prefix("config.active_job.queue_adapter"))?;
    // An assignment, not the `==` or `=~` operators or a rightward
    // assignment (`=> name`), which reads the setting.
    let value = rest.trim_start().strip_prefix('=')?;
    if value.starts_with(['=', '~', '>']) {
        return None;
    }
    let value = value.trim();
    // Drop a trailing comment, with or without a space before its `#`,
    // but not a `#` inside a quoted value.
    let value = match value.chars().next() {
        Some(q @ ('"' | '\'')) => match value[1..].find(q) {
            Some(at) => {
                let (quoted, after) = value.split_at(at + 2);
                let after = after.trim_start();
                if !after.is_empty() && !after.starts_with('#') {
                    return Some(None);
                }
                quoted
            }
            None => value,
        },
        _ => match value.find('#') {
            Some(at) => value[..at].trim(),
            None => value,
        },
    };
    let name = if let Some(sym) = value.strip_prefix(':') {
        sym
    } else if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        return Some(None);
    };
    let literal = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    Some(literal.then(|| name.to_string()))
}

/// `App::queue_adapter`: `config/application.rb`, then
/// `config/environments/production.rb`, then `config/initializers`, by
/// line scan as `read_forgery_verification_strategy` reads them. The
/// last setting wins, and a last setting that isn't a literal leaves the
/// adapter unknown (`None`) rather than falling back to an earlier one.
fn read_queue_adapter<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> Option<String> {
    let mut adapter: Option<String> = None;
    for sub in ["config/application.rb", "config/environments/production.rb", "config/initializers"] {
        let path = dir.join(sub);
        let paths = if vfs.is_dir(&path) {
            read_rb_files(vfs, &path).unwrap_or_default()
        } else {
            vec![path]
        };
        for p in paths {
            let Ok(source) = vfs.read(&p) else { continue };
            for line in String::from_utf8_lossy(&source).lines() {
                let line = line.trim();
                if line.starts_with('#') {
                    continue;
                }
                if let Some(set) = queue_adapter_line(line) {
                    adapter = set;
                }
            }
        }
    }
    adapter
}

/// `self.queue_adapter = :name` in `ApplicationJob`'s class body. Rails
/// keeps the adapter per job class, and ApplicationJob's covers every
/// job that inherits it, so it overrides the config files' setting, the
/// last assignment winning, and a value that isn't a literal leaves the
/// adapter unknown, as in `read_queue_adapter`. Another job class's own
/// setting is not honored yet: its jobs follow the app's adapter, with a
/// warning.
fn apply_application_job_queue_adapter(app: &mut App) {
    let mut set: Option<Option<String>> = None;
    for lc in &app.library_classes {
        let name = lc.name.0.as_str();
        for call in &lc.unknown_calls {
            let Some(value) = job_queue_adapter(call) else { continue };
            if name == "ApplicationJob" {
                set = Some(value);
            } else {
                warn_sidekiq(format!(
                    "`self.queue_adapter =` in {name} is not honored yet (only ApplicationJob's is read); its jobs use the app's adapter"
                ));
            }
        }
    }
    if let Some(value) = set {
        app.queue_adapter = value;
    }
}

/// The adapter a job class-body `self.queue_adapter = …` names: the
/// literal's name, `Some(None)` for a value that isn't a Symbol or
/// String literal, `None` for any other call.
fn job_queue_adapter(call: &crate::expr::Expr) -> Option<Option<String>> {
    use crate::expr::{ExprNode, Literal};
    let ExprNode::Send { method, args, .. } = &*call.node else { return None };
    if method.as_str() != "queue_adapter=" || args.len() != 1 {
        return None;
    }
    Some(match &*args[0].node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        _ => None,
    })
}

/// `App::sidekiq_redis`: the `Sidekiq.configure_client` Redis settings
/// in `config/initializers`, the last block that sets them winning, as it
/// does when Rails runs the initializers in order.
fn read_sidekiq_redis<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> crate::app::SidekiqRedisConfig {
    let mut config = crate::app::SidekiqRedisConfig::default();
    let init = dir.join("config/initializers");
    if !vfs.is_dir(&init) {
        return config;
    }
    for path in read_rb_files(vfs, &init).unwrap_or_default() {
        let Ok(source) = vfs.read(&path) else { continue };
        if let Some(found) = sidekiq_client_redis(&source, &path.display().to_string()) {
            config = found;
        }
    }
    config
}

/// The `url:` and `password:` a file's `Sidekiq.configure_client do
/// |config| config.redis = { … } end` gives, `None` when the file has no
/// such assignment. Only those two keys are read, each as a String
/// literal, `ENV["NAME"]` or `ENV.fetch("NAME"[, "default"])`. Any other
/// key, a value in another form, or a `config.redis =` that isn't a Hash
/// literal is a warning: the push then takes that setting from the
/// environment fallbacks, or not at all.
pub(super) fn sidekiq_client_redis(source: &[u8], file: &str) -> Option<crate::app::SidekiqRedisConfig> {
    let src = String::from_utf8_lossy(source);
    // Cheap skip before parsing, as `kaminari_default_per_page` does.
    if !src.contains("configure_client") {
        return None;
    }
    let result = super::prism::parse(source, file);
    let root = result.node();
    let program = root.as_program_node()?;
    let text = |node: &Node<'_>| {
        let loc = node.location();
        src[loc.start_offset()..loc.end_offset()].to_string()
    };
    let mut found = None;
    for stmt in initializer_statements(&program) {
        let Some(call) = stmt.as_call_node() else { continue };
        if super::util::constant_id_str(&call.name()) != "configure_client" {
            continue;
        }
        let Some(recv) = call.receiver() else { continue };
        if !matches!(constant_text(&recv, &src).as_deref(), Some("Sidekiq" | "::Sidekiq")) {
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
            if super::util::constant_id_str(&assign.name()) != "redis=" {
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
            let mut config = crate::app::SidekiqRedisConfig::default();
            let Some(hash) = value.as_hash_node() else {
                warn_sidekiq(format!(
                    "`{param}.redis = {}` in {file} is not a Hash literal, so it is not read; jobs are pushed to REDIS_PROVIDER, REDIS_URL or localhost",
                    text(value)
                ));
                found = Some(config);
                continue;
            };
            for element in hash.elements().iter() {
                let Some(pair) = element.as_assoc_node() else {
                    warn_sidekiq(format!("`{}` in {file}'s Sidekiq Redis settings is not read", text(&element)));
                    continue;
                };
                let Some(key) = super::util::symbol_value(&pair.key()) else {
                    warn_sidekiq(format!("`{}` in {file}'s Sidekiq Redis settings is not read", text(&element)));
                    continue;
                };
                if key != "url" && key != "password" {
                    warn_sidekiq(format!(
                        "Sidekiq Redis setting `{key}:` in {file} is not supported yet (only url: and password: are read); it is ignored"
                    ));
                    continue;
                }
                let value = pair.value();
                let Some(setting) = redis_setting(&value, &src) else {
                    warn_sidekiq(format!(
                        "Sidekiq Redis `{key}: {}` in {file} is not a String literal, ENV[\"NAME\"] or ENV.fetch(\"NAME\"); it is ignored",
                        text(&value)
                    ));
                    continue;
                };
                if key == "url" {
                    config.url = Some(setting);
                } else {
                    config.password = Some(setting);
                }
            }
            found = Some(config);
        }
    }
    found
}

/// A Redis setting's value as `RedisSetting` reads it.
fn redis_setting(node: &Node<'_>, src: &str) -> Option<crate::app::RedisSetting> {
    use crate::app::RedisSetting;
    if let Some(literal) = string_literal_value(node) {
        return Some(RedisSetting::Literal(literal));
    }
    let call = node.as_call_node()?;
    let recv = call.receiver()?;
    if !matches!(constant_text(&recv, src).as_deref(), Some("ENV" | "::ENV")) {
        return None;
    }
    if call.block().is_some() {
        return None;
    }
    let args: Vec<_> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
    let strings: Option<Vec<String>> = args.iter().map(string_literal_value).collect();
    let strings = strings?;
    match (super::util::constant_id_str(&call.name()), strings.as_slice()) {
        ("[]", [name]) => Some(RedisSetting::Env(name.clone())),
        ("fetch", [name]) => Some(RedisSetting::EnvFetch { name: name.clone(), default: None }),
        ("fetch", [name, default]) => {
            Some(RedisSetting::EnvFetch { name: name.clone(), default: Some(default.clone()) })
        }
        _ => None,
    }
}

/// A warning about what ingest reads from the app's Sidekiq settings.
fn warn_sidekiq(detail: String) {
    let mut d = crate::diagnostic::Diagnostic::unsupported(crate::span::Span::synthetic(), None, "Sidekiq", detail);
    d.severity = crate::diagnostic::Severity::Warning;
    super::prism::push(d);
}

/// What a trimmed, non-comment config line sets the app-wide
/// `wrap_parameters_by_default` to, or `None` when the line says nothing
/// about it. One definition for the lift and the initializer ledger.
pub(super) fn wrap_parameters_line(line: &str) -> Option<bool> {
    if let Some(i) = line.find("action_controller.wrap_parameters_by_default") {
        let rest = line[i..].split_once('=').map(|(_, v)| v.trim()).unwrap_or("");
        if rest.starts_with("true") {
            return Some(true);
        }
        if rest.starts_with("false") {
            return Some(false);
        }
        return None;
    }
    let rest = line.strip_prefix("wrap_parameters format:")?;
    // Receiverless only — `WidgetsController.wrap_parameters …` is
    // that controller's call, not the app-wide on_load default.
    // Format alone (the pre-7 generator); `include:` / `exclude:` /
    // `name:` on the same line are not modeled as the app default.
    if rest.contains("include:") || rest.contains("exclude:") || rest.contains("name:") {
        return None;
    }
    // Exact `:json` / `json` entry — not a substring of `:json_api`.
    Some(wrap_parameters_format_includes_json(rest))
}

/// Whether a `wrap_parameters format: …` argument list names `:json`
/// exactly (Rails' ParamsWrapper check), not a longer synonym such as
/// `:json_api`.
fn wrap_parameters_format_includes_json(rest: &str) -> bool {
    rest.split(|c: char| matches!(c, ',' | '[' | ']' | '(' | ')' | ' ' | '\t'))
        .map(|tok| tok.trim().trim_matches(|c| c == '"' || c == '\''))
        .any(|tok| tok == ":json" || tok == "json")
}

pub(super) fn extract_config_time_zone(source: &[u8]) -> Option<String> {
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
/// The modules `test/test_helper.rb` mixes into every test case: what
/// its `ActiveSupport::TestCase` reopen includes, and what a
/// `test/test_helpers/` file includes through an `on_load` hook.
fn test_case_wide_includes<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Vec<Symbol>> {
    let helpers_dir = dir.join("test/test_helpers");
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
    for entry in if vfs.is_dir(&helpers_dir) { read_rb_files(vfs, &helpers_dir)? } else { Vec::new() } {
        let Some(source) = read_or_ledger(vfs, &entry)? else { continue };
        for name in on_load_test_includes(&source, &entry.display().to_string()) {
            if !wanted.contains(&name) {
                wanted.push(name);
            }
        }
    }
    Ok(wanted)
}

fn ingest_test_helper_modules<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
) -> IngestResult<Vec<LibraryClass>> {
    let helpers_dir = dir.join("test/test_helpers");
    if !vfs.is_dir(&helpers_dir) {
        return Ok(Vec::new());
    }

    let wanted = test_case_wide_includes(vfs, dir)?;
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

/// Each `test/test_helpers/` file that is not spliced into every test
/// case, as (its top-level module, every class the file defines). The
/// file is read whole because a helper module can carry what a method
/// splice cannot: campfire's `PushServiceTestHelper` defines a nested
/// `Server` class and module methods with their own state.
fn ingest_included_test_helper_files<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    shared: &[LibraryClass],
    test_files: &[PathBuf],
) -> IngestResult<Vec<(crate::ident::ClassId, Vec<LibraryClass>)>> {
    let helpers_dir = dir.join("test/test_helpers");
    let mut candidates: Vec<(PathBuf, bool)> = Vec::new();
    if vfs.is_dir(&helpers_dir) {
        candidates.extend(read_rb_files(vfs, &helpers_dir)?.into_iter().map(|f| (f, false)));
    }
    candidates.extend(test_files.iter().map(|f| (f.clone(), true)));
    let mut out = Vec::new();
    for (entry, in_test_folder) in candidates {
        let Some(source) = read_or_ledger(vfs, &entry)? else { continue };
        // A test folder's file is a helper only when it holds no test
        // class; the test-file pass below reads the others.
        if in_test_folder
            && !ingest_test_files(&source, &entry.display().to_string()).is_ok_and(|tms| tms.is_empty())
        {
            continue;
        }
        let Some(classes) =
            unwrap_or_record(ingest_library_classes(&source, &entry.display().to_string()))?
        else {
            continue;
        };
        let Some(top) = classes.iter().find(|c| c.is_module && !c.name.0.as_str().contains("::")) else {
            continue;
        };
        if shared.iter().any(|lc| lc.name == top.name) {
            continue;
        }
        let top = top.name.clone();
        let mut classes = classes;
        for lc in &mut classes {
            for m in &mut lc.methods {
                restore_source_keywords(&mut m.params);
            }
        }
        out.push((top, classes));
    }
    Ok(out)
}

/// Undo the library-class flattening of keyword parameters: this code
/// runs only as the test-side Ruby it was written as, called with the
/// keywords its own source passes (`Server.new(**options)`,
/// `server.hung_up?(within: 1)`), so the parameters stay keywords.
fn restore_source_keywords(params: &mut [crate::dialect::Param]) {
    for p in params {
        if p.from_keyword {
            p.from_keyword = false;
            p.keyword = true;
        } else if p.from_kwrest {
            p.from_kwrest = false;
            p.keyword = true;
            p.rest = true;
            p.default = None;
        }
    }
}

/// A test class that `include`s one of those modules gets the module's
/// file as inner classes, so the include resolves in the test's own
/// emitted file. A module whose methods assert has the instance methods
/// the test reaches spliced in as its helpers too, as the shared helpers
/// are: a test class's methods are where assertions are lowered
/// (`assert_equal`, `flunk` have no harness method), and the class's own
/// copy wins over the carried module's, as a class body beats an
/// included module. A module that asserts nothing stays carried alone:
/// the splice would buy nothing, and the test-method lowering drops a
/// forwarded `**options` (campfire's `with_push_service`).
fn carry_included_test_helpers(tm: &mut TestModule, helpers: &[(crate::ident::ClassId, Vec<LibraryClass>)]) {
    for (module, classes) in helpers {
        if !tm.includes.contains(module) {
            continue;
        }
        for lc in classes {
            if !tm.inner_classes.iter().any(|c| c.name == lc.name) {
                tm.inner_classes.push(lc.clone());
            }
        }
        if let Some(top) = classes.iter().find(|c| &c.name == module) {
            // Minitest runs every `test_*` method a test class responds
            // to, the included module's too (shared tests: `include
            // SharedTests` with `def test_creates`). Carried but not
            // listed, it would never run, and the file would pass
            // without the test.
            for m in &top.methods {
                let Some(stem) = m.name.as_str().strip_prefix("test_") else { continue };
                if m.receiver != MethodReceiver::Instance || !m.params.is_empty() {
                    continue;
                }
                let name = stem.replace('_', " ");
                if !tm.tests.iter().any(|t| t.name == name) {
                    tm.tests.push(crate::dialect::Test { name, body: m.body.clone() });
                }
            }
            let mut called = crate::det::HashSet::default();
            let mut consts = crate::det::HashSet::default();
            for m in &top.methods {
                collect_referenced_names(&m.body, &mut called, &mut consts);
            }
            let asserts = called.iter().any(|name| {
                let name = name.as_str();
                name.starts_with("assert") || name.starts_with("refute") || name == "flunk"
            });
            if asserts {
                splice_test_helpers(tm, std::slice::from_ref(top));
            }
        }
    }
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
    let mut wanted: crate::det::HashSet<Symbol> = crate::det::HashSet::default();
    let mut consts: crate::det::HashSet<Symbol> = crate::det::HashSet::default();
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
    methods: &mut crate::det::HashSet<Symbol>,
    consts: &mut crate::det::HashSet<Symbol>,
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

/// The block-taking methods an app adds to the routes DSL by prepending
/// (or including) a module into `ActionDispatch::Routing::Mapper`:
///
/// ```ruby
/// ActionDispatch::Routing::Mapper.prepend(Podding::RoutingAnnotations)
/// # ... then in routes.rb:
/// routing_method :main_pod do ... end
/// ```
///
/// Read off the initializers and `lib/` the way Rails loads them: a
/// top-level `Mapper.prepend(Const)`, with `Const` looked up among the
/// ingested modules. The methods that take a block are the DSL scopes;
/// the module's other methods are not.
fn mapper_extension_block_methods<V: Vfs + ?Sized>(
    app: &App,
    vfs: &V,
    dir: &Path,
) -> crate::det::HashSet<String> {
    const TARGET: &str = "Routing::Mapper";
    let mut modules: Vec<String> = Vec::new();
    for sub in ["config/initializers", "lib"] {
        let d = dir.join(sub);
        if !vfs.is_dir(&d) {
            continue;
        }
        for entry in read_rb_files(vfs, &d).unwrap_or_default() {
            let Ok(bytes) = vfs.read(&entry) else { continue };
            // Cheap reject before parsing every initializer and lib file.
            if !bytes.windows(TARGET.len()).any(|w| w == TARGET.as_bytes()) {
                continue;
            }
            let file = entry.display().to_string();
            let result = super::prism::parse(&bytes, &file);
            let src = String::from_utf8_lossy(&bytes).into_owned();
            let root = result.node();
            let Some(program) = root.as_program_node() else { continue };
            for stmt in initializer_statements(&program) {
                let Some(call) = stmt.as_call_node() else { continue };
                if !matches!(super::util::constant_id_str(&call.name()), "prepend" | "include") {
                    continue;
                }
                let Some(recv) = call.receiver() else { continue };
                if constant_text(&recv, &src)
                    .map(|t| t.trim_start_matches("::") == format!("ActionDispatch::{TARGET}"))
                    != Some(true)
                {
                    continue;
                }
                let Some(args) = call.arguments() else { continue };
                for arg in args.arguments().iter() {
                    if let Some(name) = constant_text(&arg, &src) {
                        modules.push(name.trim_start_matches("::").to_string());
                    }
                }
            }
        }
    }
    // The DSL's own scoping macros, which the routes ingest handles
    // natively; a same-named override in the extension wraps them and
    // still means the native thing.
    const NATIVE: [&str; 9] = [
        "resources", "resource", "collection", "member", "namespace", "scope", "constraints",
        "concern", "concerns",
    ];
    let mut out = crate::det::HashSet::default();
    for name in modules {
        // Top-level `Mapper.prepend(Const)` resolves `Const` by its
        // written name; a suffix match would also pull in unrelated
        // `Other::Const` modules that share only the final segment.
        for lc in &app.library_classes {
            let full = lc.name.0.as_str();
            if full != name {
                continue;
            }
            for m in &lc.methods {
                if m.receiver == MethodReceiver::Instance
                    && m.block_param.is_some()
                    && !NATIVE.contains(&m.name.as_str())
                {
                    out.insert(m.name.as_str().to_string());
                }
            }
        }
    }
    out
}

// Not left on the declaring base: an abstract base's `enum :state` reads through the subclass's own column reader, which has to know the mapping.
fn inherit_enums(models: &mut [crate::dialect::Model]) {
    let declared: crate::det::HashMap<crate::ident::ClassId, (Option<crate::ident::ClassId>, indexmap::IndexMap<crate::ident::Symbol, Vec<(String, crate::expr::Literal)>>)> =
        models.iter().map(|m| (m.name.clone(), (m.parent.clone(), m.enums.clone()))).collect();
    let defaults: crate::det::HashMap<crate::ident::ClassId, indexmap::IndexMap<crate::ident::Symbol, crate::expr::Literal>> =
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

/// `config.i18n.default_locale = :xx` in `config/application.rb`, else `en`.
/// The locale a trimmed config line's `config.i18n.default_locale = :xx` names.
pub(super) fn default_locale_line(line: &str) -> Option<String> {
    let rest = line.strip_prefix("config.i18n.default_locale").map(str::trim)?;
    locale_literal(rest.strip_prefix('=')?.trim())
}

fn default_locale<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> String {
    let path = dir.join("config/application.rb");
    let source = vfs.read_to_string(&path).unwrap_or_default();
    source
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .find_map(default_locale_line)
        .unwrap_or_else(|| "en".to_string())
}

/// What a trimmed config line's `config.i18n.available_locales = …` sets:
/// a literal list, or why it cannot be read.
pub(super) fn available_locales_line(line: &str) -> Option<Result<Vec<String>, String>> {
    let value = line.strip_prefix("config.i18n.available_locales")?.trim().strip_prefix('=')?;
    let value = value.split('#').next().unwrap_or("").trim();
    Some(locale_list(value).ok_or_else(|| format!("`available_locales = {value}` is computed at boot")))
}

/// What a trimmed config line's `config.i18n.fallbacks = …` sets: the
/// default locales the Fallbacks backend appends (`true` is the default
/// locale, `[]` none), `None` when it leaves the backend out, or why it
/// cannot be read. A Hash of per-locale chains is not modeled.
pub(super) fn fallbacks_line(line: &str, default: &str) -> Option<Result<Option<Vec<String>>, String>> {
    let value = line.strip_prefix("config.i18n.fallbacks")?.trim().strip_prefix('=')?;
    let value = value.split('#').next().unwrap_or("").trim();
    Some(match value {
        "true" => Ok(Some(vec![default.to_string()])),
        "false" | "nil" => Ok(None),
        // Not the app's default for `I18n.default_locale`: a config file runs before the i18n railtie applies it, so the gem's `:en` is what it reads.
        _ => locale_list(&value.replace("I18n.default_locale", ":en"))
            .map(Some)
            .ok_or_else(|| format!("`fallbacks = {value}` is not a list of locales")),
    })
}

/// A warning about what the app's i18n reads from its gems.
fn warn_i18n(detail: String) {
    let mut d = crate::diagnostic::Diagnostic::unsupported(crate::span::Span::synthetic(), None, "I18n", detail);
    d.severity = crate::diagnostic::Severity::Warning;
    super::prism::push(d);
}

/// Each locale's own plural rule: rails-i18n adds its pluralization
/// module unless `config.rails_i18n.enabled_modules` leaves it out, and
/// `config/locales/plurals.rb` is a file I18n's Pluralization backend
/// reads, which wins where both name a locale.
fn plural_rules<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    lock: Option<&crate::gems::Lockfile>,
    catalog: &crate::i18n::Catalog,
) -> std::collections::BTreeMap<String, String> {
    let mut locales = catalog.locales();
    for other in &catalog.others {
        if !locales.contains(&other.locale) {
            locales.push(other.locale.clone());
        }
    }
    let application = vfs.read_to_string(&dir.join("config/application.rb")).unwrap_or_default();
    let mut out: std::collections::BTreeMap<String, String> = crate::i18n::rails_i18n::plural_rules(lock, &application, &locales).into_iter().collect();
    if let Ok(source) = vfs.read_to_string(&dir.join("config/locales/plurals.rb")) {
        for (locale, rule) in crate::i18n::plural::cldr_rules(&source) {
            if locales.contains(&locale) {
                out.insert(locale, rule.unwrap_or_default());
            }
        }
    }
    out
}

/// `%i[en pl]`, `%w[en pl]`, `[:en, :pl]` or `["en", "pl"]`.
fn locale_list(value: &str) -> Option<Vec<String>> {
    if let Some(rest) = value.strip_prefix("%i").or_else(|| value.strip_prefix("%w")) {
        let inner = rest.strip_prefix(['[', '(']).and_then(|r| r.strip_suffix([']', ')']))?;
        return Some(inner.split_whitespace().map(str::to_string).collect());
    }
    let inner = value.strip_prefix('[')?.strip_suffix(']')?.trim();
    if inner.is_empty() {
        return Some(Vec::new());
    }
    inner.split(',').map(|item| locale_literal(item.trim())).collect()
}

/// `config.i18n.available_locales` and `config.i18n.fallbacks` from
/// `config/application.rb`, and the fallbacks each
/// `config/environments/<env>.rb` sets for itself. With no
/// `available_locales`, the default locale and those the app's own
/// locale files name; the same when it is computed at boot, with why it
/// cannot be read as the third value.
fn locale_settings<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    default: &str,
    files: &[(String, String)],
) -> (Vec<String>, crate::i18n::FallbackPolicy, Option<String>) {
    let lines = |path: &Path| -> Vec<String> {
        let source = vfs.read_to_string(path).unwrap_or_default();
        source.lines().map(str::trim).filter(|l| !l.starts_with('#')).map(str::to_string).collect()
    };
    let mut policy = crate::i18n::FallbackPolicy::default();
    let app_lines = lines(&dir.join("config/application.rb"));
    let mut available_unknown = None;
    let mut available = match app_lines.iter().find_map(|l| available_locales_line(l)) {
        Some(Ok(list)) => list,
        Some(Err(why)) => {
            available_unknown = Some(why);
            crate::i18n::Catalog::file_locales(files)
        }
        None => crate::i18n::Catalog::file_locales(files),
    };
    if !available.iter().any(|l| l == default) {
        available.insert(0, default.to_string());
    }
    match app_lines.iter().find_map(|l| fallbacks_line(l, default)) {
        Some(Ok(setting)) => policy.app = setting,
        Some(Err(why)) => policy.unknown = Some(why),
        None => {}
    }
    let envs_dir = dir.join("config/environments");
    let mut envs = if vfs.is_dir(&envs_dir) { vfs.read_dir(&envs_dir).unwrap_or_default() } else { Vec::new() };
    envs.sort();
    for path in envs {
        let Some(env) = path.file_stem().and_then(|s| s.to_str()).filter(|_| path.extension().is_some_and(|e| e == "rb")) else { continue };
        match lines(&path).iter().find_map(|l| fallbacks_line(l, default)) {
            Some(Ok(setting)) => policy.envs.push((env.to_string(), setting)),
            Some(Err(why)) => policy.unknown = Some(why),
            None => {}
        }
    }
    (available, policy, available_unknown)
}

/// What a trimmed config line's `config.i18n.raise_on_missing_translations = v` sets.
pub(super) fn raise_on_missing_line(l: &str) -> Option<Result<bool, String>> {
    let value = l.strip_prefix("config.i18n.raise_on_missing_translations")?.trim().strip_prefix('=')?;
    let value = value.split('#').next().unwrap_or("").trim();
    Some(match value {
        "true" => Ok(true),
        "false" | "nil" => Ok(false),
        ":strict" => Err("`raise_on_missing_translations = :strict` also raises from ActiveModel, which is not modeled".to_string()),
        _ => Err(format!("`raise_on_missing_translations = {value}` is computed at boot")),
    })
}

/// `config.i18n.raise_on_missing_translations` from `config/application.rb`
/// and each `config/environments/<env>.rb`.
fn missing_policy<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> crate::i18n::MissingPolicy {
    let setting = |path: &Path| -> Option<Result<bool, String>> {
        let source = vfs.read_to_string(path).ok()?;
        source.lines().map(str::trim).filter(|l| !l.starts_with('#')).find_map(raise_on_missing_line)
    };
    let mut policy = crate::i18n::MissingPolicy::default();
    match setting(&dir.join("config/application.rb")) {
        Some(Ok(raises)) => policy.raises = raises,
        Some(Err(why)) => policy.unknown = Some(why),
        None => {}
    }
    let envs_dir = dir.join("config/environments");
    let mut envs = if vfs.is_dir(&envs_dir) { vfs.read_dir(&envs_dir).unwrap_or_default() } else { Vec::new() };
    envs.sort();
    for path in envs {
        let Some(env) = path.file_stem().and_then(|s| s.to_str()).filter(|_| path.extension().is_some_and(|e| e == "rb")) else { continue };
        match setting(&path) {
            Some(Ok(raises)) if raises != policy.raises => policy.flipped.push(env.to_string()),
            Some(Err(why)) => policy.unknown = Some(why),
            _ => {}
        }
    }
    policy
}

/// The config file that sets `field_error_proc`, wherever Rails reads it from.
fn field_error_proc_file<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> Option<String> {
    let mut files = vec![dir.join("config/application.rb")];
    for sub in ["config/environments", "config/initializers"] {
        let d = dir.join(sub);
        let mut found = if vfs.is_dir(&d) { vfs.read_dir(&d).unwrap_or_default() } else { Vec::new() };
        found.sort();
        files.extend(found.into_iter().filter(|p| p.extension().is_some_and(|e| e == "rb")));
    }
    files.into_iter().find(|path| {
        vfs.read_to_string(path)
            .is_ok_and(|source| source.lines().any(assigns_field_error_proc))
    })
    .map(|p| p.strip_prefix(dir).unwrap_or(&p).to_string_lossy().into_owned())
}

/// `….field_error_proc = …`, and not a line that only mentions the name.
fn assigns_field_error_proc(line: &str) -> bool {
    let code = line.split('#').next().unwrap_or("");
    code.match_indices(".field_error_proc").any(|(at, name)| {
        let rest = code[at + name.len()..].trim_start();
        rest.starts_with('=') && !rest.starts_with("==")
    })
}

/// `:fr`, `:"pt-BR"`, `"fr"` or `'fr'` at the start of `value`, ignoring what follows it.
fn locale_literal(value: &str) -> Option<String> {
    let bare = value.strip_prefix(':');
    let value = bare.unwrap_or(value);
    let name = match value.chars().next() {
        Some(quote @ ('"' | '\'')) => value[1..].split(quote).next()?,
        _ if bare.is_some() => value.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).next()?,
        _ => return None,
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// `config.assume_ssl = true` on an uncommented line of an environment
/// file. A trailing `#` comment after the value is Ruby, so it is cut
/// before the comparison; a line that is itself a comment is not.
pub(crate) fn sets_assume_ssl(text: &str) -> bool {
    text.lines().any(|line| {
        let t = line.trim();
        t.strip_prefix("config.assume_ssl")
            .map(|rest| rest.trim_start())
            .and_then(|rest| rest.strip_prefix('='))
            .is_some_and(|rest| rest.split('#').next().unwrap_or("").trim() == "true")
    })
}

#[cfg(test)]
mod locale_tests {
    use super::locale_literal;

    #[test]
    fn a_default_locale_literal_drops_a_trailing_comment_but_not_a_quoted_hash() {
        assert_eq!(locale_literal(":fr # French").as_deref(), Some("fr"));
        assert_eq!(locale_literal(":\"pt-BR\"").as_deref(), Some("pt-BR"));
        assert_eq!(locale_literal("\"fr#CA\" # comment").as_deref(), Some("fr#CA"));
        assert_eq!(locale_literal("'ja'").as_deref(), Some("ja"));
        assert_eq!(locale_literal("ENV[\"LOCALE\"]"), None);
    }
}

#[cfg(test)]
mod importmap_json_tests {
    use super::importmap_json_method;
    use crate::app::ImportmapPin;

    #[test]
    fn importmap_json_is_the_pretty_script_body() {
        let pins = vec![
            ImportmapPin { name: "application".into(), path: "/assets/application.js".into() },
            ImportmapPin {
                name: "@hotwired/turbo-rails".into(),
                path: "/assets/turbo.js".into(),
            },
        ];
        let method = importmap_json_method(&pins).expect("pins");
        let json = "{\n  \"imports\": {\n    \"application\": \"/assets/application.js\",\n    \"@hotwired/turbo-rails\": \"/assets/turbo.js\"\n  }\n}";
        assert!(
            method.contains(&format!("{json:?}")),
            "CSP hashes this string; it has to be the importmap script body:\n{method}"
        );
    }
}

#[cfg(test)]
mod filter_parameters_tests {
    use super::{extract_filter_parameters, FilterParametersLine};

    #[test]
    fn a_read_of_the_list_is_not_a_write() {
        let src = b"filter = ActiveSupport::ParameterFilter.new(Rails.application.config.filter_parameters)\n";
        assert_eq!(extract_filter_parameters(src), Vec::<FilterParametersLine>::new());
        let src = b"Rails.application.config.filter_parameters << :token\nRails.application.config.filter_parameters.push(:otp)\n";
        assert_eq!(
            extract_filter_parameters(src),
            vec![FilterParametersLine::Computed { replace: false }, FilterParametersLine::Computed { replace: false }]
        );
    }
}

#[cfg(test)]
mod csp_mapping_tests {
    use super::content_security_policy_method;

    #[test]
    fn every_rails_source_symbol_is_mapped() {
        let method = content_security_policy_method(
            "Rails.application.configure do\n  config.content_security_policy do |policy|\n    policy.img_src :self, :data, :blob\n    policy.connect_src :self, :wss\n    policy.script_src :self, :strict_dynamic\n  end\nend\n",
        )
        .expect("a policy");
        assert!(method.contains(r#"policy.set_directive("img-src", ["'self'", "data:", "blob:"])"#), "{method}");
        assert!(method.contains(r#"policy.set_directive("connect-src", ["'self'", "wss:"])"#), "{method}");
        assert!(method.contains(r#"policy.set_directive("script-src", ["'self'", "'strict-dynamic'"])"#), "{method}");
    }
}

#[cfg(test)]
mod path_gem_vfs_tests {
    use super::PathGemVfs;
    use crate::vfs::MapVfs;
    use std::path::{Path, PathBuf};

    /// The binary search over the sorted directories answers what the scan
    /// it replaced did: `path.starts_with(dir)` for some `dir`, whatever
    /// order the directories were given in.
    #[test]
    fn within_dirs_agrees_with_a_prefix_scan() {
        let vfs = MapVfs::new(Default::default());
        let dirs: Vec<PathBuf> =
            ["/app/vendor/zeta", "/app/engines/a", "/app/vendor/alpha", "/app/engines/a/inner", "/app/vendor/alpha"]
                .iter()
                .map(PathBuf::from)
                .collect();
        let linked = PathGemVfs::new(&vfs, Path::new("/app"), &dirs);
        for path in [
            "/app",
            "/app/vendor",
            "/app/vendor/alpha",
            "/app/vendor/alpha/lib/x.rb",
            "/app/vendor/alphabet/lib/x.rb",
            "/app/vendor/zeta",
            "/app/vendor/zeta/lib",
            "/app/engines/a",
            "/app/engines/a/inner/lib/y.rb",
            "/app/engines/b/lib/y.rb",
            "/app/engines",
            "/other/vendor/alpha",
            "relative/vendor/alpha",
        ] {
            let path = Path::new(path);
            assert_eq!(
                linked.within_dirs(path),
                dirs.iter().any(|dir| path.starts_with(dir)),
                "{}",
                path.display()
            );
        }
    }
}
