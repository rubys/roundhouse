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
use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::naming::snake_case;
use crate::span::Span;

pub(super) fn emit_library_class_decls(app: &App) -> Vec<EmittedFile> {
    let mut lcs: Vec<LibraryClass> = app.library_classes.clone();
    apply_scope_lowering(&mut lcs, app);
    apply_library_partial_render_lowering(&mut lcs, app);
    apply_helper_lowering(&mut lcs, app);
    // send→case grounding, update-kwargs inlining, mailer class-side
    // wrappers, and duration grounding run in the shared post-analyze
    // hook, which covers these library classes (dispatch's plural
    // duration-unit arms arrive already grounded).
    // Transpiled-shape classes carry hand-written accessors that
    // `synth_attr_reader` never sees, so the datetime reader/writer
    // rewrite still runs here for them (Ruby-only). Model-lowered classes
    // get the reader from `synth_attr_reader` (shared, all targets); this
    // re-applies the same reader idempotently and adds the Ruby writer
    // normalize.
    apply_datetime_lowering(&mut lcs, app);
    apply_boolean_lowering(&mut lcs, app);
    apply_hydration_nil_lowering(&mut lcs, app);
    apply_nilsafe_empty_lowering(&mut lcs);
    lcs.iter()
        .flat_map(|lc| {
            // `underscore`, not `snake_case`: a namespaced reopen
            // (lobsters' `ActiveRecord::Base.q`, `Net::HTTP`,
            // `ShortId::CandidateId`) nests as `active_record/base.rb` —
            // a literal `::` in the filename breaks the emitted
            // Makefile's dependency list.
            let file_stem = crate::naming::underscore(lc.name.0.as_str());
            let out_path = PathBuf::from(format!("app/models/{file_stem}.rb"));
            emit_library_class_pair(lc, app, out_path)
        })
        .collect()
}

/// App-defined vendored classes whose bodies drive un-modeled native/
/// stdlib surface (Sponge = Net::HTTP + Resolv + IPAddr + OpenSSL,
/// pending the stdlib spin packages). Spinel AOT prices every method
/// body in the reachable require graph, and these bodies cannot
/// compile without that stdlib — so the scaffold base swaps their
/// emitted files for hand-written raising façades at the SAME emit
/// path, leaving the require graph untouched. The CRuby tree, where
/// the real stdlib exists and the vendored source runs as written,
/// restores the verbatim emit via `restore_extras_facades`. Same
/// raise-loudly contract as runtime/ruby/gem_facades.rb; the real fix
/// (compiling the verbatim bodies) arrives with the stdlib packages.
const EXTRAS_FACADES: &[(&str, &str, &str)] = &[(
    "app/models/sponge",
    include_str!("../../../runtime/spinel/facades/sponge.rb"),
    include_str!("../../../runtime/spinel/facades/sponge.rbs"),
)];

/// Swap façade-fated extras emits (scaffold base: spinel + the trees
/// derived from it). No-op when the app doesn't define the class —
/// the path simply isn't present.
pub(super) fn apply_extras_facades(files: &mut [(String, String)]) {
    for (stem, rb, rbs) in EXTRAS_FACADES {
        for (path, content) in files.iter_mut() {
            if path == &format!("{stem}.rb") {
                *content = (*rb).to_string();
            } else if path == &format!("{stem}.rbs") {
                *content = (*rbs).to_string();
            }
        }
    }
}

/// CRuby: put the verbatim source-shape emit back over the façades —
/// the real net/https / resolv / ipaddr are available there and the
/// vendored bodies run as written. Re-renders the library classes so
/// the restored bytes are exactly what the base would have emitted
/// without the swap.
pub(super) fn restore_extras_facades(files: &mut [(String, String)], app: &App) {
    for ef in emit_library_class_decls(app) {
        let p = ef.path.to_string_lossy().into_owned();
        if EXTRAS_FACADES
            .iter()
            .any(|(stem, _, _)| p == format!("{stem}.rb") || p == format!("{stem}.rbs"))
        {
            for (path, content) in files.iter_mut() {
                if *path == p {
                    content.clone_from(&ef.content);
                }
            }
        }
    }
}

/// Ruby-family pre-emit pass: `render partial:` in a LIBRARY-CLASS body
/// (lobsters' ApplicationHelper#link_post renders a partial from a
/// helper). A helper's render RETURNS the string, so the rewrite is the
/// bare `Views::<Mod>.<stem>(record, closure…, extras…)` call — locals
/// bind by name against the partial's contract, everything else nil
/// (a module body has no controller ivars to thread). Slashed partial
/// names only; a bare name has no module context here.
pub(crate) fn apply_library_partial_render_lowering(lcs: &mut [LibraryClass], app: &App) {
    let contracts = crate::lower::view_to_library::partial_call_contracts(
        &app.views,
        &app.controllers,
        &app.library_classes,
    );
    if contracts.is_empty() {
        return;
    }
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            rewrite_library_partial_render(&mut m.body, &contracts);
        }
    }
}

