//! Roda application ingestion — the whole-app orchestrator and the
//! routing-tree linearizer for the Roda + Sequel front-end (issue #67;
//! mapping table in `docs/roda-sequel-plan.md`).
//!
//! A Roda app has no `config/routes.rb` — the route table is a block
//! tree of matcher calls (`r.on "articles"`, `r.on Integer`, `r.is`,
//! `r.get "new"`). Each request takes exactly one root→leaf path, so
//! the tree flattens to one record per path:
//!
//! - **route** = the concatenation of matcher segments along the path
//!   → `RouteSpec::Explicit` (or `Root`);
//! - **prologue** = the interior statements along the path (loads,
//!   guards) → a synthesized `before_action`-style `Filter` + private
//!   method on the synthesized controller, so the existing
//!   filter-chain / halt machinery is reused as-is;
//! - **terminal block body** = the controller action body.
//!
//! Interior aborts map onto the halt model: `next unless @x = ...`
//! (block → nil → route unhandled → not_found) becomes "assign; unless
//! set, render the 404 template and return", the same shape a Rails
//! guard filter lowers to.
//!
//! Everything outside the recognized vocabulary (matchers, plugins,
//! `r.*` calls) is a ledger entry — survey mode records and drops,
//! strict mode aborts. Never a silent gap.

use std::path::Path;

use ruby_prism::Node;

use crate::App;
use crate::dialect::{
    Action, Controller, ControllerBodyItem, Filter, FilterKind, HttpMethod, LayoutDecl,
    LibraryClass, RouteSpec, RouteTable,
};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, Literal};
use crate::naming::camelize;
use crate::span::Span;
use crate::ty::Row;
use crate::vfs::Vfs;
use crate::{ClassId, Symbol};

use super::app::{read_erb_files, read_rb_files};
use super::controller::infer_render_template;
use super::expr::{ingest_expr, ingest_ruby_program};
use super::model::ingest_method;
use super::sequel_migration::ingest_sequel_migration;
use super::sequel_model::{hash_kwargs, ingest_sequel_model, normalize_sequel_expr, sym_lit};
use super::util::{constant_id_str, constant_path_of, find_first_class, flatten_statements};
use super::view::ingest_template;
use super::survey::{self, unwrap_or_record};
use super::{IngestError, IngestResult};

/// Does `dir` hold a Roda app? A rack entry point with no Rails route
/// file, whose `app.rb` subclasses `Roda`.
pub fn is_roda_app<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> bool {
    vfs.exists(&dir.join("config.ru"))
        && !vfs.exists(&dir.join("config/routes.rb"))
        && vfs
            .read(&dir.join("app.rb"))
            .map(|src| String::from_utf8_lossy(&src).contains("< Roda"))
            .unwrap_or(false)
}

