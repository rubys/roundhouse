//! Library-shape Ruby emission — for transpiled-shape input where class
//! bodies already contain explicit methods (no Rails DSL expansion).
//! Mirrors `src/emit/typescript/library.rs` in scope; produces one
//! `app/models/<name>.rb` per `LibraryClass`.
//!
//! Ruby is implicit about ivar declaration and global about constant
//! resolution, so this emitter is shorter than the TS analog: no ivar
//! field block, no import partition.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;
use std::path::{Path, PathBuf};

use super::super::EmittedFile;
use crate::App;
use crate::dialect::{AccessorKind, LibraryClass, MethodReceiver};
use crate::expr::{Expr, ExprNode, InterpPart, LValue};
use crate::ident::{ClassId, Symbol, VarId};
use crate::naming::{singularize, snake_case};
use crate::span::Span;

pub(super) fn emit_library_class_decls(app: &App) -> Vec<EmittedFile> {
    let mut lcs: Vec<LibraryClass> = app.library_classes.clone();
    apply_scope_lowering(&mut lcs, app);
    apply_helper_lowering(&mut lcs, app);
    apply_duration_lowering(&mut lcs);
    // Transpiled-shape classes carry hand-written accessors that
    // `synth_attr_reader` never sees, so the datetime reader/writer
    // rewrite still runs here for them (Ruby-only). Model-lowered classes
    // get the reader from `synth_attr_reader` (shared, all targets); this
    // re-applies the same reader idempotently and adds the Ruby writer
    // normalize.
    apply_datetime_lowering(&mut lcs, app);
    lcs.iter()
        .flat_map(|lc| {
            let file_stem = snake_case(lc.name.0.as_str());
            let out_path = PathBuf::from(format!("app/models/{file_stem}.rb"));
            emit_library_class_pair(lc, app, out_path)
        })
        .collect()
}

/// Ruby-family pre-emit pass: synthesize each model's scope class methods
/// and normalize scope chains (`Story.base(u).positive_ranked` ->
/// `Story.positive_ranked(Story.base(u))`) across a group of library
/// classes, so they run against `ActiveRecord::Relation`. Lives on the
/// Ruby emit path — these methods reference a runtime only the CRuby/JRuby
/// tree provides, so the shared `lower/` must stay target-agnostic. Run
/// once per LC group (models / controllers / library_classes) before
/// rendering. A strict no-op for scope-free apps (the blog).
pub(crate) fn apply_scope_lowering(lcs: &mut [LibraryClass], app: &App) {
    let scopes = crate::lower::scope_chain::build_scope_registry(&app.models);
    if !crate::lower::scope_chain::any_scopes(&scopes) {
        return;
    }
    let names = crate::lower::scope_chain::all_scope_names(&scopes);
    let models = crate::lower::scope_chain::model_set(&app.models);
    for lc in lcs.iter_mut() {
        // Models gain their scope class methods (already chain-normalized).
        if let Some(model) = app.models.iter().find(|m| m.name == lc.name) {
            crate::lower::model_to_library::push_scope_methods(
                &mut lc.methods,
                model,
                &scopes,
                &models,
            );
        }
        // Every method body: normalize scope chains (call-site form).
        for m in &mut lc.methods {
            if crate::lower::scope_chain::mentions_scope(&m.body, &names) {
                crate::lower::scope_chain::rewrite_call_site(&mut m.body, &scopes, &models);
            }
        }
    }
}

/// Ruby-family pre-emit pass: resolve `app/helpers/*` references. Rails
/// mixes every helper module into every view as instance methods, but the
/// post-lowering IR emits views/controllers/helpers as module-functions
/// (`Views::Stories.listdetail`, `ApplicationHelper.avatar_img`), so a bare
/// `avatar_img(...)` rendered into a view body has no `self` to dispatch on
/// and raises `NoMethodError`. This pass (a) flips each helper module's own
/// methods to class methods so `ApplicationHelper.avatar_img` is a real
/// call target, and (b) rewrites every bare call whose name the helper
/// registry knows — in whatever LibraryClass bodies it's run over — into
/// `<DefiningModule>.method(...)`. Lives on the Ruby emit path: helper
/// modules are app-specific and the rewrite targets a CRuby call shape, so
/// shared `lower/` stays target-agnostic (same rule as scope lowering). A
/// strict no-op when the app has no non-empty helpers — the blog's helper
/// modules are empty, so `helper_method_index` is empty.
pub(crate) fn apply_helper_lowering(lcs: &mut [LibraryClass], app: &App) {
    if app.helper_method_index.is_empty() {
        return;
    }
    let helper_modules: BTreeSet<ClassId> =
        app.helper_method_index.values().cloned().collect();
    for lc in lcs.iter_mut() {
        let is_helper_module = helper_modules.contains(&lc.name);
        for m in &mut lc.methods {
            // A helper module's own methods become module-functions so the
            // rewritten `Module.method` call has a real target — Rails mixed
            // them into a view instance, but the emitted views are module
            // functions with no instance to receive them.
            if is_helper_module && m.receiver == MethodReceiver::Instance {
                m.receiver = MethodReceiver::Class;
            }
            rewrite_helper_calls(&mut m.body, &app.helper_method_index);
        }
    }
}