fn rewrite_library_partial_render(
    expr: &mut Expr,
    contracts: &std::collections::HashMap<
        (String, String),
        crate::lower::view_to_library::PartialCallContract,
    >,
) {
    use crate::expr::Literal;
    expr.node
        .for_each_child_mut(&mut |c| rewrite_library_partial_render(c, contracts));
    let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { return };
    if method.as_str() != "render" && method.as_str() != "render_to_string" {
        return;
    }
    let Some(first) = args.first() else { return };
    let ExprNode::Hash { entries, kwargs: true } = &*first.node else { return };
    let mut partial: Option<String> = None;
    let mut locals: Vec<(Symbol, Expr)> = Vec::new();
    for (k, v) in entries {
        let key = match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str(),
            _ => "",
        };
        match key {
            "partial" => {
                if let ExprNode::Lit { value: Literal::Str { value } } = &*v.node {
                    partial = Some(value.clone());
                }
            }
            "locals" => {
                if let ExprNode::Hash { entries: le, .. } = &*v.node {
                    for (lk, lv) in le {
                        if let ExprNode::Lit { value: Literal::Sym { value } } = &*lk.node {
                            locals.push((value.clone(), lv.clone()));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let Some(pname) = partial else { return };
    let Some((dir, base)) = pname.rsplit_once('/') else { return };
    let module_camel = crate::naming::camelize(&crate::naming::snake_case(dir));
    let stem = base.trim_start_matches('_').to_string();
    let Some(contract) = contracts.get(&(module_camel.clone(), stem.clone())) else { return };
    let span = expr.span;
    let nil = || sp_expr(ExprNode::Lit { value: Literal::Nil });
    let lookup = |name: &str| -> Option<Expr> {
        locals.iter().find(|(k, _)| k.as_str() == name).map(|(_, v)| v.clone())
    };
    let mut view_args: Vec<Expr> = Vec::new();
    view_args.push(lookup(&contract.record).unwrap_or_else(nil));
    for n in &contract.closure {
        view_args.push(lookup(n).unwrap_or_else(nil));
    }
    let bound: Vec<Option<Expr>> = contract.extras.iter().map(|n| lookup(n)).collect();
    if let Some(last) = bound.iter().rposition(|b| b.is_some()) {
        for b in bound.into_iter().take(last + 1) {
            view_args.push(b.unwrap_or_else(nil));
        }
    }
    *expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(sp_expr(ExprNode::Const {
                path: vec![Symbol::from("Views"), Symbol::from(module_camel)],
            })),
            method: crate::lower::view::view_method_name(&stem),
            args: view_args,
            block: None,
            parenthesized: true,
        },
    );
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
    let user_returns = crate::lower::scope_chain::build_user_method_returns(&app.models);
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
        // User-written class methods the registry admitted as
        // relation-taking (`def self.arrange_for_user` with bare
        // `order(...)` roots) get the same treatment push_scope_methods
        // gives declared scopes: a trailing `__rel =
        // ActiveRecord::Relation.new(self)` param, and their bare chain
        // roots threaded through it. Skip-if-last-param-is-__rel makes
        // this idempotent AND distinguishes user methods from the
        // already-synthesized scope methods.
        if is_model {
            let rel_param = Symbol::from("__rel");
            let registered: Vec<Symbol> = scopes
                .get(&lc.name)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            for m in &mut lc.methods {
                if m.receiver == MethodReceiver::Class
                    && registered.contains(&m.name)
                    // Any-position check: push_scope_methods inserts
                    // __rel BEFORE keyword params, so a kwarg-taking
                    // scope's __rel is not last.
                    && !m.params.iter().any(|p| p.as_str() == "__rel")
                {
                    // Insert before keywords for the same reason.
                    let insert_at = m
                        .params
                        .iter()
                        .position(|p| p.keyword)
                        .unwrap_or(m.params.len());
                    m.params.insert(
                        insert_at,
                        crate::dialect::Param::with_default(
                            rel_param.clone(),
                            crate::lower::model_to_library::relation_new_self(),
                        ),
                    );
                    crate::lower::scope_chain::rewrite_scope_body(
                        &mut m.body,
                        &lc.name,
                        &rel_param,
                        &scopes,
                        &models,
                        &assocs,
                    );
                }
            }
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
            // A model's own INSTANCE methods know their self model too —
            // `self.<has_many>.<scope>` there seeds a Relation from the
            // association's foreign key (recent_threads' comment chain).
            let instance_self = (is_model && m.receiver == MethodReceiver::Instance)
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
                    instance_self.as_ref(),
                    &user_returns,
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
            let Association::HasMany {
                name, target, through: Some(thr_name), scope: assoc_scope, ..
            } = assoc
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
            m.body = through_reader_body(name, target, &join_sql, &where_sql, assoc_scope);
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
    assoc_scope: &Option<Expr>,
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

    // Association scope lambda (`-> { where('votes.vote' => 1)... }`) —
    // graft its receiver-less chain onto the seeded relation so the
    // reader filters the way Rails does (without it /upvoted served
    // every joined row).
    let chain = match assoc_scope {
        Some(scope_body) => graft_chain_root(scope_body, chain),
        None => chain,
    };

    Expr::new(span(), ExprNode::Seq { exprs: vec![guard, chain] })
}

/// Replace the receiver-less root of a `where(...).order(...)` chain
/// with `seed`, turning an association-scope lambda body into a call
/// chain on the seeded relation. Non-chain shapes (a Seq, a literal)
/// return the seed untouched — better an unfiltered relation than a
/// mis-grafted one.
fn graft_chain_root(chain: &Expr, seed: Expr) -> Expr {
    match &*chain.node {
        ExprNode::Send { recv: Some(r), method, args, block, parenthesized } => {
            let new_recv = graft_chain_root(r, seed);
            Expr::new(
                chain.span,
                ExprNode::Send {
                    recv: Some(new_recv),
                    method: method.clone(),
                    args: args.clone(),
                    block: block.clone(),
                    parenthesized: *parenthesized,
                },
            )
        }
        ExprNode::Send { recv: None, method, args, block, parenthesized } => Expr::new(
            chain.span,
            ExprNode::Send {
                recv: Some(seed),
                method: method.clone(),
                args: args.clone(),
                block: block.clone(),
                parenthesized: *parenthesized,
            },
        ),
        _ => seed,
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
    // App classes that `include Rails.application.routes.url_helpers`
    // (ingest records the marker as an include of RouteHelpers —
    // lobsters' Routes). Explicit `X.<helper>` call sites anywhere in
    // the tree rewrite through RouteHelpers below. Sourced from
    // `app.library_classes`, not the local `lcs` slice — this pass
    // also runs over the lowered-models stack, whose slice doesn't
    // contain the including class itself.
    let url_helper_classes: std::collections::HashSet<Symbol> = app
        .library_classes
        .iter()
        .filter(|lc| lc.includes.iter().any(|i| i.0.as_str() == "RouteHelpers"))
        .map(|lc| lc.name.0.clone())
        .collect();
    for lc in lcs.iter_mut() {
        // CONTROLLERS in the index provide `helper_method`s to views:
        // only the call-site rewrite applies to them — their methods
        // must NOT flip class-side wholesale (actions are instance
        // methods; the class-side clone for the marked helpers comes
        // from the controller lowering itself).
        let is_helper_module = helper_modules.contains(&lc.name)
            && !app.controllers.iter().any(|c| c.name == lc.name);
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
            // Ground `…url_helpers.<x>_url(record?, host:)` → absolute-URL
            // interpolation BEFORE `rewrite_helper_calls` collapses the
            // `Rails.application.routes.url_helpers` chain to `RouteHelpers`
            // (that collapse is children-first, so it would erase the chain
            // shape this grounding matches on). mod_note's `user_url` lives
            // in a model body, reached by this same pass.
            rewrite_url_helpers_absolute(&mut m.body);
            rewrite_helper_calls(
                &mut m.body,
                &app.helper_method_index,
                &route_helpers,
                &url_helper_classes,
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
            | "image_url"
            | "path_to_javascript"
            | "javascript_path"
            | "javascript_include_tag"
            | "number_with_precision"
            | "number_with_delimiter"
            | "label_tag"
            | "url_for"
            | "submit_tag"
            | "form_tag"
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
                | "javascript_include_tag" | "label_tag" | "submit_tag" | "form_tag"
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
    url_helper_classes: &std::collections::HashSet<Symbol>,
    rewrite_request: bool,
) {
    expr.node.for_each_child_mut(&mut |c| {
        rewrite_helper_calls(c, index, route_helpers, url_helper_classes, rewrite_request)
    });

    // `X.<helper>` where X singleton-includes url_helpers (lobsters'
    // `Routes.user_url reparent_user`): a `<x>_path` retargets to the
    // generated RouteHelpers module; a `<x>_url` whose path sibling is
    // generated re-lands as the bare `<x>_url` form so the absolute-URL
    // grounding a few blocks down claims it in this same visit.
    if let ExprNode::Send { recv: Some(r), method, .. } = &*expr.node {
        if let ExprNode::Const { path } = &*r.node {
            if path.len() == 1 && url_helper_classes.contains(&path[0]) {
                let named_path = route_helpers.contains(method);
                let named_url = method
                    .as_str()
                    .strip_suffix("_url")
                    .is_some_and(|stem| {
                        route_helpers.contains(&Symbol::from(format!("{stem}_path")))
                    });
                if named_path {
                    let ExprNode::Send { recv, .. } = &mut *expr.node else { unreachable!() };
                    *recv = Some(Expr::new(
                        expr.span,
                        ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] },
                    ));
                } else if named_url {
                    let ExprNode::Send { recv, .. } = &mut *expr.node else { unreachable!() };
                    *recv = None;
                }
            }
        }
    }

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

    // Bare `<x>_url` whose `<x>_path` sibling is generated — the
    // absolute variant grounds to protocol + configured domain + the
    // path helper (same convention as `rewrite_url_helpers_absolute`'s
    // host-kwarg form): `"http://#{Rails.application.domain}#{
    // RouteHelpers.<x>_path(args)}"`. Lobsters' hats page links
    // `request_hat_url` bare.
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node {
        if let Some(stem) = method.as_str().strip_suffix("_url") {
            let path_name = Symbol::from(format!("{stem}_path"));
            if route_helpers.contains(&path_name) {
                let span = expr.span;
                let args = args.clone();
                let domain = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(Expr::new(
                            span,
                            ExprNode::Send {
                                recv: Some(Expr::new(
                                    span,
                                    ExprNode::Const { path: vec![Symbol::from("Rails")] },
                                )),
                                method: Symbol::from("application"),
                                args: vec![],
                                block: None,
                                parenthesized: false,
                            },
                        )),
                        method: Symbol::from("domain"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                );
                let path_call = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(Expr::new(
                            span,
                            ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] },
                        )),
                        method: path_name,
                        args,
                        block: None,
                        parenthesized: true,
                    },
                );
                *expr.node = ExprNode::StringInterp {
                    parts: vec![
                        crate::expr::InterpPart::Text { value: "http://".to_string() },
                        crate::expr::InterpPart::Expr { expr: domain },
                        crate::expr::InterpPart::Expr { expr: path_call },
                    ],
                };
                return;
            }
        }
    }

    // Cases 3/4: a bare call resolving to an app or framework helper module.
    let path: Option<Vec<Symbol>> = match &*expr.node {
        ExprNode::Send { recv: None, method, args, .. } => {
            if let Some(module) = index.get(method) {
                Some(module.0.as_str().split("::").map(Symbol::from).collect())
            } else if method.as_str() == "pluralize" && args.len() == 2 {
                // Count-labeling `pluralize(count, word)` in a helper
                // body — the same home the view pipeline's classifier
                // already grounds to (`Inflector.pluralize`, the
                // spinel-blog convention), NOT a second ViewHelpers
                // impl. Two-arg form only: the optional plural-word /
                // locale variants aren't in the runtime's surface, so
                // they stay verbatim rather than mis-bind arity.
                Some(vec![Symbol::from("Inflector")])
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
    // An explicit `layout: false` VETOES the wrap outright — the
    // controller-side partial-render rewrite plants it (Rails renders
    // partials without a layout), and a user's `render …, layout:
    // false` means the same thing.
    if layout_kwarg.as_ref().is_some_and(|v| {
        matches!(&*v.node, ExprNode::Lit { value: crate::expr::Literal::Bool { value: false } })
    }) {
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
    // An explicit `layout: "application"`/`true` wraps WHATEVER the
    // body is — `render html: content.html_safe, layout:
    // "application"` (lobsters /u serves a cached tree string) has a
    // Send body that isn't a Views call, and the Views-shape test
    // alone wrongly skipped it. Without an explicit request, only a
    // non-layout non-json Views call wraps (the implicit html render).
    let wrappable = layout_requested
        || match &*args[0].node {
            ExprNode::Send { recv: Some(r), method: vm, .. } => {
                !vm.as_str().ends_with("_json")
                    && matches!(&*r.node, ExprNode::Const { path }
                        if path.len() == 2
                            && path[0].as_str() == "Views"
                            && path[1].as_str() != "Layouts")
            }
            _ => false,
        };
    if !wrappable {
        return;
    }
    let inner = args[0].clone();
    if let Some(wrapped) = crate::lower::view_to_library::layout_wrap_expr(app, inner) {
        args[0] = wrapped;
    }
}

// `record.update!(k: v, ...)` kwargs inlining moved to the shared
// post-analyze hook (`lower::apply_update_kwargs_inline`) — hook
// bodies arrive here already in writer-assign + save form, with
// unknown-receiver and impure-receiver sites on the residue ledger.

// `errors.add(:field, "msg")` grounding moved to the shared
// post-analyze hook (`lower::apply_errors_add_lowering`) — every hook
// body arrives here already rewritten to the `errors << "Field msg"`
// accumulator shape, with non-self receivers on the residue ledger.

/// Ground `Rails.application.routes.url_helpers.<x>_url(record?, host: H,
/// protocol: P)` → `"#{P}://#{H}#{RouteHelpers.<x>_path(record?)}"`. The
/// routing object graph behind `url_helpers` isn't modeled (and never
/// needs to be for this shape — an absolute URL is protocol + host + the
/// generated path helper). Leading positional args (the record in
/// `user_url(sender, host:)`) and any non-host/protocol kwargs (real
/// path params) are forwarded to the `<x>_path` helper; the record rides
/// whole so its custom `to_param` resolves. Non-matching url_helpers
/// uses are left alone. Applied to the Rails::Application reopen (whose
/// kwargs-only `root_url` is the original occurrence) and mod_note's
/// `user_url(sender, host:)`.
pub(crate) fn rewrite_url_helpers_absolute(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite_url_helpers_absolute);
    let matches = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(uh), method, args, block: None, .. }
            if method.as_str().ends_with("_url")
                && !args.is_empty()
                && matches!(&*args[args.len() - 1].node, ExprNode::Hash { .. })
                && is_url_helpers_chain(uh)
    );
    if !matches {
        return;
    }
    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { method, mut args, .. } = node else { unreachable!() };
    // Split leading positional path args (e.g. the record in
    // `user_url(sender, host:)`) from the trailing kwargs hash.
    let trailing = args.pop().unwrap();
    let positional = args;
    let ExprNode::Hash { entries, .. } = &*trailing.node else { unreachable!() };
    let mut host: Option<Expr> = None;
    let mut protocol: Option<Expr> = None;
    let mut path_kwargs: Vec<(Expr, Expr)> = Vec::new();
    for (k, v) in entries {
        if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
            match value.as_str() {
                "host" => {
                    host = Some(v.clone());
                    continue;
                }
                "protocol" => {
                    protocol = Some(v.clone());
                    continue;
                }
                _ => {}
            }
        }
        // Any non-host/protocol key is a real path parameter — forward it.
        path_kwargs.push((k.clone(), v.clone()));
    }
    // Forward leading positionals plus any surviving path kwargs to the
    // generated `<stem>_path` helper (the record rides whole, so custom
    // `to_param` — User=username — resolves inside the path helper).
    let mut path_args: Vec<Expr> = positional;
    if !path_kwargs.is_empty() {
        path_args.push(Expr::new(
            trailing.span,
            ExprNode::Hash { entries: path_kwargs, kwargs: true },
        ));
    }
    let stem = method.as_str().trim_end_matches("_url");
    let path_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] },
            )),
            method: Symbol::from(format!("{stem}_path")),
            args: path_args,
            block: None,
            parenthesized: true,
        },
    );
    let lit = |s: &str| crate::expr::InterpPart::Text { value: s.to_string() };
    let dyn_part = |e: Expr| crate::expr::InterpPart::Expr { expr: e };
    let mut parts: Vec<crate::expr::InterpPart> = Vec::new();
    if let Some(p) = protocol {
        parts.push(dyn_part(p));
    } else {
        parts.push(lit("http"));
    }
    parts.push(lit("://"));
    if let Some(h) = host {
        parts.push(dyn_part(h));
    }
    parts.push(dyn_part(path_call));
    *expr.node = ExprNode::StringInterp { parts };
    expr.ty = Some(crate::ty::Ty::Str);
}