/// Ingest an entire Roda + Sequel app directory. The Roda-app analog
/// of `ingest_app_with_vfs` — same output contract (`App`), different
/// conventions walked: `db/migrate` (Sequel DSL), `models/`,
/// `views/`, `app.rb`, `seeds.rb`.
pub fn ingest_roda_app_with_vfs<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<App> {
    super::sources::reset();
    let mut app = App::new();

    // Schema — Sequel apps have no schema.rb; fold migrations in
    // filename order (integer prefixes sort chronologically).
    let migrate_dir = dir.join("db/migrate");
    if vfs.is_dir(&migrate_dir) {
        for entry in read_rb_files(vfs, &migrate_dir)? {
            let source = vfs.read(&entry)?;
            unwrap_or_record(ingest_sequel_migration(
                &source,
                &entry.display().to_string(),
                &mut app.schema,
            ))?;
        }
    }

    // db.rb — connection + model-wide behavior. One setting is
    // load-bearing for the whole lowering: `raise_on_save_failure =
    // false` makes `#save` return nil/false like ActiveRecord's, which
    // every `if model.save` branch in the linearized actions relies
    // on. Without it Sequel's `#save` raises and the lowering is wrong
    // — refuse rather than mis-transpile.
    let db_path = dir.join("db.rb");
    if vfs.exists(&db_path) {
        let source = vfs.read(&db_path)?;
        let text = String::from_utf8_lossy(&source);
        let has_no_raise = text.lines().any(|l| {
            let t = l.trim_start();
            !t.starts_with('#')
                && t.contains("raise_on_save_failure")
                && t.contains("false")
        });
        if !has_no_raise {
            let err = IngestError::Unsupported {
                file: db_path.display().to_string(),
                message: "Sequel::Model.raise_on_save_failure = false is required: \
                          without it #save raises instead of returning false, and \
                          every `if model.save` lowering would be wrong"
                    .into(),
            };
            if !survey::is_active() {
                return Err(err);
            }
            survey::record(&err);
        }
    }

    // Models — `models/*.rb`, each a `Sequel::Model` subclass. The
    // synthesized ApplicationRecord base mirrors what a Rails app's
    // app/models/application_record.rb ingests to: every Sequel model
    // reparents onto it, and emit's require graph and the runtime's
    // ActiveRecord::Base hierarchy expect it to exist.
    app.models.push(crate::dialect::Model {
        name: ClassId(Symbol::from("ApplicationRecord")),
        parent: Some(ClassId(Symbol::from("ActiveRecord::Base"))),
        table: crate::ident::TableRef(Symbol::from("application_records")),
        attributes: Row::closed(),
        body: Vec::new(),
        span: Span::synthetic(),
    });
    let models_dir = dir.join("models");
    if vfs.is_dir(&models_dir) {
        for entry in read_rb_files(vfs, &models_dir)? {
            let source = vfs.read(&entry)?;
            if let Some(maybe_model) = unwrap_or_record(ingest_sequel_model(
                &source,
                &entry.display().to_string(),
                &app.schema,
            ))? {
                if let Some(model) = maybe_model {
                    app.models.push(model);
                }
            }
        }
    }

    // The Roda class — plugins, helpers, and the routing tree.
    // `layout_stem` is the render plugin's layout template name; the
    // view walk below re-homes that template onto Rails' one
    // conventional layout slot.
    let app_rb = dir.join("app.rb");
    let mut layout_stem = "layout".to_string();
    if vfs.exists(&app_rb) {
        let source = vfs.read(&app_rb)?;
        match ingest_roda_class(&source, &app_rb.display().to_string(), &mut app) {
            Ok(stem) => layout_stem = stem,
            Err(err) if survey::is_active() => survey::record(&err),
            Err(err) => return Err(err),
        }
    }

    // Views — `views/**/*.erb`. Same generic template pipeline as
    // Rails views; two dialect touches: Roda's `part(...)` partial
    // calls normalize to `render`, and the render plugin's layout
    // template re-homes onto `layouts/application` — Rails' one
    // conventional layout slot, which the layout-wrap lowering and
    // every emitter key on. A Roda app has exactly one app-wide
    // layout, so the mapping is semantically exact.
    let views_dir = dir.join("views");
    if vfs.is_dir(&views_dir) {
        // Roda's `part("articles/_form", article:, action:, …)` kwargs
        // ARE the partial's signature — each call names every local the
        // partial receives. Collect them per partial (first call wins;
        // the vocabulary is closed by construction) and stamp them as
        // the partial's strict-locals row, the same fixed-signature
        // channel a Rails `<%# locals: (…) %>` header feeds.
        let mut part_locals: std::collections::HashMap<String, Vec<Symbol>> =
            std::collections::HashMap::new();
        for (erb_path, engine) in read_erb_files(vfs, &views_dir)? {
            let source = vfs.read_to_string(&erb_path)?;
            let rel = erb_path
                .strip_prefix(&views_dir)
                .map_err(|_| IngestError::Unsupported {
                    file: erb_path.display().to_string(),
                    message: "view path outside views dir".into(),
                })?;
            if let Some(mut view) = unwrap_or_record(ingest_template(
                &source,
                rel,
                &erb_path.display().to_string(),
                engine.compile_fn(),
            ))? {
                if view.name.as_str() == layout_stem {
                    view.name = Symbol::from("layouts/application");
                }
                rewrite_part_to_render(&mut view.body, &mut part_locals);
                rewrite_errors_full_messages(&mut view.body);
                app.views.push(view);
            }
        }
        for view in &mut app.views {
            if let Some(locals) = part_locals.get(view.name.as_str()) {
                view.strict_locals = Some(
                    locals
                        .iter()
                        .map(|n| crate::dialect::Param::keyword(n.clone(), None))
                        .collect(),
                );
            }
        }
    }

    // Seeds — same channel as Rails' db/seeds.rb, minus the boot
    // requires (they load the app, which the transpiled world does by
    // construction) and with the Sequel spellings normalized.
    let seeds_path = dir.join("seeds.rb");
    if vfs.exists(&seeds_path) {
        let source = vfs.read_to_string(&seeds_path)?;
        if let Some(mut expr) = unwrap_or_record(ingest_ruby_program(
            &source,
            &seeds_path.display().to_string(),
        ))? {
            if let ExprNode::Seq { exprs } = &mut *expr.node {
                exprs.retain(|e| {
                    !matches!(
                        &*e.node,
                        ExprNode::Send { recv: None, method, .. }
                            if matches!(method.as_str(), "require" | "require_relative")
                    )
                });
            }
            normalize_sequel_expr(&mut expr);
            app.seeds = Some(expr);
        }
    }

    app.sources = super::sources::drain();
    app.root = dir.display().to_string().trim_end_matches('/').to_string();
    Ok(app)
}

// The Roda class walk ----------------------------------------------------

/// Per-app facts gathered from the class body before the route tree
/// is linearized.
struct RodaClassFacts {
    /// `plugin :render, layout: "..."` — the app-wide layout template.
    layout: String,
    /// Template the `not_found` plugin block renders; interior aborts
    /// synthesize `render "<this>", status: :not_found`.
    not_found_template: String,
}