/// Framework view helpers callable from a helper/model body that the
/// view-template classifier (which runs only on views) never reaches —
/// plus bare calls in view bodies the classifier has no kind for (it
/// handles a fixed set; the rest fall through to this pass). They
/// resolve to `ActionView::ViewHelpers.<name>`. Grown as GET / surfaces
/// each one: asset helpers (`avatar_img` → `image_tag` → `image_path`),
/// then the date helpers + `content_tag` (`time_ago_in_words_label`
/// calls both bare).
fn is_framework_view_helper(name: &str) -> bool {
    matches!(
        name,
        "image_tag"
            | "image_path"
            | "content_tag"
            | "time_ago_in_words"
            | "distance_of_time_in_words"
    )
}

/// `<Const ending in Base>.helpers` — the Rails idiom
/// `ActionController::Base.helpers.image_path(...)` used to reach view
/// helpers from a model. Collapses to the ViewHelpers module so the
/// trailing call resolves as `ActionView::ViewHelpers.image_path(...)`.
fn is_base_dot_helpers(node: &ExprNode) -> bool {
    if let ExprNode::Send { recv: Some(r), method, args, block, .. } = node {
        if method.as_str() == "helpers" && args.is_empty() && block.is_none() {
            if let ExprNode::Const { path } = &*r.node {
                return path.last().map(|s| s.as_str() == "Base").unwrap_or(false);
            }
        }
    }
    false
}

/// `Rails.application.routes.url_helpers` — the Rails idiom for reaching
/// route helpers from a model body (`Story#short_id_path` does
/// `...url_helpers.root_path + "s/#{short_id}"`). Collapses to the
/// generated `RouteHelpers` module so the trailing `.root_path` resolves
/// as `RouteHelpers.root_path`.
fn is_rails_url_helpers(node: &ExprNode) -> bool {
    let mut cur = node;
    for step in ["url_helpers", "routes", "application"] {
        let ExprNode::Send { recv: Some(r), method, args, block, .. } = cur else {
            return false;
        };
        if method.as_str() != step || !args.is_empty() || block.is_some() {
            return false;
        }
        cur = &r.node;
    }
    matches!(cur, ExprNode::Const { path }
        if path.last().map(|s| s.as_str() == "Rails").unwrap_or(false))
}

fn view_helpers_path() -> Vec<Symbol> {
    vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")]
}

/// Walk a method body and rewrite helper calls so they resolve against the
/// module-function surfaces. Four cases (children first, so nested calls
/// and the `.helpers` receiver are rewritten before their parent):
///   1. `<…Base>.helpers` → the `ActionView::ViewHelpers` constant.
///   2. `Rails.application.routes.url_helpers` → the `RouteHelpers` constant.
///   3. bare `name(args)` where `name` is an app helper → `<Module>.name(args)`.
///   4. bare `name(args)` where `name` is a framework view helper →
///      `ActionView::ViewHelpers.name(args)`.
/// Only receiver-less Sends are rewritten in (3)/(4): a call with a receiver
/// already resolves, and a bare identifier with no call shape is a local read.
fn rewrite_helper_calls(expr: &mut Expr, index: &HashMap<Symbol, ClassId>) {
    expr.node.for_each_child_mut(&mut |c| rewrite_helper_calls(c, index));

    // Case 1: collapse `<…Base>.helpers` to the ViewHelpers module constant.
    if is_base_dot_helpers(&expr.node) {
        *expr.node = ExprNode::Const { path: view_helpers_path() };
        return;
    }

    // Case 2: collapse `Rails.application.routes.url_helpers` to RouteHelpers.
    if is_rails_url_helpers(&expr.node) {
        *expr.node = ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] };
        return;
    }

    // Cases 3/4: a bare call resolving to an app or framework helper module.
    let path: Option<Vec<Symbol>> = match &*expr.node {
        ExprNode::Send { recv: None, method, .. } => {
            if let Some(module) = index.get(method) {
                Some(module.0.as_str().split("::").map(Symbol::from).collect())
            } else if is_framework_view_helper(method.as_str()) {
                Some(view_helpers_path())
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(path) = path {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { method, args, block, .. } = node else { unreachable!() };
        *expr.node = ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path })),
            method,
            args,
            block,
            parenthesized: true,
        };
    }
}