fn is_url_helpers_chain(e: &Expr) -> bool {
    let ExprNode::Send { recv: Some(routes), method, .. } = &*e.node else { return false };
    if method.as_str() != "url_helpers" {
        return false;
    }
    let ExprNode::Send { recv: Some(rails_app), method: routes_m, .. } = &*routes.node else {
        return false;
    };
    if routes_m.as_str() != "routes" {
        return false;
    }
    matches!(&*rails_app.node, ExprNode::Send { recv: Some(r), method: app_m, .. }
        if app_m.as_str() == "application"
            && matches!(&*r.node, ExprNode::Const { path }
                if path.len() == 1 && path[0].as_str() == "Rails"))
}

// Mailer class-side wrappers (`def self.notify = new.notify(...)`)
// moved to the shared post-analyze hook
// (`lower::apply_mailer_class_side`) — mailer classes arrive here
// with the wrappers already synthesized, keyword/block-taking methods
// on the residue ledger.

// Block-form `create!/create do |kv| ... end` inlining moved to the
// shared post-analyze hook (`lower::apply_create_block_inline`) —
// every hook body arrives here with the factory block already inlined
// (kv = X.new; body; save-or-raise; kv).

/// View-pipeline vestige of the shared `Time.current` grounding
/// (`lower::apply_time_current_lowering`): the post-analyze hook skips
/// view bodies, so lowered view classes still take the rewrite here.
/// Delete when the view pipeline migrates to shared lowerings. Every
/// other body class arrives already grounded (re-running is an
/// idempotent no-op — `Time.current` no longer occurs).
pub(crate) fn apply_time_current_lowering(lcs: &mut [LibraryClass]) {
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            crate::lower::time_current::rewrite_time_current(&mut m.body);
        }
    }
}

