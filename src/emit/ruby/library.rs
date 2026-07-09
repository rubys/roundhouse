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
use crate::dialect::{AccessorKind, LibraryClass, MethodReceiver, Param};
use crate::expr::{Expr, ExprNode, InterpPart, LValue};
use crate::ident::{ClassId, Symbol, VarId};
use crate::naming::snake_case;
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
    apply_secure_password_lowering(&mut lcs, app);
    apply_typed_store_lowering(&mut lcs, app);
    apply_boolean_lowering(&mut lcs, app);
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
    let assocs = crate::lower::scope_chain::build_assoc_registry(&app.models);
    for lc in lcs.iter_mut() {
        // Models gain their scope class methods (already chain-normalized).
        let is_model = app.models.iter().any(|m| m.name == lc.name);
        if let Some(model) = app.models.iter().find(|m| m.name == lc.name) {
            crate::lower::model_to_library::push_scope_methods(
                &mut lc.methods,
                model,
                &scopes,
                &models,
                &assocs,
            );
        }
        // Every method body: normalize scope chains (call-site form).
        // Scope-free bodies still need the rewrite when they start a
        // query chain on a model constant — the arel inline pass bails
        // on dynamic-value where-hashes, and those chains only run
        // against a seeded Relation. A model's own CLASS methods
        // additionally seed bare implicit-self roots (`where(key: key)`
        // in `Keystore.value_for`), signalled via `class_self`.
        for m in &mut lc.methods {
            let class_self = (is_model && m.receiver == MethodReceiver::Class)
                .then(|| lc.name.clone());
            if crate::lower::scope_chain::mentions_scope(&m.body, &names)
                || crate::lower::scope_chain::mentions_model_chain_start(&m.body, &models)
                || (class_self.is_some()
                    && crate::lower::scope_chain::mentions_bare_chain_start(&m.body))
            {
                crate::lower::scope_chain::rewrite_call_site(
                    &mut m.body,
                    &scopes,
                    &models,
                    &assocs,
                    class_self.as_ref(),
                );
            }
        }
    }
}

/// Ruby-family pre-emit pass: correct `has_many :through` readers. The
/// shared lowering synthesizes EVERY has_many reader as a direct
/// foreign-key query (`Tag.where(story_id: @id)`) — wrong for `through:`,
/// where the foreign key lives on the join table. Rebuild those readers
/// as a Relation join through the intermediate:
///
///   def tags
///     return @tags_cache if @tags_loaded
///     ActiveRecord::Relation.new(Tag)
///       .joins("INNER JOIN taggings ON taggings.tag_id = tags.id")
///       .where("taggings.story_id = ?", @id)
///   end
///
/// The through-model's `belongs_to` whose target matches the assoc's
/// target supplies the source foreign key (works for `source:` renames —
/// `upvoted_stories, through: :votes, source: :story` finds
/// `Vote.belongs_to :story`). Unresolvable shapes (through-of-through,
/// missing models) are left on the shared reader rather than guessed.
/// KNOWN GAP: association scope-lambdas (`-> { order(...) }`, the
/// upvoted vote-conditions) are dropped at ingest, so row order/filter
/// can diverge from Rails until the lambda lands in the IR.
pub(crate) fn apply_through_assoc_lowering(lcs: &mut [LibraryClass], app: &App) {
    use crate::dialect::Association;
    use crate::naming::pluralize_snake;

    for lc in lcs.iter_mut() {
        let Some(model) = app.models.iter().find(|m| m.name == lc.name) else { continue };
        for assoc in model.associations() {
            let Association::HasMany { name, target, through: Some(thr_name), .. } = assoc
            else {
                continue;
            };
            // The through association on the owner (`:votes`, `:taggings`).
            let Some(Association::HasMany {
                target: thr_target, foreign_key: thr_fk, ..
            }) = model.associations().find(
                |a| matches!(a, Association::HasMany { name, .. } if name == thr_name),
            )
            else {
                continue;
            };
            // The source belongs_to on the through model (`Vote.belongs_to
            // :story`) — matched by target class, so `source:` renames
            // resolve without a name convention.
            let Some(thr_model) = app.models.iter().find(|m| &m.name == thr_target) else {
                continue;
            };
            let Some(Association::BelongsTo { foreign_key: src_fk, .. }) =
                thr_model.associations().find(|a| {
                    matches!(a, Association::BelongsTo { target: t, .. } if t == target)
                })
            else {
                continue;
            };

            let thr_table = pluralize_snake(thr_target.0.as_str());
            let target_table = pluralize_snake(target.0.as_str());
            let join_sql = format!(
                "INNER JOIN {thr_table} ON {thr_table}.{src_fk} = {target_table}.id"
            );
            let where_sql = format!("{thr_table}.{thr_fk} = ?");

            let Some(m) =
                lc.methods.iter_mut().find(|m| {
                    m.name == *name && m.receiver == crate::dialect::MethodReceiver::Instance
                })
            else {
                continue;
            };
            m.body = through_reader_body(name, target, &join_sql, &where_sql);
        }
    }
}