/// Ruby-family pre-emit pass: rewrite ActiveSupport duration builders
/// (`70.days`, `NEW_USER_DAYS.days`, `1.week`) — which would call a
/// nonexistent `Integer#days` — into `ActiveSupport::Duration.days(...)`
/// against the CRuby-only Duration overlay. Reopening `Integer` in the
/// shared runtime is off-limits (no built-in subclassing; `Time` arithmetic
/// doesn't transpile uniformly), so this and the runtime both stay on the
/// Ruby tree. `<dur>.ago` / `.from_now` then ride the returned Duration
/// instance and need no rewrite. A strict no-op for duration-free apps
/// (the blog).
pub(crate) fn apply_duration_lowering(lcs: &mut [LibraryClass]) {
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            rewrite_durations(&mut m.body);
        }
    }
}

/// ActiveSupport duration unit method names (`70.days`, `1.week`). The
/// singular `day`/`hour`/`month`/`year` also name `Time` component readers
/// (`created_at.day`), so those rewrite only when the receiver is numeric;
/// the others — every plural, plus `minute`/`second`/`week`/`fortnight` —
/// never collide and rewrite unconditionally (so an Int constant receiver
/// like `NEW_USER_DAYS.days`, whose type may be unresolved, still lands).
fn duration_unit_collides_with_time(unit: &str) -> bool {
    matches!(unit, "day" | "hour" | "month" | "year")
}

fn is_duration_unit(unit: &str) -> bool {
    matches!(
        unit,
        "days" | "day" | "hours" | "hour" | "minutes" | "minute" | "seconds" | "second"
            | "weeks" | "week" | "fortnights" | "fortnight" | "months" | "month" | "years" | "year"
    )
}

/// Is `e` a numeric value — an Int/Float literal or an expression the typer
/// resolved to `Int`/`Float`? (Used to keep `created_at.day` — a Str-typed
/// datetime — out of the colliding-unit rewrite.)
fn is_numeric_expr(e: &Expr) -> bool {
    if matches!(&*e.node, ExprNode::Lit { value: crate::expr::Literal::Int { .. } })
        || matches!(&*e.node, ExprNode::Lit { value: crate::expr::Literal::Float { .. } })
    {
        return true;
    }
    matches!(&e.ty, Some(crate::ty::Ty::Int) | Some(crate::ty::Ty::Float))
}

fn rewrite_durations(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite_durations);
    let rewrite = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if args.is_empty() && is_duration_unit(method.as_str()) =>
        {
            !duration_unit_collides_with_time(method.as_str()) || is_numeric_expr(r)
        }
        _ => false,
    };
    if rewrite {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { recv, method, .. } = node else { unreachable!() };
        let arg = recv.expect("duration send has a receiver");
        let path = vec![Symbol::from("ActiveSupport"), Symbol::from("Duration")];
        *expr.node = ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path })),
            method,
            args: vec![arg],
            block: None,
            parenthesized: true,
        };
    }
}