/// View-pipeline vestige of the shared duration grounding
/// (`lower::apply_duration_lowering`): the post-analyze hook skips view
/// bodies, so lowered view classes still take the rewrite here
/// (lobsters' `_commentbox.html.erb` compares against
/// `COMMENTABLE_DAYS.days.ago`). Delete when the view pipeline migrates
/// to shared lowerings. Every other body class arrives already
/// grounded (re-running is an idempotent no-op — the grounded form no
/// longer matches).
pub(crate) fn apply_duration_lowering(lcs: &mut [LibraryClass]) {
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            crate::lower::duration::rewrite_durations(&mut m.body);
        }
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
                    // Synthesized `<col>_raw=` writer: invalidate the
                    // reader memo (below) before storing.
                    if let Some(base) = m.name.as_str().strip_suffix("_raw=") {
                        let base_sym = Symbol::from(base);
                        if temporal.contains(&base_sym) {
                            m.body = Expr::new(
                                Span::synthetic(),
                                ExprNode::Seq {
                                    exprs: vec![
                                        Expr::new(
                                            Span::synthetic(),
                                            ExprNode::Assign {
                                                target: LValue::Ivar {
                                                    name: parse_memo_ivar(&base_sym),
                                                },
                                                value: Expr::new(
                                                    Span::synthetic(),
                                                    ExprNode::Lit { value: Literal::Nil },
                                                ),
                                            },
                                        ),
                                        m.body.clone(),
                                    ],
                                },
                            );
                        }
                    }
                }
                // Schema-synthesized temporal reader:
                // `ActiveSupport.parse_db_time(@<col>_raw)`. Profiling
                // the lobsters bench put Date._parse + its regexps at
                // ~4% of wall time — every `created_at` read re-parses
                // the same string. Memoize per instance:
                // `@__t_<col> ||= ActiveSupport.parse_db_time(@<col>_raw)`
                // (writer above invalidates). nil/"" raw re-evaluates
                // each read — parse_db_time's empty path is cheap.
                AccessorKind::Method | AccessorKind::AttributeReader
                    if temporal.contains(&m.name) && is_parse_db_time_body(&m.body) =>
                {
                    m.body = Expr::new(
                        Span::synthetic(),
                        ExprNode::OpAssign {
                            target: LValue::Ivar { name: parse_memo_ivar(&m.name) },
                            op: crate::expr::OpAssignOp::OrOr,
                            value: m.body.clone(),
                        },
                    );
                }
                _ => {}
            }
        }

        // The public `<col>=` writer (`self.<col>_raw =
        // ActiveSupport.format_db_time(value)`) is synthesized by the
        // shared model lowering (`schema::synth_temporal_writer`, kind
        // `Method` so the AttributeWriter arm above can't re-point it
        // at a nonexistent `@<col>`) — it arrives here already present,
        // and its raw-writer dispatch picks up the memo invalidation
        // installed above.
    }
}