/// Ruby-family pre-emit pass: belongs_to writers. Rails' `belongs_to
/// :story` provides `comment.story = story_obj` alongside the reader;
/// the shared lowering synthesizes only the reader. The writer stores
/// the foreign key, mirroring the reader's `@fk == 0` nil sentinel
/// (fk columns are non-nullable Int in the schema typing):
///
///   def story=(value)
///     if value.nil?
///       @story_id = 0
///     else
///       @story_id = value.id
///     end
///   end
///
/// No object cache: the reader re-queries by fk, which matches its
/// existing shape (assigning an UNSAVED record then reading the
/// association back is the one Rails behavior this doesn't cover).
/// Skips names the app already defines (custom writers win).
pub(crate) fn apply_belongs_to_writer_lowering(lcs: &mut [LibraryClass], app: &App) {
    use crate::dialect::Association;

    for lc in lcs.iter_mut() {
        let Some(model) = app.models.iter().find(|m| m.name == lc.name) else { continue };
        for assoc in model.associations() {
            let Association::BelongsTo { name, foreign_key, .. } = assoc else { continue };
            let value = || sp_expr(ExprNode::Var { id: VarId(0), name: Symbol::from("value") });
            let fk_assign = |v: Expr| {
                sp_expr(ExprNode::Assign {
                    target: LValue::Ivar { name: foreign_key.clone() },
                    value: v,
                })
            };
            let body = sp_expr(ExprNode::If {
                cond: sp_expr(ExprNode::Send {
                    recv: Some(value()),
                    method: Symbol::from("nil?"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                }),
                then_branch: fk_assign(sp_expr(ExprNode::Lit {
                    value: crate::expr::Literal::Int { value: 0 },
                })),
                else_branch: fk_assign(sp_expr(ExprNode::Send {
                    recv: Some(value()),
                    method: Symbol::from("id"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                })),
            });
            push_instance_method_unless_defined(
                lc,
                Symbol::from(format!("{}=", name.as_str())),
                vec![Param::positional(Symbol::from("value"))],
                body,
                AccessorKind::Method,
                true,
            );
        }
    }
}

/// `return @<name>_cache if @<name>_loaded` + the joined Relation chain
/// (see `apply_through_assoc_lowering`). Guard shape mirrors the shared
/// `synth_has_many_reader` so `_preload_<name>` keeps working.
fn through_reader_body(
    name: &Symbol,
    target: &ClassId,
    join_sql: &str,
    where_sql: &str,
) -> Expr {
    let span = Span::synthetic;
    let guard = Expr::new(
        span(),
        ExprNode::If {
            cond: Expr::new(
                span(),
                ExprNode::Ivar { name: Symbol::from(format!("{}_loaded", name.as_str())) },
            ),
            then_branch: Expr::new(
                span(),
                ExprNode::Return {
                    value: Expr::new(
                        span(),
                        ExprNode::Ivar {
                            name: Symbol::from(format!("{}_cache", name.as_str())),
                        },
                    ),
                },
            ),
            else_branch: Expr::new(span(), ExprNode::Lit { value: crate::expr::Literal::Nil }),
        },
    );

    let target_const = Expr::new(
        span(),
        ExprNode::Const {
            path: target.0.as_str().split("::").map(Symbol::from).collect(),
        },
    );
    let seed = Expr::new(
        span(),
        ExprNode::Send {
            recv: Some(Expr::new(
                span(),
                ExprNode::Const {
                    path: vec![Symbol::from("ActiveRecord"), Symbol::from("Relation")],
                },
            )),
            method: Symbol::from("new"),
            args: vec![target_const],
            block: None,
            parenthesized: true,
        },
    );
    let joined = Expr::new(
        span(),
        ExprNode::Send {
            recv: Some(seed),
            method: Symbol::from("joins"),
            args: vec![Expr::new(
                span(),
                ExprNode::Lit {
                    value: crate::expr::Literal::Str { value: join_sql.to_string() },
                },
            )],
            block: None,
            parenthesized: true,
        },
    );
    let chain = Expr::new(
        span(),
        ExprNode::Send {
            recv: Some(joined),
            method: Symbol::from("where"),
            args: vec![
                Expr::new(
                    span(),
                    ExprNode::Lit {
                        value: crate::expr::Literal::Str { value: where_sql.to_string() },
                    },
                ),
                Expr::new(span(), ExprNode::Ivar { name: Symbol::from("id") }),
            ],
            block: None,
            parenthesized: true,
        },
    );

    Expr::new(span(), ExprNode::Seq { exprs: vec![guard, chain] })
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
    // Generated route-helper names (`active_path`, `story_path`, …) —
    // bare calls to these in layout/helper bodies resolve to the
    // generated `RouteHelpers` module. (The view walker rewrites route
    // helpers in the URL positions it classifies; bare calls nested in
    // unclassified expressions fall through to this pass.)
    let route_helpers: std::collections::HashSet<Symbol> =
        crate::lower::lower_routes_to_library_functions(app)
            .into_iter()
            .map(|f| f.name)
            .collect();
    for lc in lcs.iter_mut() {
        let is_helper_module = helper_modules.contains(&lc.name);
        // Helper and view module functions have no controller context —
        // a bare `request` read there resolves to the per-dispatch
        // `ActionController::Current.request` (controllers keep their
        // own `request` accessor and are left alone).
        let rewrite_request =
            is_helper_module || lc.name.0.as_str().starts_with("Views::");
        for m in &mut lc.methods {
            // A helper module's own methods become module-functions so the
            // rewritten `Module.method` call has a real target — Rails mixed
            // them into a view instance, but the emitted views are module
            // functions with no instance to receive them.
            if is_helper_module && m.receiver == MethodReceiver::Instance {
                m.receiver = MethodReceiver::Class;
            }
            rewrite_helper_calls(
                &mut m.body,
                &app.helper_method_index,
                &route_helpers,
                rewrite_request,
            );
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
            | "path_to_javascript"
            | "javascript_path"
            | "javascript_include_tag"
            | "number_with_precision"
            | "content_tag"
            | "time_ago_in_words"
            | "distance_of_time_in_words"
            | "raw"
            | "link_to"
            | "content_for?"
            | "capture"
            | "concat"
    )
}

/// Is this a call on `ActionView::ViewHelpers` (or a bare `ViewHelpers`
/// const)? Shared by the post-rewrite transforms below.
fn is_view_helpers_const(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Const { path }
        if path.last().map(|s| s.as_str() == "ViewHelpers").unwrap_or(false))
}

/// Strip one trailing `.to_s` (the view walker's `coerce_to_s` wrap) so
/// the safe-call check sees the helper call itself.
fn strip_to_s(e: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return r;
        }
    }
    e
}