/// Ruby-family pre-emit pass for TRANSPILED-SHAPE classes only: models
/// whose accessors were hand-written in the source (`attr_accessor
/// :created_at`) and so never passed through the shared model lowering.
/// Schema-synthesized models don't need it — `schema::synth_attr_reader`
/// already splits storage (`@<col>_raw`, String) from access (a reader
/// parsing via `ActiveSupport.parse_db_time`), for every target.
///
/// For the hand-written shape (storage under `@<col>` itself), the
/// reader becomes `@col && ActiveSupport.parse_db_time(@col)` — short-
/// circuits a nullable column's `nil` without needing to know
/// nullability; `parse_db_time` (not bare `Time.parse`) treats a
/// zone-less column as UTC rather than the system's local zone (see
/// `active_support_time_parsing.rb`). The writer becomes
/// `@col = (value.respond_to?(:iso8601) ? value.iso8601 : value)` —
/// normalizing a `Time` passed by app code back to text so every write
/// lands on the same on-disk format. A strict no-op for apps with no
/// Date/DateTime/Time columns (the blog) and for schema-synthesized
/// models (the plain-ivar-read gate below).
pub(crate) fn apply_datetime_lowering(lcs: &mut [LibraryClass], app: &App) {
    for model in &app.models {
        let Some(table) = app.schema.tables.get(&model.table.0) else {
            continue;
        };
        let temporal: BTreeSet<Symbol> = table
            .columns
            .iter()
            .filter(|c| {
                matches!(
                    c.col_type,
                    crate::schema::ColumnType::Date
                        | crate::schema::ColumnType::DateTime
                        | crate::schema::ColumnType::Time
                )
            })
            .map(|c| c.name.clone())
            .collect();
        if temporal.is_empty() {
            continue;
        }
        let Some(lc) = lcs.iter_mut().find(|lc| lc.name == model.name) else {
            continue;
        };
        for m in &mut lc.methods {
            if m.receiver != MethodReceiver::Instance {
                continue;
            }
            match m.kind {
                // Only a PLAIN `@col`-read body — the hand-written
                // `attr_reader` shape from transpiled fixtures. A
                // schema-synthesized model's temporal reader already
                // parses its `@<col>_raw` storage ivar (see
                // `schema::synth_attr_reader`); overwriting it here
                // would re-point the read at a nonexistent `@<col>`.
                AccessorKind::AttributeReader
                    if temporal.contains(&m.name)
                        && is_plain_ivar_read(&m.body, &m.name) =>
                {
                    m.body = temporal_reader_body(&m.name);
                }
                // Hand-written temporal writers only, same reasoning:
                // synthesized models write storage via `<col>_raw=`
                // (never named after a temporal column), so any
                // `<col>=` writer matching the temporal set is the
                // transpiled-fixture shape that needs the Time→text
                // normalize.
                AccessorKind::AttributeWriter => {
                    let col = Symbol::from(m.name.as_str().trim_end_matches('='));
                    if temporal.contains(&col) {
                        if let Some(param) = m.params.first() {
                            m.body = temporal_writer_body(&col, &param.name);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// True when a reader body is exactly `@<name>` — the untouched
/// hand-written `attr_reader` shape (vs a synthesized parsing body).
fn is_plain_ivar_read(body: &Expr, name: &Symbol) -> bool {
    matches!(&*body.node, ExprNode::Ivar { name: n } if n == name)
}

fn datetime_ivar(col: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Ivar { name: col.clone() })
}

fn datetime_var(name: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: name.clone() })
}

/// `@col && ActiveSupport.parse_db_time(@col)`.
fn temporal_reader_body(col: &Symbol) -> Expr {
    // `ActiveSupport.parse_db_time` (not bare `Time.parse`) — a stored
    // column with no zone marker is always implicitly UTC (Rails/sqlite3
    // convention), but `Time.parse` defaults an absent zone to the
    // *system's local zone*. See `active_support_time_parsing.rb`.
    let parse_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
            )),
            method: Symbol::from("parse_db_time"),
            args: vec![datetime_ivar(col)],
            block: None,
            parenthesized: true,
        },
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::And,
            surface: crate::expr::BoolOpSurface::Symbol,
            left: datetime_ivar(col),
            right: parse_call,
        },
    )
}

/// `@col = (value.respond_to?(:iso8601) ? value.iso8601 : value)`.
fn temporal_writer_body(col: &Symbol, value_param: &Symbol) -> Expr {
    let responds = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(datetime_var(value_param)),
            method: Symbol::from("respond_to?"),
            args: vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit {
                    value: crate::expr::Literal::Sym { value: Symbol::from("iso8601") },
                },
            )],
            block: None,
            parenthesized: true,
        },
    );
    let iso_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(datetime_var(value_param)),
            method: Symbol::from("iso8601"),
            args: Vec::new(),
            block: None,
            parenthesized: true,
        },
    );
    let normalized = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: responds,
            then_branch: iso_call,
            else_branch: datetime_var(value_param),
        },
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Ivar { name: col.clone() },
            value: normalized,
        },
    )
}

/// Emit both the `.rb` file and its `.rbs` sidecar for a single
/// LibraryClass. The sidecar carries the typed-signature view of the
/// same class shape — spinel reads it as an inference hint (see
/// project_rbs_emit_opportunity.md / spinel#571), and Steep/TypeProf
/// can consume it from the CRuby target.
pub(super) fn emit_library_class_pair(
    lc: &LibraryClass,
    app: &App,
    out_path: PathBuf,
) -> Vec<EmittedFile> {
    let rb = emit_library_class_decl(lc, app, out_path.clone());
    let rbs = super::rbs::emit_library_class_rbs(lc, &out_path);
    vec![rb, rbs]
}

/// Pair variant for callers that pass synthesized sibling anchors.
pub(super) fn emit_library_class_pair_with_synthesized(
    lc: &LibraryClass,
    app: &App,
    out_path: PathBuf,
    synthesized_siblings: &[(String, String)],
) -> Vec<EmittedFile> {
    let rb = emit_library_class_decl_with_synthesized(
        lc,
        app,
        out_path.clone(),
        synthesized_siblings,
    );
    let rbs = super::rbs::emit_library_class_rbs(lc, &out_path);
    vec![rb, rbs]
}