fn ingest_roda_class(source: &[u8], file: &str, app: &mut App) -> IngestResult<String> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();
    let Some(class) = find_first_class(&root) else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "no class found in Roda app file".into(),
        });
    };
    if !class
        .superclass()
        .and_then(|n| constant_path_of(&n))
        .is_some_and(|p| p == ["Roda"])
    {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "app class does not subclass Roda".into(),
        });
    }

    let mut facts = RodaClassFacts {
        layout: "layout".to_string(),
        not_found_template: "not_found".to_string(),
    };
    let mut helper_methods: Vec<crate::dialect::MethodDef> = Vec::new();
    let mut route_call: Option<ruby_prism::CallNode<'_>> = None;

    let Some(class_body) = class.body() else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "Roda class has an empty body".into(),
        });
    };
    for stmt in flatten_statements(class_body) {
        if let Some(def) = stmt.as_def_node() {
            // Instance methods on the app class are view helpers
            // (Roda renders templates against the app instance). They
            // register like app-helper functions below.
            helper_methods.push(ingest_method(&def, file)?);
            continue;
        }
        let Some(call) = stmt.as_call_node() else {
            ledger(file, "Roda class-body statement not recognized")?;
            continue;
        };
        if call.receiver().is_some() {
            ledger(file, "receiver-qualified Roda class-body call not recognized")?;
            continue;
        }
        match constant_id_str(&call.name()) {
            "plugin" => ingest_plugin_call(&call, file, &mut facts)?,
            "use" => {
                // `use Rack::MethodOverride` — the hidden-`_method`
                // override Rails installs implicitly; the runtime
                // router already honors it. Other middleware is
                // unmodeled.
                let target = call
                    .arguments()
                    .and_then(|a| a.arguments().iter().next())
                    .and_then(|n| constant_path_of(&n));
                if target.as_deref() != Some(&["Rack".into(), "MethodOverride".into()][..]) {
                    ledger(file, "Roda `use` middleware not recognized")?;
                }
            }
            "route" => {
                if call.block().is_some() {
                    route_call = Some(call);
                } else {
                    ledger(file, "route call without a block")?;
                }
            }
            other => {
                ledger(file, &format!("Roda class-body call not recognized: {other}"))?;
            }
        }
    }

    // Helper registration — mirrors the app/helpers pass of the Rails
    // walk: methods land on a synthesized ApplicationHelper module and
    // in the bare-call registry the view lowering consults (registry
    // wins over same-named framework helpers, preserving shadowing).
    if !helper_methods.is_empty() {
        let helper_id = ClassId(Symbol::from("ApplicationHelper"));
        for m in &helper_methods {
            app.helper_method_index.insert(m.name.clone(), helper_id.clone());
        }
        app.library_classes.push(LibraryClass {
            name: helper_id,
            is_module: true,
            parent: None,
            includes: Vec::new(),
            methods: helper_methods,
            origin: None,
            constants: Vec::new(),
        });
    }

    // ApplicationController — the synthesized parent every linearized
    // controller inherits from. Layout stays `Inherit`: the app's one
    // layout re-homes to `layouts/application` (the convention
    // default), so no explicit declaration is needed.
    app.controllers.push(Controller {
        name: ClassId(Symbol::from("ApplicationController")),
        parent: Some(ClassId(Symbol::from("ActionController::Base"))),
        body: Vec::new(),
        layout: LayoutDecl::Inherit,
        sibling_classes: Vec::new(),
    });

    let Some(route_call) = route_call else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "Roda class has no route block".into(),
        });
    };
    let block = route_call
        .block()
        .and_then(|b| b.as_block_node())
        .ok_or_else(|| IngestError::Unsupported {
            file: file.into(),
            message: "route block shape not recognized".into(),
        })?;
    let r_name = block
        .parameters()
        .and_then(|p| p.as_block_parameters_node())
        .and_then(|bp| bp.parameters())
        .and_then(|pn| pn.requireds().iter().next())
        .and_then(|n| n.as_required_parameter_node().map(|rp| rp.name()))
        .map(|n| constant_id_str(&n).to_string())
        .ok_or_else(|| IngestError::Unsupported {
            file: file.into(),
            message: "route block must name its request parameter (`route do |r|`)".into(),
        })?;

    let mut walker = RouteWalker {
        file,
        r_name,
        not_found_template: facts.not_found_template.clone(),
        prologues: Vec::new(),
        leaves: Vec::new(),
    };
    if let Some(body) = block.body() {
        walker.walk_block(flatten_statements(body), WalkCtx::default())?;
    }
    walker.assemble(app);
    Ok(facts.layout)
}

/// Recognize one `plugin :name[, opts][ { block }]` call against the
/// allowlist. Unknown plugins are ledger entries — a plugin can change
/// request semantics arbitrarily, so pretending not to see one is the
/// silent-gap failure mode this front-end refuses.
fn ingest_plugin_call(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    facts: &mut RodaClassFacts,
) -> IngestResult<()> {
    let args: Vec<Node<'_>> = call
        .arguments()
        .map(|a| a.arguments().iter().collect())
        .unwrap_or_default();
    let Some(name) = args.first().and_then(|n| super::util::symbol_value(n)) else {
        return ledger(file, "plugin call without a symbol name");
    };
    let kwarg = |key: &str| -> Option<Node<'_>> {
        args.iter().find_map(|a| {
            let kh = a.as_keyword_hash_node()?;
            for el in kh.elements().iter() {
                let assoc = el.as_assoc_node()?;
                if super::util::symbol_value(&assoc.key()).as_deref() == Some(key) {
                    return Some(assoc.value());
                }
            }
            None
        })
    };
    match name.as_str() {
        "render" => {
            // `escape: true` aligns Roda's `<%= %>` with Rails' (and
            // this pipeline's) escape-by-default. Without it every
            // `<%=` in the source is raw output and the transpiled
            // auto-escape would change rendering — refuse to guess.
            if kwarg("escape").and_then(|v| super::util::bool_value(&v)) != Some(true) {
                ledger(file, "plugin :render without escape: true (escape semantics would differ)")?;
            }
            if let Some(layout) = kwarg("layout").and_then(|v| super::util::string_value(&v)) {
                facts.layout = layout;
            }
        }
        // part: partial rendering (normalized to `render` in view
        // bodies); all_verbs: r.patch/r.delete; sessions + flash: the
        // runtime's session-backed flash covers both.
        "part" | "all_verbs" | "sessions" | "flash" => {}
        "not_found" => {
            // The block names the app's 404 template: `plugin :not_found
            // do view "not_found" end`.
            if let Some(body) = call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body())
            {
                let stmts = flatten_statements(body);
                if let [only] = stmts.as_slice() {
                    if let Some(inner) = only.as_call_node() {
                        if constant_id_str(&inner.name()) == "view" {
                            if let Some(t) = inner
                                .arguments()
                                .and_then(|a| a.arguments().iter().next())
                                .and_then(|n| super::util::string_value(&n))
                            {
                                facts.not_found_template = t;
                                return Ok(());
                            }
                        }
                    }
                }
            }
            ledger(file, "plugin :not_found block shape not recognized")?;
        }
        other => {
            ledger(file, &format!("Roda plugin not in the recognized set: {other}"))?;
        }
    }
    Ok(())
}

/// Record an unsupported construct: survey mode ledgers and continues,
/// strict mode aborts. The single choke point for stance (c) of the
/// plan.
fn ledger(file: &str, message: &str) -> IngestResult<()> {
    let err = IngestError::Unsupported { file: file.into(), message: message.into() };
    if survey::is_active() {
        survey::record(&err);
        Ok(())
    } else {
        Err(err)
    }
}

// The routing-tree linearizer -------------------------------------------

/// One path segment of a linearized route.
#[derive(Clone, Debug, PartialEq)]
enum Seg {
    Literal(String),
    /// `r.on Integer do |id|` — a captured, digit-constrained segment.
    Param { name: String },
}