/// Calls whose result Rails treats as an html_safe buffer: tag-producing
/// framework helpers, `raw`, and app-helper module functions (which
/// compose those). The view walker's default `html_escape(<call>.to_s)`
/// wrap must NOT apply to these — escaping a safe buffer ships literal
/// `&lt;img&gt;` markup. Plain-string helpers (truncate,
/// time_ago_in_words) stay wrapped: their escape is Rails-correct.
/// Treating every app-helper as safe is a simplification (Rails escapes
/// an app helper that returns a plain string); the corpus' helpers all
/// return tag-helper compositions, and per-method safety inference can
/// refine this when a counterexample shows up.
fn is_html_safe_call(e: &Expr, index: &HashMap<Symbol, ClassId>) -> bool {
    let ExprNode::Send { recv: Some(r), method, .. } = &*e.node else {
        return false;
    };
    let ExprNode::Const { path } = &*r.node else {
        return false;
    };
    let joined =
        path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
    if joined.ends_with("ViewHelpers") {
        return matches!(
            method.as_str(),
            "raw" | "link_to" | "link_to_raw" | "button_to" | "image_tag" | "content_tag"
                | "javascript_include_tag"
        );
    }
    index.values().any(|cid| cid.0.as_str() == joined)
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
fn rewrite_helper_calls(
    expr: &mut Expr,
    index: &HashMap<Symbol, ClassId>,
    route_helpers: &std::collections::HashSet<Symbol>,
    rewrite_request: bool,
) {
    expr.node.for_each_child_mut(&mut |c| {
        rewrite_helper_calls(c, index, route_helpers, rewrite_request)
    });

    // Bare `request` in a helper/view module body → the per-dispatch
    // `ActionController::Current.request` (module functions have no
    // controller to delegate to).
    if rewrite_request {
        if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node {
            if method.as_str() == "request" && args.is_empty() {
                let span = expr.span;
                *expr.node = ExprNode::Send {
                    recv: Some(Expr::new(
                        span,
                        ExprNode::Const {
                            path: vec![
                                Symbol::from("ActionController"),
                                Symbol::from("Current"),
                            ],
                        },
                    )),
                    method: Symbol::from("request"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                };
                return;
            }
        }
    }

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
            } else if route_helpers.contains(method) {
                Some(vec![Symbol::from("RouteHelpers")])
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(path) = path {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { method, mut args, block, .. } = node else { unreachable!() };
        // `link_to(37, url)` — Rails stringifies the text arg; the runtime
        // link_to is deliberately monomorphic (String text), so coercion
        // belongs here at the call boundary. Literal strings stay bare.
        if matches!(method.as_str(), "link_to" | "link_to_raw") {
            if let Some(text) = args.first_mut() {
                if !matches!(
                    &*text.node,
                    ExprNode::Lit { value: crate::expr::Literal::Str { .. } }
                ) {
                    let inner = std::mem::replace(
                        &mut *text.node,
                        ExprNode::Seq { exprs: vec![] },
                    );
                    *text.node = ExprNode::Send {
                        recv: Some(Expr::new(text.span, inner)),
                        method: Symbol::from("to_s"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    };
                }
            }
        }
        *expr.node = ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path })),
            method,
            args,
            block,
            parenthesized: true,
        };
    }

    // `RouteHelpers.<x>_path(format: :rss)` → `RouteHelpers.<x>_path +
    // ".rss"`. Rails path helpers accept `format:` whether or not the
    // route spells `(.:format)`; the IR has no keyword params, so the
    // suffix moves to the call site — arity-independent, and helpers
    // without a format slot stay format-capable.
    let format_suffix = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str().ends_with("_path")
                && matches!(&*r.node, ExprNode::Const { path }
                    if path.last().map(|s| s.as_str() == "RouteHelpers").unwrap_or(false)) =>
        {
            match args.last().map(|a| &*a.node) {
                Some(ExprNode::Hash { entries, kwargs: true }) if entries.len() == 1 => {
                    match &*entries[0].0.node {
                        ExprNode::Lit { value: crate::expr::Literal::Sym { value } }
                            if value.as_str() == "format" =>
                        {
                            Some(entries[0].1.clone())
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        _ => None,
    };
    if let Some(fmt) = format_suffix {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { recv, method, mut args, block, parenthesized } = node else {
            unreachable!()
        };
        args.pop();
        let call = Expr::new(span, ExprNode::Send { recv, method, args, block, parenthesized });
        let suffix = Expr::new(
            span,
            ExprNode::StringInterp {
                parts: vec![
                    crate::expr::InterpPart::Text { value: ".".to_string() },
                    crate::expr::InterpPart::Expr { expr: fmt },
                ],
            },
        );
        *expr.node = ExprNode::Send {
            recv: Some(call),
            method: Symbol::from("+"),
            args: vec![suffix],
            block: None,
            parenthesized: false,
        };
        return;
    }

    // `link_to(raw(x), …)` → `link_to_raw(x, …)`: Rails skips the label
    // escape for a safe buffer; with no safe-buffer type the exemption is
    // decided here. Children were rewritten first (and the bare-call
    // rewrite above may have just fired), so both calls are already in
    // their ViewHelpers.* form.
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &mut *expr.node {
        if method.as_str() == "link_to" && is_view_helpers_const(r) && !args.is_empty() {
            let raw_inner = match &*args[0].node {
                ExprNode::Send { recv: Some(r2), method: m2, args: a2, .. }
                    if m2.as_str() == "raw"
                        && a2.len() == 1
                        && is_view_helpers_const(r2) =>
                {
                    Some(a2[0].clone())
                }
                _ => None,
            };
            if let Some(inner) = raw_inner {
                *method = Symbol::from("link_to_raw");
                args[0] = inner;
            }
        }
    }

    // Unwrap the view walker's default `html_escape(<call>.to_s)` when
    // the call is html_safe (see is_html_safe_call) — Rails doesn't
    // escape safe buffers, and escaping them ships literal &lt;img&gt;.
    let unwrap: Option<Expr> = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "html_escape"
                && args.len() == 1
                && is_view_helpers_const(r)
                && is_html_safe_call(strip_to_s(&args[0]), index) =>
        {
            Some(args[0].clone())
        }
        _ => None,
    };
    if let Some(inner) = unwrap {
        *expr = inner;
    }
}

/// Ruby-emit-path pass: wrap each action's html render in the layout
/// call — `render(Views::X.y(...))` → `render(Views::Layouts.application(
/// Views::X.y(...), @<ivar>…, @flash…))`. Lives here (not the shared
/// controller lowering) because the wrap shape and the CRuby dispatch
/// contract move together: the overlay main.rb ships `controller.body`
/// verbatim, while other targets' dispatchers still wrap body-only.
/// The controller render seam is where a layout's ivar reads (@user,
/// @title) are statically in scope — the generic dispatch had no way to
/// pass them. Skipped renders: jbuilder json (`*_json` view call or a
/// `content_type:` kwarg), non-view renders (`render html:`/`plain:` —
/// Rails skips the layout for those too), and an already-wrapped
/// Layouts call (idempotence). No-op when the app has no
/// layouts/application view.
pub(crate) fn apply_layout_lowering(lcs: &mut [LibraryClass], app: &App) {
    // Cheap probe: no layouts/application view → nothing to wrap.
    let probe = Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Lit { value: crate::expr::Literal::Nil },
    );
    if crate::lower::view_to_library::layout_wrap_expr(app, probe).is_none() {
        return;
    }
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            rewrite_layout_wrap(&mut m.body, app);
        }
    }
}