/// Emit a group of LibraryFunctions sharing a `module_path` as a
/// single Ruby file. Mirrors `typescript::library::emit_module_file`
/// — converts the function group into a synthetic
/// `LibraryClass{is_module:true}` with class-method (`def self.X`)
/// declarations, then delegates to `emit_library_class_decl` so
/// require resolution, nested-module rendering, and method body
/// emission share one code path with class-shaped artifacts.
///
/// `module_function` would be the more idiomatic Ruby spelling,
/// but `def self.X` is what the existing spinel-blog hand-written
/// modules use AND what `emit_method` already produces — going
/// through that path keeps shapes byte-identical.
pub(super) fn emit_module_file(
    funcs: &[crate::dialect::LibraryFunction],
    app: &App,
    out_path: PathBuf,
) -> EmittedFile {
    if funcs.is_empty() {
        // No functions in the module — emit a placeholder file with
        // just the module wrapper. Callers can guard upstream by
        // checking the lowerer's output and not calling this when
        // they know the module would be empty.
        return EmittedFile { path: out_path, content: String::new() };
    }
    let lc = synthesize_module_lc(funcs);
    emit_library_class_decl(&lc, app, out_path)
}

/// Pair variant of `emit_module_file` — emits both `.rb` and `.rbs`.
pub(super) fn emit_module_file_pair(
    funcs: &[crate::dialect::LibraryFunction],
    app: &App,
    out_path: PathBuf,
) -> Vec<EmittedFile> {
    if funcs.is_empty() {
        return vec![EmittedFile { path: out_path, content: String::new() }];
    }
    let lc = synthesize_module_lc(funcs);
    let rb = emit_library_class_decl(&lc, app, out_path.clone());
    let rbs = super::rbs::emit_library_class_rbs(&lc, &out_path);
    vec![rb, rbs]
}

/// Emit only the `.rbs` sidecar for a `LibraryClass`. Used when the
/// `.rb` emit has bespoke post-processing the pair helpers can't
/// model (e.g. test files with autorun shim + preamble).
pub(super) fn emit_rbs_sidecar(lc: &LibraryClass, rb_path: &std::path::Path) -> EmittedFile {
    super::rbs::emit_library_class_rbs(lc, rb_path)
}

/// Emit only the `.rbs` sidecar derived from a `LibraryFunction` group.
/// Companion to `emit_rbs_sidecar` for module-shaped output whose `.rb`
/// emit flows through a bespoke path (e.g. `config/routes.rb`).
pub(super) fn emit_rbs_sidecar_from_funcs(
    funcs: &[crate::dialect::LibraryFunction],
    rb_path: &std::path::Path,
) -> EmittedFile {
    let lc = synthesize_module_lc(funcs);
    super::rbs::emit_library_class_rbs(&lc, rb_path)
}