/// Interior statements of one tree node, already ingested and
/// rewritten; becomes a synthesized before-filter on every controller
/// whose leaves pass through the node.
struct Prologue {
    body: Vec<Expr>,
    /// The ivar the prologue's guard assigns (`@article`), when its
    /// shape is the `next unless @x = ...` idiom — names the filter
    /// method `set_<ivar>`.
    ivar: Option<Symbol>,
}

/// One root→leaf path: a concrete route plus its action body.
struct Leaf {
    method: HttpMethod,
    segs: Vec<Seg>,
    /// Segments since the controller-naming literal — what the REST
    /// recognizer names actions from.
    rel: Vec<Seg>,
    /// Lowercase literal naming the controller (`"articles"`), or
    /// `None` for the root leaf.
    controller: Option<String>,
    body: Expr,
    /// Prologue ids (indices into `RouteWalker::prologues`) on this
    /// leaf's path, root-first.
    chain: Vec<usize>,
    is_root: bool,
}

#[derive(Clone, Default)]
struct WalkCtx {
    segs: Vec<Seg>,
    rel: Vec<Seg>,
    controller: Option<String>,
    in_is: bool,
    chain: Vec<usize>,
    /// Matcher block params visible on this path (var name = route
    /// param name): bodies read them as locals, the linearized actions
    /// read `params[:<name>]`.
    params: Vec<String>,
}

struct RouteWalker<'f> {
    file: &'f str,
    r_name: String,
    not_found_template: String,
    prologues: Vec<Prologue>,
    leaves: Vec<Leaf>,
}