fn rewrite_layout_wrap(expr: &mut Expr, app: &App) {
    expr.node.for_each_child_mut(&mut |c| rewrite_layout_wrap(c, app));
    // Post-lowering action bodies carry `render` as a SelfRef-receiver
    // Send (`self.render(...)` shape); accept the bare form too.
    let ExprNode::Send { recv, method, args, .. } = &mut *expr.node else {
        return;
    };
    let self_recv = match recv {
        None => true,
        Some(r) => matches!(&*r.node, ExprNode::SelfRef),
    };
    if !self_recv || method.as_str() != "render" || args.is_empty() {
        return;
    }
    // Extract-and-strip any `layout:` kwarg first: the shared lowering
    // keeps it as the wrap marker for body renders (`render html: X,
    // layout: "application"`), and the runtime `render(body, status:,
    // content_type:, location:)` doesn't accept it, so it must never
    // survive to the call.
    let mut layout_kwarg: Option<Expr> = None;
    for a in args.iter_mut().skip(1) {
        if let ExprNode::Hash { entries, .. } = &mut *a.node {
            entries.retain(|(k, v)| {
                let is_layout = matches!(
                    &*k.node,
                    ExprNode::Lit { value: crate::expr::Literal::Sym { value } }
                        if value.as_str() == "layout"
                );
                if is_layout {
                    layout_kwarg = Some(v.clone());
                }
                !is_layout
            });
        }
    }
    args.retain(|a| !matches!(&*a.node, ExprNode::Hash { entries, .. } if entries.is_empty()));
    if args.is_empty() {
        return;
    }
    // An explicit `layout: "application"` / `layout: true` wraps a body
    // render (non-Views literal html) the way Rails does. Other layout
    // names are left unwrapped — honest residue, only `application`
    // exists as an emitted layout.
    let layout_requested = layout_kwarg.as_ref().is_some_and(|v| match &*v.node {
        ExprNode::Lit { value: crate::expr::Literal::Str { value } } => value == "application",
        ExprNode::Lit { value: crate::expr::Literal::Sym { value } } => {
            value.as_str() == "application"
        }
        ExprNode::Lit { value: crate::expr::Literal::Bool { value } } => *value,
        _ => false,
    });
    // Trailing kwargs are fine (`status: :unprocessable_entity` renders
    // WITH layout in Rails) — except the jbuilder branch's
    // `content_type:`, which marks a non-html response.
    let has_content_type = args.iter().skip(1).any(|a| match &*a.node {
        ExprNode::Hash { entries, .. } => entries.iter().any(|(k, _)| {
            matches!(&*k.node, ExprNode::Lit { value: crate::expr::Literal::Sym { value } }
                if value.as_str() == "content_type")
        }),
        _ => false,
    });
    if has_content_type {
        return;
    }
    let wrappable = match &*args[0].node {
        ExprNode::Send { recv: Some(r), method: vm, .. } => {
            !vm.as_str().ends_with("_json")
                && matches!(&*r.node, ExprNode::Const { path }
                    if path.len() == 2
                        && path[0].as_str() == "Views"
                        && path[1].as_str() != "Layouts")
        }
        _ => layout_requested,
    };
    if !wrappable {
        return;
    }
    let inner = args[0].clone();
    if let Some(wrapped) = crate::lower::view_to_library::layout_wrap_expr(app, inner) {
        args[0] = wrapped;
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
        // Same-dir siblings are required too (not just cross-dir):
        // a model referenced ONLY from another model's scope body
        // (story.rb's `not_hidden_by` → HiddenStory) is invisible to
        // the controller require chain that was assumed to load it —
        // nothing else loads the file by request time. The require
        // cycles this creates between models (story ↔ user) are
        // benign: `require_relative` returns early on a file already
        // mid-load, and model-to-model references live inside method
        // bodies, resolved at request time when both classes exist.
        if let Some(anchor) = require_path_for_body_const(path, app, name) {
            if anchor != self_anchor {
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

// ── has_secure_password lowering ─────────────────────────────────────

/// Ruby-family pre-emit pass: synthesize the methods
/// `has_secure_password` provides — `authenticate` (returning the
/// record or `false`) plus the plaintext virtual-attribute accessors
/// (`password`/`password=`/`password_confirmation`/…, or a custom
/// attribute's spellings) — onto models that declare the marker.
///
/// Lives on the Ruby emit path because the bodies call the bcrypt gem
/// (`BCrypt::Password`), which only the CRuby/JRuby tree loads (a
/// guarded require in the overlay main.rb); shared `lower/` stays
/// target-agnostic per the scope-lowering rule. The analyze layer
/// already *types* this surface (`register_has_secure_password` in
/// analyze/mod.rs); this pass supplies the runtime bodies that type
/// registry promised. Strict no-op for marker-free apps (the blog).
pub(crate) fn apply_secure_password_lowering(lcs: &mut [LibraryClass], app: &App) {
    for model in &app.models {
        let Some(attr) = secure_password_attr(&model.body) else {
            continue;
        };
        let Some(lc) = lcs.iter_mut().find(|lc| lc.name == model.name) else {
            continue;
        };
        let digest = Symbol::from(format!("{}_digest", attr.as_str()));
        let confirmation = Symbol::from(format!("{}_confirmation", attr.as_str()));
        // Rails names the authenticator after the attribute, except the
        // default `password` which gets the bare `authenticate`.
        let auth_name = if attr.as_str() == "password" {
            Symbol::from("authenticate")
        } else {
            Symbol::from(format!("authenticate_{}", attr.as_str()))
        };
        push_instance_method_unless_defined(
            lc,
            auth_name,
            vec![Param::positional(Symbol::from("unencrypted_password"))],
            authenticate_body(&digest),
            AccessorKind::Method,
            false,
        );
        push_instance_method_unless_defined(
            lc,
            attr.clone(),
            Vec::new(),
            ivar_read(&attr),
            AccessorKind::AttributeReader,
            false,
        );
        push_instance_method_unless_defined(
            lc,
            Symbol::from(format!("{}=", attr.as_str())),
            vec![Param::positional(Symbol::from("unencrypted_password"))],
            plaintext_writer_body(&attr, &digest),
            AccessorKind::Method,
            true,
        );
        push_instance_method_unless_defined(
            lc,
            confirmation.clone(),
            Vec::new(),
            ivar_read(&confirmation),
            AccessorKind::AttributeReader,
            false,
        );
        push_instance_method_unless_defined(
            lc,
            Symbol::from(format!("{}=", confirmation.as_str())),
            vec![Param::positional(Symbol::from("value"))],
            plain_ivar_assign(&confirmation, "value"),
            AccessorKind::AttributeWriter,
            true,
        );
    }
}

/// The secure-password attribute name when the model body declares
/// `has_secure_password` (first positional symbol, default
/// `password`), else None. Mirrors analyze's registration scan.
fn secure_password_attr(body: &[crate::dialect::ModelBodyItem]) -> Option<Symbol> {
    for item in body {
        let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item else {
            continue;
        };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else {
            continue;
        };
        if method.as_str() != "has_secure_password" {
            continue;
        }
        let attr = args
            .iter()
            .find_map(|a| match &*a.node {
                ExprNode::Lit { value: crate::expr::Literal::Sym { value } } => {
                    Some(value.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| Symbol::from("password"));
        return Some(attr);
    }
    None
}

fn push_instance_method_unless_defined(
    lc: &mut LibraryClass,
    name: Symbol,
    params: Vec<Param>,
    body: Expr,
    kind: AccessorKind,
    mutates_self: bool,
) {
    if lc
        .methods
        .iter()
        .any(|m| m.receiver == MethodReceiver::Instance && m.name == name)
    {
        return;
    }
    lc.methods.push(crate::dialect::MethodDef {
        name,
        receiver: MethodReceiver::Instance,
        params,
        body,
        signature: None,
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(lc.name.0.clone()),
        kind,
        is_async: false,
        mutates_self,
        block_param: None,
    });
}

fn sp_expr(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

fn ivar_read(name: &Symbol) -> Expr {
    sp_expr(ExprNode::Ivar { name: name.clone() })
}

fn plain_ivar_assign(name: &Symbol, param: &str) -> Expr {
    sp_expr(ExprNode::Assign {
        target: LValue::Ivar { name: name.clone() },
        value: sp_expr(ExprNode::Var { id: VarId(0), name: Symbol::from(param) }),
    })
}

/// `BCrypt::Password.new(@<digest>) == unencrypted_password ? self : false`.
fn authenticate_body(digest: &Symbol) -> Expr {
    let wrapped = sp_expr(ExprNode::Send {
        recv: Some(sp_expr(ExprNode::Const {
            path: vec![Symbol::from("BCrypt"), Symbol::from("Password")],
        })),
        method: Symbol::from("new"),
        args: vec![ivar_read(digest)],
        block: None,
        parenthesized: true,
    });
    let cmp = sp_expr(ExprNode::Send {
        recv: Some(wrapped),
        method: Symbol::from("=="),
        args: vec![sp_expr(ExprNode::Var {
            id: VarId(0),
            name: Symbol::from("unencrypted_password"),
        })],
        block: None,
        parenthesized: false,
    });
    sp_expr(ExprNode::If {
        cond: cmp,
        then_branch: sp_expr(ExprNode::SelfRef),
        else_branch: sp_expr(ExprNode::Lit {
            value: crate::expr::Literal::Bool { value: false },
        }),
    })
}

/// The plaintext writer Rails' macro provides:
///   `@<attr> = v; @<attr>_digest = BCrypt::Password.create(v).to_s unless v.nil?`
/// (`.to_s` because BCrypt::Password subclasses String but the digest
/// column stores plain text). Nil skips digest generation, mirroring
/// Rails' blank-guard closely enough for the login/rehash paths.
fn plaintext_writer_body(attr: &Symbol, digest: &Symbol) -> Expr {
    let value_var = || {
        sp_expr(ExprNode::Var { id: VarId(0), name: Symbol::from("unencrypted_password") })
    };
    let store_plain = sp_expr(ExprNode::Assign {
        target: LValue::Ivar { name: attr.clone() },
        value: value_var(),
    });
    let create = sp_expr(ExprNode::Send {
        recv: Some(sp_expr(ExprNode::Const {
            path: vec![Symbol::from("BCrypt"), Symbol::from("Password")],
        })),
        method: Symbol::from("create"),
        args: vec![value_var()],
        block: None,
        parenthesized: true,
    });
    let digest_str = sp_expr(ExprNode::Send {
        recv: Some(create),
        method: Symbol::from("to_s"),
        args: Vec::new(),
        block: None,
        parenthesized: false,
    });
    let guarded_digest = sp_expr(ExprNode::If {
        cond: sp_expr(ExprNode::Send {
            recv: Some(value_var()),
            method: Symbol::from("nil?"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        }),
        then_branch: sp_expr(ExprNode::Lit { value: crate::expr::Literal::Nil }),
        else_branch: sp_expr(ExprNode::Assign {
            target: LValue::Ivar { name: digest.clone() },
            value: digest_str,
        }),
    });
    sp_expr(ExprNode::Seq { exprs: vec![store_plain, guarded_digest] })
}

// Request-params key normalization moved to the Ruby expr emitter's
// type-directed index hook (`emit::ruby::expr`): a symbol/dynamic key on
// a statically string-keyed hash (`Hash[String, _]`) is coerced to a
// string at the single `[]` emit chokepoint. That gates on the receiver
// *type* rather than a `params` name heuristic, so it covers views and
// helpers (params flows there too) and never touches a genuine
// symbol-keyed `Hash[Symbol, _]` like `StoryRepository#@params`.

// ── typed_store lowering ─────────────────────────────────────────────

/// Ruby-family pre-emit pass: lower `typed_store :settings do |s|
/// s.string :totp_secret … end` (the typed_store gem — virtual
/// attributes YAML-serialized into a TEXT column) to per-attribute
/// reader/writer methods routing through the overlay `TypedStore`
/// module. Boolean attributes additionally get the Rails `<name>?`
/// predicate spelling. Lives on the Ruby emit path (the bodies call a
/// CRuby-overlay module); strict no-op for apps without the DSL.
pub(crate) fn apply_typed_store_lowering(lcs: &mut [LibraryClass], app: &App) {
    use crate::lower::typed_store::typed_store_decls;
    for model in &app.models {
        let stores = typed_store_decls(&model.body);
        if stores.is_empty() {
            continue;
        }
        let Some(lc) = lcs.iter_mut().find(|lc| lc.name == model.name) else {
            continue;
        };
        for (col, attrs) in &stores {
            for a in attrs {
                push_instance_method_unless_defined(
                    lc,
                    a.name.clone(),
                    Vec::new(),
                    typed_store_read_body(col, a),
                    AccessorKind::Method,
                    false,
                );
                if a.is_bool {
                    push_instance_method_unless_defined(
                        lc,
                        Symbol::from(format!("{}?", a.name.as_str())),
                        Vec::new(),
                        typed_store_read_body(col, a),
                        AccessorKind::Method,
                        false,
                    );
                }
                push_instance_method_unless_defined(
                    lc,
                    Symbol::from(format!("{}=", a.name.as_str())),
                    vec![Param::positional(Symbol::from("value"))],
                    typed_store_write_body(col, a),
                    AccessorKind::Method,
                    true,
                );
            }
        }
    }
}

/// `TypedStore.read(@<col>, "<name>", <default|nil>)`.
use crate::lower::typed_store::TypedStoreAttr;

fn typed_store_read_body(col: &Symbol, a: &TypedStoreAttr) -> Expr {
    let default = a.default.clone().unwrap_or_else(|| {
        sp_expr(ExprNode::Lit { value: crate::expr::Literal::Nil })
    });
    sp_expr(ExprNode::Send {
        recv: Some(sp_expr(ExprNode::Const { path: vec![Symbol::from("TypedStore")] })),
        method: Symbol::from("read"),
        args: vec![
            ivar_read(col),
            sp_expr(ExprNode::Lit {
                value: crate::expr::Literal::Str { value: a.name.as_str().to_string() },
            }),
            default,
        ],
        block: None,
        parenthesized: true,
    })
}

/// `@<col> = TypedStore.write(@<col>, "<name>", value)`.
fn typed_store_write_body(col: &Symbol, a: &TypedStoreAttr) -> Expr {
    let write = sp_expr(ExprNode::Send {
        recv: Some(sp_expr(ExprNode::Const { path: vec![Symbol::from("TypedStore")] })),
        method: Symbol::from("write"),
        args: vec![
            ivar_read(col),
            sp_expr(ExprNode::Lit {
                value: crate::expr::Literal::Str { value: a.name.as_str().to_string() },
            }),
            sp_expr(ExprNode::Var { id: VarId(0), name: Symbol::from("value") }),
        ],
        block: None,
        parenthesized: true,
    });
    sp_expr(ExprNode::Assign {
        target: LValue::Ivar { name: col.clone() },
        value: write,
    })
}

// ── boolean-column cast lowering ─────────────────────────────────────

/// Ruby-family pre-emit pass: boolean-column readers and `<col>?`
/// predicates cast the stored value instead of returning it raw. The
/// CRuby sqlite adapter hydrates boolean columns as the Integers
/// SQLite stores (0/1) — and `0` is TRUTHY in Ruby, so a plain `@col`
/// read makes every `user.is_admin?` guard pass for non-admins.
/// Rewritten body: `@col == true || @col == 1` (handles both a
/// DB-hydrated Integer and an app-assigned true/false; nil/0/false →
/// false). Strict targets hydrate native booleans and keep the shared
/// synthesized shape. Only plain `@col`-read bodies are rewritten
/// (idempotent; custom bodies win).
pub(crate) fn apply_boolean_lowering(lcs: &mut [LibraryClass], app: &App) {
    for model in &app.models {
        let Some(table) = app.schema.tables.get(&model.table.0) else {
            continue;
        };
        let bool_cols: BTreeSet<Symbol> = table
            .columns
            .iter()
            .filter(|c| matches!(c.col_type, crate::schema::ColumnType::Boolean))
            .map(|c| c.name.clone())
            .collect();
        if bool_cols.is_empty() {
            continue;
        }
        let Some(lc) = lcs.iter_mut().find(|lc| lc.name == model.name) else {
            continue;
        };
        for m in &mut lc.methods {
            if m.receiver != MethodReceiver::Instance {
                continue;
            }
            let col = Symbol::from(m.name.as_str().trim_end_matches('?'));
            if !bool_cols.contains(&col) {
                continue;
            }
            if is_plain_ivar_read(&m.body, &col) {
                m.body = boolean_cast_body(&col);
            }
        }
    }
}

/// `@col == true || @col == 1`.
fn boolean_cast_body(col: &Symbol) -> Expr {
    let eq = |rhs: Expr| {
        sp_expr(ExprNode::Send {
            recv: Some(ivar_read(col)),
            method: Symbol::from("=="),
            args: vec![rhs],
            block: None,
            parenthesized: false,
        })
    };
    sp_expr(ExprNode::BoolOp {
        op: crate::expr::BoolOpKind::Or,
        surface: Default::default(),
        left: eq(sp_expr(ExprNode::Lit {
            value: crate::expr::Literal::Bool { value: true },
        })),
        right: eq(sp_expr(ExprNode::Lit {
            value: crate::expr::Literal::Int { value: 1 },
        })),
    })
}