fn synthesize_module_lc(
    funcs: &[crate::dialect::LibraryFunction],
) -> LibraryClass {
    use crate::dialect::{AccessorKind, MethodDef, MethodReceiver};
    use crate::ident::Symbol;

    let module_id = funcs
        .first()
        .map(|f| {
            ClassId(Symbol::from(
                f.module_path
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("::"),
            ))
        })
        .unwrap_or_else(|| ClassId(Symbol::from("")));
    let methods: Vec<MethodDef> = funcs
        .iter()
        .map(|f| MethodDef {
            name: f.name.clone(),
            receiver: MethodReceiver::Class,
            params: f.params.clone(),
            body: f.body.clone(),
            signature: f.signature.clone(),
            effects: f.effects.clone(),
            enclosing_class: Some(module_id.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        })
        .collect();
    LibraryClass {
        name: module_id,
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods,
        origin: None,
        constants: Vec::new(),
    }
}

/// Emit a single library-shape file. `out_path` is the project-root-relative
/// destination for the file; the require resolver computes paths relative to
/// `out_path`'s parent, so files emitted to `app/views/<plural>/` get
/// `../../../runtime/<x>` while files in `app/models/` get `../../runtime/<x>`.
pub(super) fn emit_library_class_decl(
    lc: &LibraryClass,
    app: &App,
    out_path: PathBuf,
) -> EmittedFile {
    emit_library_class_decl_with_synthesized(lc, app, out_path, &[])
}

/// Variant that also accepts a list of (class_name, anchor) pairs for
/// synthesized siblings (e.g. `<Model>Row`, `<Resource>Params`) that
/// aren't in `app.library_classes` / `app.models`. Synthesized classes
/// have no separate require chain — nothing else loads them — so a
/// file that references one needs an explicit `require_relative`,
/// even when the target is in the same directory. Callers that don't
/// emit synthesized siblings pass an empty slice.
pub(super) fn emit_library_class_decl_with_synthesized(
    lc: &LibraryClass,
    app: &App,
    out_path: PathBuf,
    synthesized_siblings: &[(String, String)],
) -> EmittedFile {
    let name = lc.name.0.as_str();
    let out_dir = out_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(PathBuf::new);
    let self_anchor = out_path.with_extension("").to_string_lossy().into_owned();
    let mut s = String::new();

    // Parent + body-derived `require_relative` headers. Helpers return
    // project-root-anchored paths; we relpath each one against `out_dir`
    // so emit works correctly from any output directory.
    let mut requires: Vec<String> = Vec::new();
    if let Some(parent) = lc.parent.as_ref() {
        if let Some(anchor) = require_path_for_parent(parent, app) {
            if anchor != self_anchor {
                requires.push(relpath(&out_dir, &anchor));
            }
        }
    }
    // `include`d modules must be LOADED before the `include` executes at
    // class-definition time — unlike body const-refs (request-time), so we
    // require them even when they're same-dir siblings (plain Ruby has no
    // Rails autoload). Resolve through the same model/library_class anchor.
    for inc in &lc.includes {
        let path = vec![inc.0.as_str().to_string()];
        if let Some(anchor) = require_path_for_body_const(&path, app, name) {
            if anchor != self_anchor {
                requires.push(relpath(&out_dir, &anchor));
            }
        }
    }
    let mut const_paths: BTreeSet<Vec<String>> = BTreeSet::new();
    for m in &lc.methods {
        walk_const_paths(&m.body, &mut const_paths);
    }
    for (_, value) in &lc.constants {
        walk_const_paths(value, &mut const_paths);
    }
    let mut body_requires: BTreeSet<String> = BTreeSet::new();
    for path in &const_paths {
        let first = match path.first() {
            Some(s) => s,
            None => continue,
        };
        // Synthesized siblings: emit require regardless of same-dir,
        // because nothing else loads them. Match by exact first-segment
        // name; deeper paths (`X::Y`) don't match here since synthesized
        // classes are flat.
        if let Some((_, anchor)) = synthesized_siblings.iter().find(|(n, _)| n == first) {
            if anchor != &self_anchor {
                body_requires.insert(relpath(&out_dir, anchor));
                continue;
            }
        }
        if let Some(anchor) = require_path_for_body_const(path, app, name) {
            if anchor != self_anchor && !is_same_dir(&out_dir, &anchor) {
                body_requires.insert(relpath(&out_dir, &anchor));
            }
        }
    }
    requires.extend(body_requires);
    for r in &requires {
        writeln!(s, "require_relative {r:?}").unwrap();
    }
    if !requires.is_empty() {
        writeln!(s).unwrap();
    }

    // Compound names like `Views::Articles` emit as nested
    // `module Views\n  module Articles` rather than `module Views::Articles`.
    // Compound-form headers blow up at load time when the outer namespace
    // isn't already defined (Ruby looks up `Views` as a constant); nested
    // headers create the chain on the fly. Spinel-blog's hand-written
    // views use the nested form for the same reason.
    let segments: Vec<&str> = name.split("::").collect();
    let depth = segments.len();
    let body_pad = "  ".repeat(depth);

    if lc.is_module {
        // Modules don't take a parent; ingest already enforces this.
        for (i, seg) in segments.iter().enumerate() {
            writeln!(s, "{}module {seg}", "  ".repeat(i)).unwrap();
        }
    } else {
        // Outer segments (if any) are namespace modules; the last is the class.
        for (i, seg) in segments.iter().take(depth - 1).enumerate() {
            writeln!(s, "{}module {seg}", "  ".repeat(i)).unwrap();
        }
        let last = segments[depth - 1];
        let pad = "  ".repeat(depth - 1);
        match lc.parent.as_ref() {
            Some(p) => writeln!(s, "{pad}class {last} < {}", p.0.as_str()).unwrap(),
            None => writeln!(s, "{pad}class {last}").unwrap(),
        }
    }

    for inc in &lc.includes {
        writeln!(s, "{body_pad}include {}", inc.0.as_str()).unwrap();
    }
    if !lc.includes.is_empty() && !lc.methods.is_empty() {
        writeln!(s).unwrap();
    }

    // Class-level constants (`NAME = <expr>`), emitted before methods so
    // refs in method bodies resolve. A multi-line value (proc/array) keeps
    // its continuation lines indented to the class body.
    for (cname, value) in &lc.constants {
        let rendered = super::emit_expr(value);
        let mut lines = rendered.lines();
        match lines.next() {
            Some(first_line) => {
                writeln!(s, "{body_pad}{} = {first_line}", cname.as_str()).unwrap();
                for line in lines {
                    if line.is_empty() {
                        writeln!(s).unwrap();
                    } else {
                        writeln!(s, "{body_pad}{line}").unwrap();
                    }
                }
            }
            None => writeln!(s, "{body_pad}{} = nil", cname.as_str()).unwrap(),
        }
    }
    if !lc.constants.is_empty() && !lc.methods.is_empty() {
        writeln!(s).unwrap();
    }

    let mut first = true;
    for m in &lc.methods {
        if !first {
            writeln!(s).unwrap();
        }
        first = false;
        let body = super::emit_method(m);
        for line in body.lines() {
            if line.is_empty() {
                writeln!(s).unwrap();
            } else {
                writeln!(s, "{body_pad}{line}").unwrap();
            }
        }
    }

    for i in (0..depth).rev() {
        writeln!(s, "{}end", "  ".repeat(i)).unwrap();
    }

    EmittedFile { path: out_path, content: s }
}

/// Project-root-anchored require target for a parent class, if one is needed.
/// `ActiveRecord::Base` lives in the runtime; same-dir parents
/// (ApplicationRecord, custom abstract bases) resolve to a sibling under
/// `app/models/`. Everything else returns `None` (assume the loader sees
/// the parent some other way).
fn require_path_for_parent(parent: &ClassId, app: &App) -> Option<String> {
    let raw = parent.0.as_str();
    if raw == "ActiveRecord::Base" {
        return Some("runtime/active_record".to_string());
    }
    if raw == "ActionController::Base" || raw == "ActionController::API" {
        return Some("runtime/action_controller".to_string());
    }
    if app.models.iter().any(|m| m.name.0.as_str() == raw)
        || app.library_classes.iter().any(|lc| lc.name.0.as_str() == raw)
    {
        return Some(format!("app/models/{}", snake_case(raw)));
    }
    if app.controllers.iter().any(|c| c.name.0.as_str() == raw) {
        return Some(format!("app/controllers/{}", snake_case(raw)));
    }
    None
}

/// Core Ruby classes an app may reopen (monkeypatch) without them being
/// `app/models` files. Kept in sync with `rbs::is_builtin_class_name`.
fn is_core_class_name(name: &str) -> bool {
    matches!(
        name,
        "Integer"
            | "Float"
            | "String"
            | "Symbol"
            | "TrueClass"
            | "FalseClass"
            | "NilClass"
            | "Array"
            | "Hash"
            | "Object"
            | "Numeric"
            | "Comparable"
            | "Enumerable"
            | "Kernel"
    )
}

/// Project-root-anchored require target for a body-referenced constant.
/// `Views::<Plural>` resolves to `app/views/<plural>/_<singular>`; runtime
/// modules resolve to `runtime/<x>`. The caller relpaths the result against
/// the requirer's `out_dir`, so a single mapping serves every output kind.
/// Same-dir siblings (other models, library_classes) drop because Ruby's
/// load path covers them; unknowns drop silently.
fn require_path_for_body_const(
    path: &[String],
    app: &App,
    self_name: &str,
) -> Option<String> {
    let first = path.first()?;
    if first == self_name {
        return None;
    }
    // A core Ruby class an app reopens (e.g. `class String` in
    // lib/monkey.rb) lands in `library_classes`, but it is NOT an
    // `app/models/<name>` file — every `String.new` would otherwise emit
    // a dangling `require_relative "app/models/string"`. The reference
    // resolves to the builtin; any monkeypatch is loaded via its own file
    // (lib/), not through this model anchor.
    if is_core_class_name(first) {
        return None;
    }
    if app.models.iter().any(|m| m.name.0.as_str() == first.as_str())
        || app
            .library_classes
            .iter()
            .any(|lc| lc.name.0.as_str() == first.as_str())
    {
        return Some(format!("app/models/{}", snake_case(first)));
    }
    if app.controllers.iter().any(|c| c.name.0.as_str() == first.as_str()) {
        return Some(format!("app/controllers/{}", snake_case(first)));
    }
    match first.as_str() {
        // `Views::*` refs always go through the per-app aggregator at
        // `app/views.rb` (spinel-blog convention; loads all view
        // modules so any `Views::X.method` resolves regardless of
        // which template the method lives in). Per-template requires
        // would be wrong because the same `Views::X` const can host
        // methods from multiple files (`_article.rb`, `index.rb`,
        // `show.rb` all re-open `module Views::Articles`).
        "Views" => Some("app/views".to_string()),
        // Runtime modules under `runtime/`. ViewHelpers still ships
        // hand-written; RouteHelpers is now generated into
        // `app/route_helpers.rb` from `app.routes` so consumers
        // resolve there. Add entries as lowerings introduce new ones;
        // unknown idents silently drop.
        "Broadcasts" => Some("runtime/broadcasts".to_string()),
        "Inflector" => Some("runtime/inflector".to_string()),
        "ViewHelpers" => Some("runtime/action_view".to_string()),
        "RouteHelpers" => Some("app/route_helpers".to_string()),
        _ => None,
    }
}

/// True when `to_anchor` lives in `from_dir`. Used to drop body-ref
/// requires for same-dir siblings — Ruby's `require_relative` for a
/// bare-name target works, but the load order isn't guaranteed when
/// the file just lazily references the sibling at call time. Same-
/// dir body refs are skipped (loader picks them up); cross-dir refs
/// emit an explicit require.
fn is_same_dir(from_dir: &Path, to_anchor: &str) -> bool {
    let to_dir: String = to_anchor
        .rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .unwrap_or_default();
    from_dir.to_str().unwrap_or("") == to_dir
}

/// Compute a `require_relative`-style relative path from `from_dir` to
/// the project-root-anchored `to_anchor`. Both inputs are slash-separated;
/// the result has no `.rb` extension because `require_relative` doesn't
/// need one.
fn relpath(from_dir: &Path, to_anchor: &str) -> String {
    let from_parts: Vec<&str> = from_dir
        .to_str()
        .unwrap_or("")
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let to_parts: Vec<&str> = to_anchor.split('/').filter(|s| !s.is_empty()).collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let ups = from_parts.len() - common;
    let mut parts: Vec<&str> = std::iter::repeat("..").take(ups).collect();
    parts.extend(&to_parts[common..]);
    parts.join("/")
}

pub(super) fn walk_const_paths(e: &Expr, out: &mut BTreeSet<Vec<String>>) {
    match &*e.node {
        ExprNode::Const { path } => {
            out.insert(path.iter().map(|s| s.as_str().to_string()).collect());
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                walk_const_paths(r, out);
            }
            for a in args {
                walk_const_paths(a, out);
            }
            if let Some(b) = block {
                walk_const_paths(b, out);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            walk_const_paths(fun, out);
            for a in args {
                walk_const_paths(a, out);
            }
            if let Some(b) = block {
                walk_const_paths(b, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk_const_paths(k, out);
                walk_const_paths(v, out);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                walk_const_paths(el, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    walk_const_paths(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk_const_paths(left, out);
            walk_const_paths(right, out);
        }
        ExprNode::Let { value, body, .. } => {
            walk_const_paths(value, out);
            walk_const_paths(body, out);
        }
        ExprNode::Lambda { body, .. } => walk_const_paths(body, out),
        ExprNode::If { cond, then_branch, else_branch } => {
            walk_const_paths(cond, out);
            walk_const_paths(then_branch, out);
            walk_const_paths(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            walk_const_paths(scrutinee, out);
            for arm in arms {
                walk_const_paths(&arm.body, out);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                walk_const_paths(e, out);
            }
        }
        ExprNode::Assign { value, .. } => walk_const_paths(value, out),
        ExprNode::Yield { args } => {
            for a in args {
                walk_const_paths(a, out);
            }
        }
        ExprNode::Raise { value } => walk_const_paths(value, out),
        ExprNode::RescueModifier { expr, fallback } => {
            walk_const_paths(expr, out);
            walk_const_paths(fallback, out);
        }
        ExprNode::Return { value } => walk_const_paths(value, out),
        ExprNode::Super { args: Some(args) } => {
            for a in args {
                walk_const_paths(a, out);
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            walk_const_paths(body, out);
            for r in rescues {
                walk_const_paths(&r.body, out);
            }
            if let Some(e) = else_branch {
                walk_const_paths(e, out);
            }
            if let Some(e) = ensure {
                walk_const_paths(e, out);
            }
        }
        ExprNode::Next { value: Some(v) } => walk_const_paths(v, out),
        ExprNode::MultiAssign { value, .. } => walk_const_paths(value, out),
        ExprNode::While { cond, body, .. } => {
            walk_const_paths(cond, out);
            walk_const_paths(body, out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                walk_const_paths(b, out);
            }
            if let Some(e) = end {
                walk_const_paths(e, out);
            }
        }
        // Leaves and uninteresting nodes pass through.
        _ => {}
    }
}