impl<'f> RouteWalker<'f> {
    /// Walk one block's statements. Interior statements collect into
    /// at most one prologue per node, and must precede the node's
    /// matchers: a statement between/after matcher calls would only
    /// run when earlier matchers *fail*, which per-route linearization
    /// cannot express — ledger, don't mis-lower.
    fn walk_block(&mut self, stmts: Vec<Node<'_>>, ctx: WalkCtx) -> IngestResult<()> {
        let mut interior: Vec<Node<'_>> = Vec::new();
        let mut node_prologue: Option<usize> = None;
        let mut seen_matcher = false;

        for stmt in stmts {
            let r_call = stmt
                .as_call_node()
                .filter(|c| {
                    c.receiver()
                        .and_then(|r| r.as_local_variable_read_node().map(|v| v.name()))
                        .is_some_and(|n| constant_id_str(&n) == self.r_name)
                });
            let Some(call) = r_call else {
                if seen_matcher {
                    ledger(
                        self.file,
                        "statement after a matcher in a Roda routing block \
                         (runs only when earlier matchers fail; not linearizable)",
                    )?;
                    continue;
                }
                interior.push(stmt);
                continue;
            };
            let method = constant_id_str(&call.name()).to_string();
            let verb = match method.as_str() {
                "get" => Some(HttpMethod::Get),
                "post" => Some(HttpMethod::Post),
                "put" => Some(HttpMethod::Put),
                "patch" => Some(HttpMethod::Patch),
                "delete" => Some(HttpMethod::Delete),
                _ => None,
            };
            let is_matcher =
                verb.is_some() || matches!(method.as_str(), "on" | "is" | "root");
            if !is_matcher {
                // A non-matcher `r.*` at interior position (`r.halt`,
                // `r.redirect` guards) is interior code.
                if seen_matcher {
                    ledger(self.file, "statement after a matcher in a Roda routing block")?;
                    continue;
                }
                interior.push(stmt);
                continue;
            }
            seen_matcher = true;
            // Interior statements above this matcher become the node's
            // prologue, shared by every leaf below.
            if !interior.is_empty() && node_prologue.is_none() {
                node_prologue = Some(self.build_prologue(&interior, &ctx)?);
            }
            let mut child = ctx.clone();
            if let Some(id) = node_prologue {
                if !child.chain.contains(&id) {
                    child.chain.push(id);
                }
            }

            match method.as_str() {
                "on" => self.walk_on(&call, child)?,
                "is" => {
                    if call.arguments().is_some() {
                        ledger(self.file, "r.is with matcher arguments not recognized")?;
                        continue;
                    }
                    child.in_is = true;
                    if let Some(body) =
                        call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body())
                    {
                        self.walk_block(flatten_statements(body), child)?;
                    }
                }
                "root" => {
                    let body = self.build_leaf_body(&call, &child)?;
                    self.leaves.push(Leaf {
                        method: HttpMethod::Get,
                        segs: Vec::new(),
                        rel: Vec::new(),
                        controller: None,
                        body,
                        chain: child.chain.clone(),
                        is_root: true,
                    });
                }
                _ => self.walk_verb(&call, verb.expect("verb match arm"), child)?,
            }
        }
        Ok(())
    }

    /// `r.on <matcher> do ... end` — one segment deeper.
    fn walk_on(&mut self, call: &ruby_prism::CallNode<'_>, mut ctx: WalkCtx) -> IngestResult<()> {
        let args: Vec<Node<'_>> = call
            .arguments()
            .map(|a| a.arguments().iter().collect())
            .unwrap_or_default();
        let [matcher] = args.as_slice() else {
            return ledger(self.file, "r.on with zero or multiple matchers not recognized");
        };
        if let Some(lit) = super::util::string_value(matcher) {
            ctx.segs.push(Seg::Literal(lit.clone()));
            ctx.rel = Vec::new();
            ctx.controller = Some(lit);
        } else if is_integer_class(matcher) {
            let Some(param) = block_param_name(call) else {
                return ledger(self.file, "r.on Integer without a named block parameter");
            };
            ctx.segs.push(Seg::Param { name: param.clone() });
            ctx.rel.push(Seg::Param { name: param.clone() });
            ctx.params.push(param);
        } else {
            return ledger(
                self.file,
                "r.on matcher not recognized (only string literals and Integer are)",
            );
        }
        ctx.in_is = false;
        if let Some(body) = call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body()) {
            self.walk_block(flatten_statements(body), ctx)?;
        }
        Ok(())
    }

    /// A verb call — a route leaf when it carries a path-termination
    /// check (an argument, or an enclosing `r.is`). A bare verb
    /// outside `r.is` matches trailing garbage and is not a route.
    fn walk_verb(
        &mut self,
        call: &ruby_prism::CallNode<'_>,
        verb: HttpMethod,
        mut ctx: WalkCtx,
    ) -> IngestResult<()> {
        let args: Vec<Node<'_>> = call
            .arguments()
            .map(|a| a.arguments().iter().collect())
            .unwrap_or_default();
        match args.as_slice() {
            [] => {
                if !ctx.in_is {
                    return ledger(
                        self.file,
                        "bare r.<verb> outside r.is has no path-termination check",
                    );
                }
            }
            [arg] if arg.as_true_node().is_some() => {}
            [arg] => {
                if let Some(lit) = super::util::string_value(arg) {
                    ctx.segs.push(Seg::Literal(lit.clone()));
                    ctx.rel.push(Seg::Literal(lit));
                } else if is_integer_class(arg) {
                    let Some(param) = block_param_name(call) else {
                        return ledger(
                            self.file,
                            "r.<verb> Integer without a named block parameter",
                        );
                    };
                    ctx.segs.push(Seg::Param { name: param.clone() });
                    ctx.rel.push(Seg::Param { name: param.clone() });
                    ctx.params.push(param);
                } else {
                    return ledger(self.file, "r.<verb> matcher not recognized");
                }
            }
            _ => return ledger(self.file, "r.<verb> with multiple matchers not recognized"),
        }
        let body = self.build_leaf_body(call, &ctx)?;
        self.leaves.push(Leaf {
            method: verb,
            segs: ctx.segs.clone(),
            rel: ctx.rel.clone(),
            controller: ctx.controller.clone(),
            body,
            chain: ctx.chain.clone(),
            is_root: false,
        });
        Ok(())
    }

    /// Ingest a node's interior statements into a prologue record.
    fn build_prologue(&mut self, stmts: &[Node<'_>], ctx: &WalkCtx) -> IngestResult<usize> {
        let exprs = self.ingest_and_rewrite(stmts, ctx)?;
        let ivar = exprs.iter().find_map(|e| match &*e.node {
            ExprNode::Assign { target: crate::expr::LValue::Ivar { name }, .. } => {
                Some(name.clone())
            }
            ExprNode::Seq { exprs } => exprs.iter().find_map(|e| match &*e.node {
                ExprNode::Assign { target: crate::expr::LValue::Ivar { name }, .. } => {
                    Some(name.clone())
                }
                _ => None,
            }),
            _ => None,
        });
        self.prologues.push(Prologue { body: exprs, ivar });
        Ok(self.prologues.len() - 1)
    }

    fn build_leaf_body(&self, call: &ruby_prism::CallNode<'_>, ctx: &WalkCtx) -> IngestResult<Expr> {
        let stmts = call
            .block()
            .and_then(|b| b.as_block_node())
            .and_then(|b| b.body())
            .map(flatten_statements)
            .unwrap_or_default();
        let exprs = self.ingest_and_rewrite(&stmts, ctx)?;
        Ok(seq(exprs))
    }

    /// The shared body pipeline: ingest, then the four rewrites in
    /// order — matcher params → `params[:x]`, Roda vocabulary → Rails
    /// vocabulary, interior aborts → render-404-and-return, Sequel →
    /// AR — then the assign+save→update peephole.
    fn ingest_and_rewrite(&self, stmts: &[Node<'_>], ctx: &WalkCtx) -> IngestResult<Vec<Expr>> {
        let mut exprs = Vec::with_capacity(stmts.len());
        for stmt in stmts {
            exprs.push(ingest_expr(stmt, self.file)?);
        }
        let mut out: Vec<Expr> = Vec::new();
        for mut e in exprs {
            subst_matcher_params(&mut e, &ctx.params);
            rewrite_roda_vocab(&mut e, &self.r_name);
            let expanded = rewrite_next_unless_404(e, &self.not_found_template);
            out.extend(expanded);
        }
        for e in &mut out {
            normalize_sequel_expr(e);
        }
        merge_assign_save_into_update(&mut out);
        Ok(out)
    }

    /// Turn the collected leaves into controllers + a route table on
    /// `app`.
    fn assemble(self, app: &mut App) {
        let RouteWalker { prologues, leaves, .. } = self;

        // Route table, in source (leaf) order.
        let mut entries: Vec<RouteSpec> = Vec::new();
        for leaf in &leaves {
            if leaf.is_root {
                entries.push(RouteSpec::Root { target: "root#index".to_string() });
                continue;
            }
            let controller_stem = leaf.controller.clone().unwrap_or_else(|| "root".into());
            let controller =
                ClassId(Symbol::from(format!("{}Controller", camelize(&controller_stem))));
            let mut constraints = indexmap::IndexMap::new();
            for seg in &leaf.segs {
                if let Seg::Param { name } = seg {
                    // The Integer matcher only ever matched digit
                    // segments; carry that as a Rails-style constraint.
                    constraints.insert(Symbol::from(name.as_str()), "\\d+".to_string());
                }
            }
            entries.push(RouteSpec::Explicit {
                method: leaf.method.clone(),
                path: path_of(&leaf.segs),
                controller,
                action: Symbol::from(action_name(&leaf.method, &leaf.rel)),
                as_name: None,
                constraints,
                scope: Default::default(),
            });
        }
        app.routes = RouteTable { entries };

        // Controllers — group leaves by controller stem, first-seen
        // order.
        let mut stems: Vec<String> = Vec::new();
        for leaf in &leaves {
            let stem = leaf.controller.clone().unwrap_or_else(|| "root".into());
            if !stems.contains(&stem) {
                stems.push(stem);
            }
        }
        for stem in stems {
            let class_name = format!("{}Controller", camelize(&stem));
            let my_leaves: Vec<&Leaf> = leaves
                .iter()
                .filter(|l| {
                    l.controller.clone().unwrap_or_else(|| "root".into()) == stem
                })
                .collect();
            let action_names: Vec<Symbol> = my_leaves
                .iter()
                .map(|l| Symbol::from(action_name(&l.method, &l.rel)))
                .collect();

            // Distinct prologues used by this controller's leaves, in
            // chain order.
            let mut pro_ids: Vec<usize> = Vec::new();
            for leaf in &my_leaves {
                for id in &leaf.chain {
                    if !pro_ids.contains(id) {
                        pro_ids.push(*id);
                    }
                }
            }

            let mut body: Vec<ControllerBodyItem> = Vec::new();
            for id in &pro_ids {
                let target = prologue_method_name(&prologues[*id], *id);
                let users: Vec<Symbol> = my_leaves
                    .iter()
                    .zip(&action_names)
                    .filter(|(l, _)| l.chain.contains(id))
                    .map(|(_, a)| a.clone())
                    .collect();
                // `only:` lists the guarded actions unless the filter
                // covers every action (Rails' bare-filter shape).
                let only = if users.len() == my_leaves.len() { Vec::new() } else { users };
                body.push(ControllerBodyItem::Filter {
                    filter: Filter {
                        kind: FilterKind::Before,
                        target,
                        only,
                        except: Vec::new(),
                        only_style: Default::default(),
                        except_style: Default::default(),
                        if_cond: None,
                        unless_cond: None,
                        if_cond_expr: None,
                        unless_cond_expr: None,
                    },
                    leading_comments: Vec::new(),
                    leading_blank_line: false,
                });
            }
            // Inline `params.expect(article: [...])` calls hoist into
            // the Rails-conventional `<resource>_params` private
            // helper — the shape the strong-params lowering
            // (`rewrite_to_from_raw` + `new` → `from_params`)
            // recognizes.
            let mut params_helpers: Vec<(Symbol, Expr)> = Vec::new();
            for (leaf, name) in my_leaves.iter().zip(&action_names) {
                let mut action_body = leaf.body.clone();
                hoist_params_expect(&mut action_body, &mut params_helpers);
                body.push(action_item(name.clone(), action_body));
            }
            if !pro_ids.is_empty() || !params_helpers.is_empty() {
                body.push(ControllerBodyItem::PrivateMarker {
                    leading_comments: Vec::new(),
                    leading_blank_line: true,
                });
                for id in &pro_ids {
                    let name = prologue_method_name(&prologues[*id], *id);
                    body.push(action_item(name, seq(prologues[*id].body.clone())));
                }
                for (name, expect) in params_helpers {
                    body.push(action_item(name, expect));
                }
            }
            app.controllers.push(Controller {
                name: ClassId(Symbol::from(class_name)),
                parent: Some(ClassId(Symbol::from("ApplicationController"))),
                body,
                layout: LayoutDecl::Inherit,
                sibling_classes: Vec::new(),
            });
        }
    }
}

/// Replace `params.expect(<key>: [...])` with a bare `<key>_params`
/// call, recording the hoisted helper body. One helper per key —
/// create and update share the same allow-list by construction (both
/// came from the same `set_fields` field list), so the first
/// occurrence wins.
fn hoist_params_expect(expr: &mut Expr, helpers: &mut Vec<(Symbol, Expr)>) {
    expr.node.for_each_child_mut(&mut |c| hoist_params_expect(c, helpers));
    let key: Option<Symbol> = match &*expr.node {
        ExprNode::Send { recv: Some(p), method, args, .. }
            if method.as_str() == "expect"
                && args.len() == 1
                && matches!(
                    &*p.node,
                    ExprNode::Send { recv: None, method, args, .. }
                        if method.as_str() == "params" && args.is_empty()
                ) =>
        {
            match &*args[0].node {
                ExprNode::Hash { entries, kwargs: true } if entries.len() == 1 => {
                    match &*entries[0].0.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        _ => None,
    };
    let Some(key) = key else { return };
    let helper = Symbol::from(format!("{key}_params"));
    if !helpers.iter().any(|(n, _)| *n == helper) {
        helpers.push((helper.clone(), expr.clone()));
    }
    expr.node = Box::new(ExprNode::Send {
        recv: None,
        method: helper,
        args: Vec::new(),
        block: None,
        parenthesized: false,
    });
}

fn prologue_method_name(p: &Prologue, id: usize) -> Symbol {
    match &p.ivar {
        Some(ivar) => Symbol::from(format!("set_{ivar}")),
        None => Symbol::from(format!("route_prologue_{id}")),
    }
}

fn action_item(name: Symbol, body: Expr) -> ControllerBodyItem {
    let renders = infer_render_template(&body)
        .map(|n| crate::dialect::RenderTarget::Template { name: n, formats: Vec::new() })
        .unwrap_or(crate::dialect::RenderTarget::Inferred);
    ControllerBodyItem::Action {
        action: Action {
            name,
            params: Row::closed(),
            opt_params: Vec::new(),
            block_param: None,
            body,
            renders,
            effects: EffectSet::pure(),
        },
        leading_comments: Vec::new(),
        leading_blank_line: false,
    }
}

fn path_of(segs: &[Seg]) -> String {
    let mut path = String::new();
    for seg in segs {
        path.push('/');
        match seg {
            Seg::Literal(s) => path.push_str(s),
            Seg::Param { name } => {
                path.push(':');
                path.push_str(name);
            }
        }
    }
    if path.is_empty() { "/".to_string() } else { path }
}

/// REST-shape action naming — cosmetic (it makes the IR diff cleanly
/// against the Rails fixture); the linearization beneath is purely
/// mechanical. Falls back to a mechanical `<verb>_<segments>` name.
fn action_name(method: &HttpMethod, rel: &[Seg]) -> String {
    match (method, rel) {
        (HttpMethod::Get, []) => "index".into(),
        (HttpMethod::Post, []) => "create".into(),
        (HttpMethod::Get, [Seg::Literal(l)]) if l == "new" => "new".into(),
        (HttpMethod::Get, [Seg::Param { .. }]) => "show".into(),
        (HttpMethod::Patch | HttpMethod::Put, [Seg::Param { .. }]) => "update".into(),
        (HttpMethod::Delete, [Seg::Param { .. }]) => "destroy".into(),
        (HttpMethod::Get, [Seg::Param { .. }, Seg::Literal(l)]) if l == "edit" => "edit".into(),
        _ => {
            let verb = format!("{method:?}").to_lowercase();
            let mut parts = vec![verb];
            for seg in rel {
                match seg {
                    Seg::Literal(s) => parts.push(s.clone()),
                    Seg::Param { name } => parts.push(name.clone()),
                }
            }
            parts.join("_")
        }
    }
}

fn is_integer_class(node: &Node<'_>) -> bool {
    node.as_constant_read_node()
        .map(|c| constant_id_str(&c.name()) == "Integer")
        .unwrap_or(false)
}

fn block_param_name(call: &ruby_prism::CallNode<'_>) -> Option<String> {
    call.block()
        .and_then(|b| b.as_block_node())
        .and_then(|b| b.parameters())
        .and_then(|p| p.as_block_parameters_node())
        .and_then(|bp| bp.parameters())
        .and_then(|pn| pn.requireds().iter().next())
        .and_then(|n| n.as_required_parameter_node().map(|rp| rp.name()))
        .map(|n| constant_id_str(&n).to_string())
}

// Body rewrites ----------------------------------------------------------

/// Matcher block params are locals in the source (`Article[id]`); the
/// linearized action reads route params (`params[:id]`), like every
/// Rails action.
fn subst_matcher_params(expr: &mut Expr, params: &[String]) {
    if params.is_empty() {
        return;
    }
    expr.node.for_each_child_mut(&mut |c| subst_matcher_params(c, params));
    if let ExprNode::Var { name, .. } = &*expr.node {
        if params.iter().any(|p| p == name.as_str()) {
            let read = params_index_read(name.as_str());
            expr.node = read.node;
        }
    }
}

/// `params[:id]` as an Expr.
fn params_index_read(key: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("params"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                },
            )),
            method: Symbol::from("[]"),
            args: vec![sym_lit(key)],
            block: None,
            parenthesized: false,
        },
    )
}