/// `@__t_<col>` — the per-instance memo slot for a parsed temporal
/// column. Underscore-prefixed so it can't collide with a real column.
fn parse_memo_ivar(col: &Symbol) -> Symbol {
    Symbol::from(format!("__t_{}", col.as_str()))
}

/// Is this body exactly `ActiveSupport.parse_db_time(<anything>)` —
/// the synthesized temporal-reader shape?
fn is_parse_db_time_body(body: &Expr) -> bool {
    matches!(&*body.node, ExprNode::Send { recv: Some(r), method, args, .. }
        if method.as_str() == "parse_db_time"
            && args.len() == 1
            && matches!(&*r.node, ExprNode::Const { path }
                if path.len() == 1 && path[0].as_str() == "ActiveSupport"))
}

/// Ruby-family pre-emit pass: SQL NULL survives hydration as real nil.
///
/// The shared `<Model>Row.from_raw` synthesis coerces every scalar slot
/// (`(row["col"] || 0).to_i`, `(row["col"]).to_s`) so strict targets get
/// non-nilable fields — but on the Ruby tree that turns NULL into 0/""
/// and breaks Rails semantics: `group_by(&:invited_by_user_id)[nil]`
/// finds no root users (the /u tree renders empty), `banned_at?` is
/// true for everyone. For NULLABLE, non-primary-key columns, rewrite
/// the slot assign to `row["col"].nil? ? nil : <original coercion>`.
///
/// The fk 0-sentinel convention stays (belongs_to writers store 0 for
/// nil); readers' `@fk == 0` guards are WIDENED to `@fk.nil? || @fk ==
/// 0` so both representations mean "no parent". CRuby-only by
/// placement: strict targets keep the defaulted non-nilable slots
/// until the nullable-column typing workstream lands.
pub(crate) fn apply_hydration_nil_lowering(lcs: &mut [LibraryClass], app: &App) {
    for model in &app.models {
        let Some(table) = app.schema.tables.get(&model.table.0) else {
            continue;
        };
        let nullable: BTreeSet<Symbol> = table
            .columns
            .iter()
            .filter(|c| c.nullable && !c.primary_key && c.name.as_str() != "id")
            .map(|c| c.name.clone())
            .collect();
        if nullable.is_empty() {
            continue;
        }
        let row_id = ClassId(Symbol::from(format!("{}Row", model.name.0.as_str())));
        if let Some(row_lc) = lcs.iter_mut().find(|lc| lc.name == row_id) {
            for m in &mut row_lc.methods {
                if m.name.as_str() == "from_raw" {
                    nil_guard_from_raw_slots(&mut m.body, &nullable);
                }
            }
        }
        let nullable_fks: BTreeSet<Symbol> = nullable
            .iter()
            .filter(|n| n.as_str().ends_with("_id"))
            .cloned()
            .collect();
        if nullable_fks.is_empty() {
            continue;
        }
        if let Some(lc) = lcs.iter_mut().find(|lc| lc.name == model.name) {
            for m in &mut lc.methods {
                widen_fk_zero_guards(&mut m.body, &nullable_fks);
            }
        }
    }
}

/// Inside a `from_raw` body, rewrite `instance.<col> = <coercion>` for
/// nullable cols to `instance.<col> = (row["col"].nil? ? nil :
/// <coercion-sans-|| default>)`. The lookup is a pure Hash read, so the
/// duplicated evaluation in the guard is safe.
fn nil_guard_from_raw_slots(body: &mut Expr, nullable: &BTreeSet<Symbol>) {
    let ExprNode::Seq { exprs } = &mut *body.node else { return };
    for stmt in exprs.iter_mut() {
        let ExprNode::Send { method, args, .. } = &mut *stmt.node else { continue };
        let Some(col) = method.as_str().strip_suffix('=') else { continue };
        if !nullable.contains(&Symbol::from(col)) {
            continue;
        }
        let Some(value) = args.first_mut() else { continue };
        // Only Cast-wrapped scalars coerce; raw slots (bools) already
        // carry nil through.
        let ExprNode::Cast { value: inner, target_ty } = &*value.node else { continue };
        // Strip a `|| <default>` fallback (the id-shaped-column form) so
        // NULL isn't defaulted before the coercion sees it.
        let lookup = match &*inner.node {
            ExprNode::BoolOp { left, right, .. }
                if matches!(&*right.node, ExprNode::Lit { .. }) =>
            {
                left.clone()
            }
            _ => inner.clone(),
        };
        let nil_check = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(lookup.clone()),
                method: Symbol::from("nil?"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let guarded = Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: nil_check,
                then_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
                else_branch: Expr::new(
                    Span::synthetic(),
                    ExprNode::Cast { value: lookup, target_ty: target_ty.clone() },
                ),
            },
        );
        *value = guarded;
    }
}

/// `@<fk> == 0` → `@<fk>.nil? || @<fk> == 0` for nullable fks, walking
/// the whole method body (belongs_to reader guards, app-code sentinel
/// checks alike).
fn widen_fk_zero_guards(expr: &mut Expr, fks: &BTreeSet<Symbol>) {
    expr.node.for_each_child_mut(&mut |c| widen_fk_zero_guards(c, fks));
    let is_sentinel_eq = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "==" && args.len() == 1 =>
        {
            matches!(&*r.node, ExprNode::Ivar { name } if fks.contains(name))
                && matches!(
                    &*args[0].node,
                    ExprNode::Lit { value: Literal::Int { value: 0 } }
                )
        }
        _ => false,
    };
    if !is_sentinel_eq {
        return;
    }
    let ExprNode::Send { recv: Some(r), .. } = &*expr.node else { return };
    let nil_check = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(r.clone()),
            method: Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let original = expr.clone();
    *expr = Expr::new(
        expr.span,
        ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::Or,
            surface: crate::expr::BoolOpSurface::Symbol,
            left: nil_check,
            right: original,
        },
    );
}