/// Rewrite the request-object vocabulary: `r.params` → `params`,
/// `r.redirect` → `redirect_to`, `view "x"` → `render "x"`, and
/// string-keyed params reads → the symbol form the Rails corpus uses.
fn rewrite_roda_vocab(expr: &mut Expr, r_name: &str) {
    expr.node.for_each_child_mut(&mut |c| rewrite_roda_vocab(c, r_name));

    let replacement: Option<ExprNode> = match &*expr.node {
        ExprNode::Send { recv: Some(recv), method, args, block, parenthesized } => {
            let recv_is_r = matches!(&*recv.node, ExprNode::Var { name, .. } if name.as_str() == r_name);
            if recv_is_r {
                match method.as_str() {
                    "params" if args.is_empty() => Some(ExprNode::Send {
                        recv: None,
                        method: Symbol::from("params"),
                        args: Vec::new(),
                        block: None,
                        parenthesized: false,
                    }),
                    "redirect" => Some(ExprNode::Send {
                        recv: None,
                        method: Symbol::from("redirect_to"),
                        args: args.clone(),
                        block: block.clone(),
                        parenthesized: *parenthesized,
                    }),
                    _ => None,
                }
            } else {
                // `params["article"]` → `params[:article]` — one
                // vocabulary downstream (the runtime's params are
                // string-keyed; the symbol spelling is the corpus
                // convention the emitters gate on).
                match (&*expr.node, &*recv.node) {
                    (
                        ExprNode::Send { method: m, args: idx, .. },
                        ExprNode::Send { recv: None, method: pm, args: pargs, .. },
                    ) if m.as_str() == "[]"
                        && pm.as_str() == "params"
                        && pargs.is_empty()
                        && idx.len() == 1 =>
                    {
                        match &*idx[0].node {
                            ExprNode::Lit { value: Literal::Str { value } } => {
                                Some(ExprNode::Send {
                                    recv: Some(recv.clone()),
                                    method: Symbol::from("[]"),
                                    args: vec![sym_lit(value)],
                                    block: None,
                                    parenthesized: false,
                                })
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                }
            }
        }
        ExprNode::Send { recv: None, method, args, block, parenthesized }
            if method.as_str() == "view" =>
        {
            Some(ExprNode::Send {
                recv: None,
                method: Symbol::from("render"),
                args: args.clone(),
                block: block.clone(),
                parenthesized: *parenthesized,
            })
        }
        _ => None,
    };
    if let Some(node) = replacement {
        expr.node = Box::new(node);
    }
}

/// `next unless @article = Article.find_by(...)` — Roda's interior
/// abort (block → nil → not_found renders). Linearized form: hoist the
/// assignment, and on the nil branch render the 404 template and
/// return — the shape the filter-chain halt machinery already handles.
///
/// One statement may expand to two, so this runs statement-wise.
fn rewrite_next_unless_404(expr: Expr, not_found: &str) -> Vec<Expr> {
    // Recurse into nested Seqs first (e.g. an If arm holding a Seq).
    let mut expr = expr;
    if let ExprNode::Seq { exprs } = &mut *expr.node {
        let old = std::mem::take(exprs);
        *exprs = old
            .into_iter()
            .flat_map(|e| rewrite_next_unless_404(e, not_found))
            .collect();
        return vec![expr];
    }

    let ExprNode::If { cond, then_branch, else_branch } = &*expr.node else {
        return vec![expr];
    };
    let then_trivial = matches!(&*then_branch.node, ExprNode::Lit { value: Literal::Nil })
        || matches!(&*then_branch.node, ExprNode::Seq { exprs } if exprs.is_empty());
    let else_is_next = match &*else_branch.node {
        ExprNode::Next { value: None } => true,
        ExprNode::Seq { exprs } => {
            exprs.len() == 1 && matches!(&*exprs[0].node, ExprNode::Next { value: None })
        }
        _ => false,
    };
    if !then_trivial || !else_is_next {
        return vec![expr];
    }

    let halt = seq(vec![
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("render"),
                args: vec![
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Str { value: not_found.to_string() } },
                    ),
                    hash_kwargs(vec![(sym_lit("status"), sym_lit("not_found"))]),
                ],
                block: None,
                parenthesized: false,
            },
        ),
        Expr::new(
            Span::synthetic(),
            ExprNode::Return {
                value: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            },
        ),
    ]);

    match &*cond.node {
        // `next unless @x = <expr>` — hoist the assign, guard on the
        // ivar/local read.
        ExprNode::Assign { target, value } => {
            let read = match target {
                crate::expr::LValue::Ivar { name } => {
                    Expr::new(Span::synthetic(), ExprNode::Ivar { name: name.clone() })
                }
                crate::expr::LValue::Var { id, name } => Expr::new(
                    Span::synthetic(),
                    ExprNode::Var { id: *id, name: name.clone() },
                ),
                _ => return vec![expr],
            };
            vec![
                Expr::new(
                    expr.span,
                    ExprNode::Assign { target: target.clone(), value: value.clone() },
                ),
                Expr::new(
                    expr.span,
                    ExprNode::If {
                        cond: read,
                        then_branch: Expr::new(
                            Span::synthetic(),
                            ExprNode::Lit { value: Literal::Nil },
                        ),
                        else_branch: halt,
                    },
                ),
            ]
        }
        // `next unless <cond>` without an assignment.
        _ => vec![Expr::new(
            expr.span,
            ExprNode::If {
                cond: cond.clone(),
                then_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
                else_branch: halt,
            },
        )],
    }
}

/// `x.assign_attributes(h)` immediately followed by `if x.save` is
/// ActiveRecord's `if x.update(h)` — merge so the update action's IR
/// converges with the Rails fixture's.
fn merge_assign_save_into_update(stmts: &mut Vec<Expr>) {
    let mut i = 0;
    while i + 1 < stmts.len() {
        let merged: Option<Expr> = {
            let (a, b) = (&stmts[i], &stmts[i + 1]);
            match (&*a.node, &*b.node) {
                (
                    ExprNode::Send { recv: Some(ar), method: am, args, .. },
                    ExprNode::If { cond, then_branch, else_branch },
                ) if am.as_str() == "assign_attributes" => match &*cond.node {
                    ExprNode::Send { recv: Some(cr), method: cm, args: cargs, .. }
                        if cm.as_str() == "save"
                            && cargs.is_empty()
                            && same_simple_recv(ar, cr) =>
                    {
                        Some(Expr::new(
                            b.span,
                            ExprNode::If {
                                cond: Expr::new(
                                    cond.span,
                                    ExprNode::Send {
                                        recv: Some(cr.clone()),
                                        method: Symbol::from("update"),
                                        args: args.clone(),
                                        block: None,
                                        parenthesized: true,
                                    },
                                ),
                                then_branch: then_branch.clone(),
                                else_branch: else_branch.clone(),
                            },
                        ))
                    }
                    _ => None,
                },
                _ => None,
            }
        };
        if let Some(m) = merged {
            stmts[i] = m;
            stmts.remove(i + 1);
        } else {
            i += 1;
        }
    }
}