/// Ruby-family pre-emit pass, companion to
/// `apply_hydration_nil_lowering`: once nullable columns hydrate to
/// real nil, the `.empty?` forms the predicate lowering synthesized
/// from `present?`/`blank?` (which assumed never-nil reads) crash.
/// Rewrite `<recv>.empty?` → `(<recv> || "").empty?` — single
/// evaluation, transparent for arrays/strings that are never nil, and
/// nil reads get Rails' blank-when-nil semantics. Only the
/// zero-arg `empty?` shape is touched.
pub(crate) fn apply_nilsafe_empty_lowering(lcs: &mut [LibraryClass]) {
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            rewrite_empty_nilsafe(&mut m.body);
        }
    }
}

fn rewrite_empty_nilsafe(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut |c| rewrite_empty_nilsafe(c));
    let ExprNode::Send { recv: Some(r), method, args, .. } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "empty?" || !args.is_empty() {
        return;
    }
    // Idempotence: an already-guarded `(x || "").empty?` keeps its shape.
    if matches!(&*r.node, ExprNode::BoolOp { right, .. }
        if matches!(&*right.node, ExprNode::Lit { value: Literal::Str { value } } if value.is_empty()))
    {
        return;
    }
    let guarded = Expr::new(
        r.span,
        ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::Or,
            surface: crate::expr::BoolOpSurface::Symbol,
            left: r.clone(),
            right: Expr::new(
                r.span,
                ExprNode::Lit { value: Literal::Str { value: String::new() } },
            ),
        },
    );
    *r = guarded;
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
    // A reopen of a runtime framework class (lobsters' `module
    // ActiveRecord; class Base; def q ...`) must load the runtime's
    // definition first — under plain Ruby a bare reopen would otherwise
    // DEFINE an empty class, and under spinel the reopen's method
    // bodies reference runtime members. Parent-less and named into a
    // runtime namespace is the reopen signature.
    if lc.parent.is_none() {
        if let Some(anchor) = runtime_reopen_anchor(name) {
            requires.push(relpath(&out_dir, anchor));
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
/// The runtime stem a framework-class REOPEN must load first, if the
/// class lives in a runtime namespace this tree ships. App classes with
/// `::` names outside these namespaces (ShortId::CandidateId) get None.
fn runtime_reopen_anchor(name: &str) -> Option<&'static str> {
    for (prefix, anchor) in [
        ("ActiveRecord", "runtime/active_record"),
        ("ActionController", "runtime/action_controller"),
        ("ActionView", "runtime/action_view"),
        ("ActionDispatch", "runtime/action_dispatch"),
        ("ActionMailer", "runtime/action_mailer"),
        ("ActiveJob", "runtime/active_job"),
    ] {
        if name == prefix || name.strip_prefix(prefix).is_some_and(|r| r.starts_with("::")) {
            return Some(anchor);
        }
    }
    None
}

fn require_path_for_parent(parent: &ClassId, app: &App) -> Option<String> {
    let raw = parent.0.as_str();
    if raw == "ActiveRecord::Base" {
        // When the app REOPENS ActiveRecord::Base (lobsters' `q`
        // monkeypatch, emitted as a library class), route the parent
        // require through the reopen file — it requires the runtime
        // itself first, so subclasses see both the framework methods
        // and the app's additions. Without this the reopen dangles:
        // nothing else in the require graph names it.
        if app
            .library_classes
            .iter()
            .any(|lc| lc.name.0.as_str() == "ActiveRecord::Base")
        {
            return Some("app/models/active_record/base".to_string());
        }
        return Some("runtime/active_record".to_string());
    }
    if raw == "ActionController::Base" || raw == "ActionController::API" {
        return Some("runtime/action_controller".to_string());
    }
    if raw == "ActionMailer::Base" {
        return Some("runtime/action_mailer".to_string());
    }
    if raw == "ActiveJob::Base" {
        return Some("runtime/active_job".to_string());
    }
    if app.models.iter().any(|m| m.name.0.as_str() == raw)
        || app.library_classes.iter().any(|lc| lc.name.0.as_str() == raw)
    {
        // underscore: namespaced parents nest (see emit_library_class_decls).
        return Some(format!("app/models/{}", crate::naming::underscore(raw)));
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
        // Gem façades — typed raising stand-ins for write-path-only
        // gem surface (see runtime/ruby/gem_facades.rb). One file
        // hosts every stubbed gem, so all their roots anchor here.
        "Markly" | "Nokogiri" | "Mail" => Some("runtime/gem_facades".to_string()),
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

// has_secure_password synthesis moved to the shared model lowering
// (`lower::secure_password::push_secure_password_methods`) — every
// target's model classes now carry authenticate + the plaintext
// accessors, in the bcrypt gem's own contract shape
// (`BCrypt::Password.create/new`): the CRuby/JRuby trees load the
// real gem (guarded require in the overlay main.rb), and a future
// spinel-bcrypt spin package satisfies the same calls.

fn sp_expr(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

fn ivar_read(name: &Symbol) -> Expr {
    sp_expr(ExprNode::Ivar { name: name.clone() })
}

// Request-params key normalization moved to the Ruby expr emitter's
// type-directed index hook (`emit::ruby::expr`): a symbol/dynamic key on
// a statically string-keyed hash (`Hash[String, _]`) is coerced to a
// string at the single `[]` emit chokepoint. That gates on the receiver
// *type* rather than a `params` name heuristic, so it covers views and
// helpers (params flows there too) and never touches a genuine
// symbol-keyed `Hash[Symbol, _]` like `StoryRepository#@params`.

// typed_store accessor synthesis moved to the shared model lowering
// (`lower::typed_store::push_typed_store_methods`) — every target's
// model classes now carry the reader/predicate/writer methods; the
// `TypedStore` runtime module (YAML seam) still ships only on the
// CRuby/JRuby overlay trees.

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

// ─── Runtime-Relation eager loading (issue #27 follow-up) ──────────────
//
// The static arel path already lowers `includes(:assoc)` into inline
// preload statements, but chains that reach the runtime
// `ActiveRecord::Relation` (scope chains, association relations) only
// RECORDED their `includes(...)` specs — `to_a` never executed them, so
// every association read was a lazy per-row query (the lobsters 2x
// query-count gap vs Rails, ~985 excess queries per benchmark pass:
// belongs_to singles ~870, has_many/through ~295).
//
// This pass synthesizes, per model, the statically-dispatched preload
// machinery `Relation#to_a` calls (`@model.preload_associations(records,
// @includes)` — Base supplies a no-op default):
//
//   def self.preload_associations(records, specs)   # spec walker
//   def self._preload_dispatch(records, name, nested)  # case-dispatch
//   def self._preload_batch_<assoc>(records)        # one batched IN load
//   def _preload_<belongs_to>(rec)                  # cache setter
//
// plus a cache guard prepended to each belongs_to reader (mirroring the
// has_many readers' `return @x_cache if @x_loaded` shape; the has_many
// setters/caches already exist from the static-path work).
//
// No method_missing, no send: nested specs (`story: :user`) recurse
// through the case arm's statically-named target class — the same
// case-dispatch shape as dynamic-partial rendering. Bodies are generated
// as Ruby source and parsed back through `runtime_src::parse_methods`
// (templates are fixed; identifiers come from assoc/table names).
//
// Known gaps, deliberate: has_one and scope-carrying through-assocs
// (other than a plain `order("...")`) get no batch arm — the dispatch
// falls through and the lazy reader stays correct (just N+1, matching
// Rails, which also lazy-loads what `includes` doesn't name). Assigning
// a belongs_to (`c.story = s`) on a PRELOADED record does not refresh
// the cache (fresh records never have the loaded flag set, so the
// benchmark's build-then-render flows are unaffected).
pub(crate) fn apply_preload_lowering(lcs: &mut [LibraryClass], app: &App) {
    use crate::dialect::Association;

    // Gate: runtime Relations only arise in scope-chain apps (scope-free
    // apps resolve every chain on the static arel path), and synthesis
    // only pays for itself when some `includes(...)` survives to
    // runtime. real-blog (`includes` but no scopes) and tiny-blog
    // (scopes but no `includes`) both stay byte-identical.
    let scopes = crate::lower::scope_chain::build_scope_registry(&app.models);
    if !crate::lower::scope_chain::any_scopes(&scopes) || !app_mentions_includes(app) {
        return;
    }

    for lc in lcs.iter_mut() {
        let Some(model) = app.models.iter().find(|m| m.name == lc.name) else { continue };

        // belongs_to readers gain the cache guard the has_many readers
        // already carry: `return @user_cache if @user_loaded`.
        for assoc in model.associations() {
            let Association::BelongsTo { name, .. } = assoc else { continue };
            let Some(m) = lc.methods.iter_mut().find(|m| {
                m.name == *name && m.receiver == MethodReceiver::Instance
            }) else {
                continue;
            };
            let old = std::mem::replace(
                &mut m.body,
                Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
            );
            m.body = Expr::new(
                Span::synthetic(),
                ExprNode::Seq { exprs: vec![preload_cache_guard(name), old] },
            );
        }

        let src = preload_methods_source(model, app);
        let methods = crate::runtime_src::parse_methods(&src).unwrap_or_else(|e| {
            panic!("apply_preload_lowering: generated source failed to parse: {e}\n{src}")
        });
        for mut m in methods {
            if lc.methods.iter().any(|existing| {
                existing.name == m.name && existing.receiver == m.receiver
            }) {
                continue; // user-defined names win
            }
            m.enclosing_class = Some(lc.name.0.clone());
            lc.methods.push(m);
        }
    }
}

/// `return @<name>_cache if @<name>_loaded` — the same guard shape the
/// has_many readers carry (`through_reader_body`), so preloaded and lazy
/// reads share one cache contract.
fn preload_cache_guard(name: &Symbol) -> Expr {
    let span = Span::synthetic;
    Expr::new(
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
            else_branch: Expr::new(span(), ExprNode::Lit { value: Literal::Nil }),
        },
    )
}

/// True when any raw app body (controller actions, library-class
/// methods, model scopes/methods) sends `includes`/`preload`/
/// `eager_load` to a receiver. Over-approximates (a chain the arel pass
/// later consumes statically still counts) — the cost of a false
/// positive is inert synthesized methods, not wrong behavior.
fn app_mentions_includes(app: &App) -> bool {
    use crate::dialect::{ControllerBodyItem, ModelBodyItem};
    let in_expr = expr_mentions_includes;
    app.controllers.iter().any(|c| {
        c.body.iter().any(|item| match item {
            ControllerBodyItem::Action { action, .. } => in_expr(&action.body),
            _ => false,
        })
    }) || app.library_classes.iter().any(|lc| lc.methods.iter().any(|m| in_expr(&m.body)))
        || app.models.iter().any(|m| {
            m.body.iter().any(|item| match item {
                ModelBodyItem::Scope { scope, .. } => in_expr(&scope.body),
                ModelBodyItem::Method { method, .. } => in_expr(&method.body),
                _ => false,
            })
        })
}

fn expr_mentions_includes(expr: &Expr) -> bool {
    let mut found = false;
    fn walk(e: &Expr, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(_), method, .. } = &*e.node {
            if matches!(method.as_str(), "includes" | "preload" | "eager_load") {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, found));
    }
    walk(expr, &mut found);
    found
}

/// One preloadable association, resolved against the app's model set.
enum PreloadKind {
    /// (fk column on the owner, target class, target table)
    BelongsTo { fk: String, target: String, table: String },
    /// (fk column on the target, target class)
    HasMany { fk: String, target: String },
    /// Batched form of the through-reader join:
    /// `SELECT <t>.*, <thr>.<thr_fk> AS __src FROM <t> JOIN <thr> ON
    /// <thr>.<src_fk> = <t>.id WHERE <thr>.<thr_fk> IN (...)`.
    Through { target: String, join: String, group_col: String, order: Option<String> },
}

fn preload_targets(model: &crate::dialect::Model, app: &App) -> Vec<(String, PreloadKind)> {
    use crate::dialect::Association;
    use crate::naming::pluralize_snake;

    let model_exists = |id: &ClassId| app.models.iter().any(|m| &m.name == id);
    let mut out = Vec::new();
    for assoc in model.associations() {
        match assoc {
            Association::BelongsTo { name, target, foreign_key, .. } => {
                if !model_exists(target) {
                    continue;
                }
                out.push((
                    name.as_str().to_string(),
                    PreloadKind::BelongsTo {
                        fk: foreign_key.as_str().to_string(),
                        target: target.0.as_str().to_string(),
                        table: pluralize_snake(target.0.as_str()),
                    },
                ));
            }
            Association::HasMany { name, target, foreign_key, through: None, .. } => {
                if !model_exists(target) {
                    continue;
                }
                out.push((
                    name.as_str().to_string(),
                    PreloadKind::HasMany {
                        fk: foreign_key.as_str().to_string(),
                        target: target.0.as_str().to_string(),
                    },
                ));
            }
            // Through: same two-hop resolution as
            // `apply_through_assoc_lowering`; assoc scopes other than a
            // plain `order("...")` (or none) don't batch — the lazy
            // reader keeps them correct.
            Association::HasMany {
                name, target, through: Some(thr_name), scope, ..
            } => {
                if !model_exists(target) {
                    continue;
                }
                let order = match scope {
                    None => None,
                    Some(s) => match order_literal(s) {
                        Some(o) => Some(o),
                        None => continue,
                    },
                };
                let Some(Association::HasMany { target: thr_target, foreign_key: thr_fk, .. }) =
                    model.associations().find(|a| {
                        matches!(a, Association::HasMany { name, .. } if name == thr_name)
                    })
                else {
                    continue;
                };
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
                out.push((
                    name.as_str().to_string(),
                    PreloadKind::Through {
                        target: target.0.as_str().to_string(),
                        join: format!(
                            "INNER JOIN {thr_table} ON {thr_table}.{src_fk} = {target_table}.id"
                        ),
                        group_col: format!("{thr_table}.{thr_fk}"),
                        order: order.map(|o| o.to_string()),
                    },
                ));
            }
            _ => {}
        }
    }
    out
}

/// Extract the string literal from an assoc-scope lambda body of the
/// exact shape `order("...")` (lobsters `has_many :tags, -> { order
///('tags.is_media desc, tags.tag') }, through: :taggings`).
fn order_literal(scope_body: &Expr) -> Option<&str> {
    let ExprNode::Send { recv: None, method, args, .. } = &*scope_body.node else {
        return None;
    };
    if method.as_str() != "order" || args.len() != 1 {
        return None;
    }
    let ExprNode::Lit { value: Literal::Str { value } } = &*args[0].node else {
        return None;
    };
    Some(value.as_str())
}

/// Generate the per-model preload methods as Ruby source (fed back
/// through `runtime_src::parse_methods`). Templates stay boring on
/// purpose: statement-level assigns and explicit nil-guards round-trip
/// through every walker; no `||=`-on-index, no ternaries.
fn preload_methods_source(model: &crate::dialect::Model, app: &App) -> String {
    let targets = preload_targets(model, app);
    let mut src = String::new();

    // Batch loaders + belongs_to cache setters.
    for (name, kind) in &targets {
        match kind {
            PreloadKind::BelongsTo { fk, target, table } => {
                let _ = write!(
                    src,
                    r#"
def self._preload_batch_{name}(records)
  ids = []
  records.each do |r|
    v = r.{fk}
    ids << v unless v.nil? || v == 0
  end
  ids.uniq!
  by_id = {{}}
  if ids.length > 0
    ActiveRecord.adapter.select_rows("SELECT {table}.* FROM {table} WHERE {table}.id IN (" + Db.escape_int_list(ids) + ")").each do |row|
      rec = {target}.instantiate(row)
      by_id[rec.id] = rec
    end
  end
  records.each do |r|
    r._preload_{name}(by_id[r.{fk}])
  end
  by_id.values
end

def _preload_{name}(rec)
  @{name}_cache = rec
  @{name}_loaded = true
  nil
end
"#
                );
            }
            PreloadKind::HasMany { fk, target } => {
                let _ = write!(
                    src,
                    r#"
def self._preload_batch_{name}(records)
  ids = []
  records.each do |r|
    ids << r.id
  end
  loaded = []
  if ids.length > 0
    loaded = ActiveRecord::Relation.new({target}).where({fk}: ids).to_a
  end
  grouped = {{}}
  loaded.each do |rec|
    k = rec.{fk}
    grouped[k] = [] if grouped[k].nil?
    grouped[k] << rec
  end
  records.each do |r|
    r._preload_{name}(grouped[r.id] || [])
  end
  loaded
end
"#
                );
            }
            PreloadKind::Through { target, join, group_col, order } => {
                let table = crate::naming::pluralize_snake(target.as_str());
                let order_sql = match order {
                    Some(o) => format!(" ORDER BY {o}"),
                    None => String::new(),
                };
                let _ = write!(
                    src,
                    r#"
def self._preload_batch_{name}(records)
  ids = []
  records.each do |r|
    ids << r.id
  end
  grouped = {{}}
  loaded = []
  if ids.length > 0
    rows = ActiveRecord.adapter.select_rows("SELECT {table}.*, {group_col} AS __src FROM {table} {join} WHERE {group_col} IN (" + Db.escape_int_list(ids) + "){order_sql}")
    rows.each do |row|
      rec = {target}.instantiate(row)
      loaded << rec
      k = row["__src"].to_i
      grouped[k] = [] if grouped[k].nil?
      grouped[k] << rec
    end
  end
  records.each do |r|
    r._preload_{name}(grouped[r.id] || [])
  end
  loaded
end
"#
                );
            }
        }
    }

    // Dispatch: one case arm per preloadable assoc; unknown names fall
    // through silently (lazy readers stay correct — mirrors what Rails
    // does for anything `includes` didn't name).
    src.push_str("\ndef self._preload_dispatch(records, name, nested)\n");
    if targets.is_empty() {
        src.push_str("  nil\nend\n");
    } else {
        src.push_str("  case name\n");
        for (name, kind) in &targets {
            let target = match kind {
                PreloadKind::BelongsTo { target, .. } => target,
                PreloadKind::HasMany { target, .. } => target,
                PreloadKind::Through { target, .. } => target,
            };
            let _ = write!(
                src,
                "  when :{name}\n    loaded = _preload_batch_{name}(records)\n    {target}.preload_associations(loaded, [nested]) unless nested.nil?\n"
            );
        }
        src.push_str("  end\n  nil\nend\n");
    }

    // Spec walker — the entry point `Relation#to_a` calls. Specs are
    // Symbols, Hashes (nested: `story: :user`), or Arrays of either.
    src.push_str(
        r#"
def self.preload_associations(records, specs)
  return nil if records.length == 0
  specs.each do |spec|
    if spec.is_a?(Hash)
      spec.each do |name, nested|
        _preload_dispatch(records, name, nested)
      end
    elsif spec.is_a?(Array)
      preload_associations(records, spec)
    elsif !spec.nil?
      _preload_dispatch(records, spec, nil)
    end
  end
  nil
end
"#,
    );

    src
}