fn same_simple_recv(a: &Expr, b: &Expr) -> bool {
    match (&*a.node, &*b.node) {
        (ExprNode::Ivar { name: an }, ExprNode::Ivar { name: bn }) => an == bn,
        (ExprNode::Var { name: an, .. }, ExprNode::Var { name: bn, .. }) => an == bn,
        _ => false,
    }
}

/// Roda's `part("dir/_name", k: v)` → the Rails partial-render
/// spelling `render("dir/name", k: v)` in view bodies (the underscore
/// is re-derived by partial resolution, same as Rails). Each call's
/// kwarg names are recorded per partial in `part_locals` — they become
/// the partial's strict-locals signature.
fn rewrite_part_to_render(
    expr: &mut Expr,
    part_locals: &mut std::collections::HashMap<String, Vec<Symbol>>,
) {
    expr.node
        .for_each_child_mut(&mut |c| rewrite_part_to_render(c, part_locals));
    let ExprNode::Send { recv: None, method, args, block, parenthesized } = &*expr.node else {
        return;
    };
    if method.as_str() != "part" {
        return;
    }
    let mut args = args.clone();
    let kwarg_names: Vec<Symbol> = args
        .iter()
        .skip(1)
        .filter_map(|a| match &*a.node {
            ExprNode::Hash { entries, kwargs: true } => Some(entries),
            _ => None,
        })
        .flatten()
        .filter_map(|(k, _)| match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
            _ => None,
        })
        .collect();
    if let Some(first) = args.first_mut() {
        if let ExprNode::Lit { value: Literal::Str { value } } = &*first.node {
            part_locals.entry(value.clone()).or_insert(kwarg_names);
            let stripped = match value.rsplit_once('/') {
                Some((dir, base)) => {
                    format!("{dir}/{}", base.strip_prefix('_').unwrap_or(base))
                }
                None => value.strip_prefix('_').unwrap_or(value).to_string(),
            };
            first.node = Box::new(ExprNode::Lit { value: Literal::Str { value: stripped } });
        }
    }
    expr.node = Box::new(ExprNode::Send {
        recv: None,
        method: Symbol::from("render"),
        args,
        block: block.clone(),
        parenthesized: *parenthesized,
    });
}

/// `record.errors.full_messages` → `record.errors`. The framework
/// runtime's `errors` is already the array of full-message strings
/// (the inline validation checks push humanized text), so Sequel's
/// explicit `.full_messages` hop is an identity there.
fn rewrite_errors_full_messages(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite_errors_full_messages);
    let replacement = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "full_messages"
                && args.is_empty()
                && matches!(
                    &*r.node,
                    ExprNode::Send { method, .. } if method.as_str() == "errors"
                ) =>
        {
            Some((*r.node).clone())
        }
        _ => None,
    };
    if let Some(node) = replacement {
        expr.node = Box::new(node);
    }
}

fn seq(exprs: Vec<Expr>) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Seq { exprs })
}
