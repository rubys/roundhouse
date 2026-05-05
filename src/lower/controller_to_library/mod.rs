//! Lower a Rails-shape `Controller` into a post-lowering `LibraryClass`
//! whose body is a flat sequence of `MethodDef`s — the universal IR
//! shape every emitter consumes (see
//! `project_universal_post_lowering_ir.md`).
//!
//! The output target is `fixtures/spinel-blog/app/controllers/<name>.rb`:
//! a synthesized `process_action(action_name)` dispatcher that
//! conditionally invokes before-action filters and case-dispatches to
//! per-action methods, plus the public actions and the private filter
//! targets as ordinary methods.
//!
//! What this pass does NOT do (each is a separate follow-on lowerer):
//!
//! - Action-body rewrites: `params` → `@params`, `flash` → `@flash`,
//!   polymorphic `redirect_to @x` → `redirect_to(RouteHelpers.x_path(...))`,
//!   `Article.includes(:foo).order(...)` → `.all` + in-memory sort.
//! - Implicit-render synthesis: spinel actions all carry explicit
//!   `render(Views::...)` calls; this lowering just unwraps any
//!   `respond_to` wrappers and trusts the body otherwise.
//!
//! The skeleton landed first because it surfaces the dispatcher shape
//! (the structural piece tests can pin down) without requiring every
//! body-level rewrite to be wired up at once. Body rewrites layer on
//! top by transforming each action's `body` Expr before it's hung off
//! the synthesized `MethodDef`.

mod process_action;
pub mod params;
pub mod rewrites;
pub mod util;

use crate::dialect::{
    AccessorKind, Action, Controller, ControllerBodyItem, Filter, FilterKind, LibraryClass,
    MethodDef, MethodReceiver, Param,
};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;
use crate::lower::controller::body::{synthesize_implicit_render, unwrap_respond_to};

use self::params::ParamsSpec;
use self::process_action::synthesize_process_action;
use self::rewrites::{
    rewrite_assoc_through_parent_typed, rewrite_destroy_bang, rewrite_drop_includes,
    rewrite_model_new_to_from_params, rewrite_order_to_sort_by, rewrite_params,
    rewrite_redirect_to, rewrite_render_to_views, rewrite_route_helpers,
};
use self::util::{ivars_in_scope, method_name_for_action, views_module_name};

use std::collections::BTreeMap;

/// Bulk entry point: lower every controller against a shared class
/// registry so cross-controller / model / view dispatch types
/// correctly. Builds methods for each controller, constructs a
/// per-controller ClassInfo, then runs the body-typer with the merged
/// registry (caller-supplied `extras` plus self-derived entries).
///
/// `extras` typically carries the model + view ClassInfos so calls
/// like `Article.find(...)` and `Views::Articles.index(...)` from
/// action bodies type through the same path the model lowerer uses.
pub fn lower_controllers_to_library_classes(
    controllers: &[Controller],
    extras: Vec<(ClassId, crate::analyze::ClassInfo)>,
) -> Vec<LibraryClass> {
    // Scan source-shape action bodies for `permit(...)` declarations.
    // Each unique resource yields one `<Resource>Params` synthesized
    // class plus the (resource, fields, class_id) record we need to
    // rewrite controller bodies + register the class with the typer.
    let params_specs = self::params::collect_specs(controllers);
    let params_lcs = self::params::synthesize_params_classes(&params_specs);

    let mut all_methods: Vec<(Vec<MethodDef>, &Controller)> = Vec::new();
    for controller in controllers {
        let methods = build_methods(controller, &params_specs);
        all_methods.push((methods, controller));
    }

    let mut classes: std::collections::HashMap<ClassId, crate::analyze::ClassInfo> =
        std::collections::HashMap::new();
    // Register synthesized Params classes so dispatch on
    // `<Resource>Params.from_raw(@params)` and the typed factory
    // accessors resolves through the body-typer.
    for params_lc in &params_lcs {
        classes.insert(params_lc.name.clone(), self::params::params_class_info(params_lc));
    }
    // Framework runtime stubs (ViewHelpers, RouteHelpers, Inflector,
    // String, Broadcasts, FormBuilder, ErrorCollection). Same set
    // the view lowerer registers — controller actions call into the
    // same helpers (RouteHelpers.x_path from redirect_to rewrites,
    // ErrorCollection from @article.errors checks).
    crate::lower::view_to_library::insert_framework_stubs(&mut classes);
    // Self-info for each controller (its own synthesized methods).
    for (methods, controller) in &all_methods {
        let mut info = crate::analyze::ClassInfo::default();
        for m in methods {
            if let Some(sig) = &m.signature {
                match m.receiver {
                    MethodReceiver::Instance => {
                        info.instance_methods.insert(m.name.clone(), sig.clone());
                        info.instance_method_kinds.insert(m.name.clone(), m.kind);
                    }
                    MethodReceiver::Class => {
                        info.class_methods.insert(m.name.clone(), sig.clone());
                        info.class_method_kinds.insert(m.name.clone(), m.kind);
                    }
                }
            }
        }
        // ApplicationController baseline — render/redirect_to/head/params
        // surface in every action body, and the typer needs signatures
        // to dispatch through SelfRef.
        insert_baseline_controller_methods(&mut info);
        // Tag baseline entries that lacked an explicit kind as Method
        // (render/redirect_to/head/params are all real method calls).
        for name in info.instance_methods.keys().cloned().collect::<Vec<_>>() {
            info.instance_method_kinds.entry(name).or_insert(AccessorKind::Method);
        }
        for name in info.class_methods.keys().cloned().collect::<Vec<_>>() {
            info.class_method_kinds.entry(name).or_insert(AccessorKind::Method);
        }
        classes.insert(controller.name.clone(), info);
    }
    for (id, info) in extras {
        classes.insert(id, info);
    }

    // Ivar bindings: `@params` is framework-guaranteed (the lowerer
    // itself rewrites bare `params` → `@params` in action bodies, so
    // every controller has it; ActionController's runtime constructs
    // a Hash-shaped Parameters object). This isn't a naming
    // heuristic — it's a fact about the framework that the lowerer
    // KNOWS because it produced the @params reference.
    //
    // Other ivars (`@article`, `@articles`, `@comment`, ...) come
    // from inlined filter bodies: when `set_article` runs
    // `@article = Article.find(@params[:id].to_i)` at the top of
    // an action, the body-typer's Seq walk picks it up and
    // propagates the type to downstream reads. No naming guess.
    let mut framework_ivars: std::collections::HashMap<Symbol, Ty> =
        std::collections::HashMap::new();
    framework_ivars.insert(
        Symbol::from("params"),
        Ty::Hash {
            key: Box::new(Ty::Sym),
            value: Box::new(Ty::Untyped),
        },
    );

    let mut out = Vec::new();
    for (mut methods, controller) in all_methods {
        for method in &mut methods {
            crate::lower::typing::type_method_body(method, &classes, &framework_ivars);
        }
        out.push(LibraryClass {
            name: controller.name.clone(),
            is_module: false,
            parent: controller.parent.clone(),
            includes: Vec::new(),
            methods,
            origin: None,
        });
    }
    // Type-check synthesized Params class method bodies with a per-class
    // ivar map seeded from the permitted-fields list. Each `attr_reader`
    // body is `@<field>` whose type comes from this map; without the
    // seed, the typer leaves it as `TyVar(0)` and the strict residual
    // check fails.
    let mut params_lcs = params_lcs;
    for params_lc in &mut params_lcs {
        let mut params_ivars: std::collections::HashMap<Symbol, Ty> =
            std::collections::HashMap::new();
        if let Some(crate::dialect::LibraryClassOrigin::ResourceParams { fields, .. }) =
            &params_lc.origin
        {
            for f in fields {
                params_ivars.insert(f.clone(), Ty::Str);
            }
        }
        for method in &mut params_lc.methods {
            crate::lower::typing::type_method_body(method, &classes, &params_ivars);
        }
    }
    // Append synthesized Params classes after controllers. Each becomes
    // its own `app/models/<resource>_params.{rb,ts}` file via the
    // standard per-LC emit path.
    out.extend(params_lcs);
    out
}

/// Single-controller entry point — kept for tests and call sites that
/// don't need cross-class typing. For whole-app emit, use
/// `lower_controllers_to_library_classes`.
pub fn lower_controller_to_library_class(controller: &Controller) -> LibraryClass {
    let specs = self::params::collect_specs(std::slice::from_ref(controller));
    let methods = build_methods(controller, &specs);
    LibraryClass {
        name: controller.name.clone(),
        is_module: false,
        parent: controller.parent.clone(),
        includes: Vec::new(),
        methods,
        origin: None,
    }
}

fn build_methods(
    controller: &Controller,
    params_specs: &BTreeMap<Symbol, ParamsSpec>,
) -> Vec<MethodDef> {
    let mut methods: Vec<MethodDef> = Vec::new();

    let (publics, privs) = split_public_private_actions(controller);
    let before_filters: Vec<&Filter> = controller
        .filters()
        .filter(|f| matches!(f.kind, FilterKind::Before))
        .collect();

    // Inline before_action filter bodies into each action that
    // fires them. This pushes the assignment to `@article` (etc) into
    // the action body, where the body-typer's Seq walk picks it up
    // and types subsequent reads correctly. Self-describing IR — no
    // convention-based ivar naming heuristic needed downstream.
    let publics_inlined: Vec<Action> = publics
        .iter()
        .map(|a| inline_before_filters(a, &before_filters, &privs))
        .collect();

    // Filter targets that are PURELY filter targets (called only via
    // before_action, never from an action body) are dead after
    // inlining — drop them from the emitted methods. Filter targets
    // that are also called from action bodies (e.g., `_params`
    // helpers — actually those don't appear in before_filters, but
    // be defensive) stay.
    let filter_target_names: std::collections::HashSet<&Symbol> =
        before_filters.iter().map(|f| &f.target).collect();
    let privs_kept: Vec<Action> = privs
        .iter()
        .filter(|a| !filter_target_names.contains(&a.name))
        .cloned()
        .collect();

    if !publics_inlined.is_empty() {
        // Filter dispatch is removed from process_action since the
        // filters are now inlined directly into the actions; emit
        // an empty filter list so the dispatcher just routes by
        // action_name.
        methods.push(synthesize_process_action(
            &[],
            &publics_inlined,
            controller.name.0.clone(),
        ));
    }

    for a in &publics_inlined {
        methods.push(action_to_method(a, controller, &privs, /*is_public=*/ true, params_specs));
    }
    for a in &privs_kept {
        methods.push(action_to_method(a, controller, &privs, /*is_public=*/ false, params_specs));
    }

    methods
}

/// Return a copy of `action` with every applicable before_action
/// filter target's body prepended to the action body. A filter
/// applies when its `only:` includes the action name, or its
/// `except:` doesn't, or it has neither (unconditional).
fn inline_before_filters(action: &Action, filters: &[&Filter], privs: &[Action]) -> Action {
    let action_name = &action.name;
    let mut prepended: Vec<Expr> = Vec::new();
    for f in filters {
        let applies = if !f.only.is_empty() {
            f.only.contains(action_name)
        } else if !f.except.is_empty() {
            !f.except.contains(action_name)
        } else {
            true
        };
        if !applies {
            continue;
        }
        // Look up the filter's target action by name in privs.
        // (Filter targets are conventionally private actions; if the
        // target isn't found, skip — could be a built-in framework
        // helper we don't model.)
        let Some(target) = privs.iter().find(|a| &a.name == &f.target) else {
            continue;
        };
        match &*target.body.node {
            ExprNode::Seq { exprs } => prepended.extend(exprs.iter().cloned()),
            _ => prepended.push(target.body.clone()),
        }
    }
    if prepended.is_empty() {
        return action.clone();
    }
    // Compose: prepended filter stmts + action body stmts → new Seq.
    let mut combined: Vec<Expr> = prepended;
    match &*action.body.node {
        ExprNode::Seq { exprs } => combined.extend(exprs.iter().cloned()),
        _ => combined.push(action.body.clone()),
    }
    let mut new_action = action.clone();
    new_action.body = Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Seq { exprs: combined },
    );
    new_action
}

/// ApplicationController baseline — methods every action body may
/// reference via implicit-self dispatch. Signatures are loose
/// (`Untyped` for kwargs, return Nil for terminal helpers); refining
/// per-arg types lands when a routing-table-aware typer surfaces.
fn insert_baseline_controller_methods(info: &mut crate::analyze::ClassInfo) {
    use crate::lower::typing::fn_sig;
    let any_hash = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };

    // Terminals — render/redirect/head/render_404 all return Nil.
    // The framework runtime declares these with named keyword params
    // (`render(html, status: 200)`, `redirect_to(path, notice: nil,
    // alert: nil, status: :found)`), so the trailing kwargs Hash
    // SHOULD stay as bare named-args at the call site. Use a
    // `KeywordRest` `**opts` shape so the body-typer's
    // normalize_trailing_kwargs treats the trailing Hash as kwargs
    // (kept), not as a positional Hash (flipped). The simplification
    // doesn't matter for typing — we don't check per-key types of
    // controller render options today.
    let kw_rest_opts = || -> Ty {
        Ty::Fn {
            params: vec![crate::ty::Param {
                name: Symbol::from("opts"),
                ty: any_hash.clone(),
                kind: crate::ty::ParamKind::KeywordRest,
            }],
            block: None,
            ret: Box::new(Ty::Nil),
            effects: crate::effect::EffectSet::pure(),
        }
    };
    let positional_with_kwargs = |first_name: &str, first_ty: Ty| -> Ty {
        Ty::Fn {
            params: vec![
                crate::ty::Param {
                    name: Symbol::from(first_name),
                    ty: first_ty,
                    kind: crate::ty::ParamKind::Required,
                },
                crate::ty::Param {
                    name: Symbol::from("opts"),
                    ty: any_hash.clone(),
                    kind: crate::ty::ParamKind::KeywordRest,
                },
            ],
            block: None,
            ret: Box::new(Ty::Nil),
            effects: crate::effect::EffectSet::pure(),
        }
    };
    info.instance_methods
        .entry(Symbol::from("render"))
        .or_insert_with(|| positional_with_kwargs("html", Ty::Untyped));
    info.instance_methods
        .entry(Symbol::from("redirect_to"))
        .or_insert_with(|| positional_with_kwargs("location", Ty::Untyped));
    info.instance_methods
        .entry(Symbol::from("head"))
        .or_insert_with(|| fn_sig(vec![(Symbol::from("status"), Ty::Sym)], Ty::Nil));
    let _ = kw_rest_opts; // helper retained for future zero-positional kwargs callees

    // Implicit-`params` — actions read `@params` (the lowerer rewrote
    // bare `params` → `@params`) which the typer should treat as a
    // Hash-shaped object. The instance-method version is for cases
    // the rewrite missed.
    info.instance_methods
        .entry(Symbol::from("params"))
        .or_insert_with(|| fn_sig(vec![], any_hash));
}

/// Walk the controller body in source order, partitioning actions at
/// the `private` marker. Filters and unknown class-body statements are
/// dropped here — filters get re-synthesized into `process_action`,
/// unknowns (e.g. `allow_browser`) carry no semantics in spinel.
fn split_public_private_actions(c: &Controller) -> (Vec<Action>, Vec<Action>) {
    let mut pubs = Vec::new();
    let mut privs = Vec::new();
    let mut seen_private = false;
    for item in &c.body {
        match item {
            ControllerBodyItem::PrivateMarker { .. } => seen_private = true,
            ControllerBodyItem::Action { action, .. } => {
                if seen_private {
                    privs.push(action.clone());
                } else {
                    pubs.push(action.clone());
                }
            }
            _ => {}
        }
    }
    (pubs, privs)
}

/// Convert one `Action` into a `MethodDef`. Renames `new` →
/// `new_action` (Ruby `def new` would shadow `Object#new`); applies
/// the full action-body rewrite pipeline (see `lower_action_body`).
/// `is_public` gates the implicit-render synthesis: private filter
/// targets (`set_article`) and param helpers (`article_params`)
/// don't render — their callers do.
fn action_to_method(
    a: &Action,
    controller: &Controller,
    privs: &[Action],
    is_public: bool,
    params_specs: &BTreeMap<Symbol, ParamsSpec>,
) -> MethodDef {
    let method_name = method_name_for_action(a.name.as_str());
    let params: Vec<Param> = a
        .params
        .fields
        .iter()
        .map(|(n, _)| Param::positional(n.clone()))
        .collect();
    let body = lower_action_body(
        &a.body,
        controller,
        a.name.as_str(),
        privs,
        is_public,
        params_specs,
    );
    // Action params type to Untyped for now — Rails action signatures
    // are conventionally `def show(id)` with all-string CGI inputs;
    // refinement to per-route param types can ride on a later
    // routing-table-aware pass.
    //
    // Return type:
    //   - Public actions terminate in render/redirect (synthesized or
    //     explicit) → Nil.
    //   - Private `_params` helpers return the typed `<Resource>Params`
    //     class (callers do `Model.from_params(comment_params)`); the
    //     resource is derived by stripping the `_params` suffix.
    //   - Other private actions default to Nil; refine when a
    //     forcing fixture surfaces.
    let ret_ty = if !is_public && method_name.ends_with("_params") {
        let resource = Symbol::from(method_name.trim_end_matches("_params"));
        if let Some(spec) = params_specs.get(&resource) {
            Ty::Class { id: spec.class_id.clone(), args: vec![] }
        } else {
            // Fallback for helpers whose resource we didn't recognize
            // (shouldn't happen for source-derived helpers, but stays
            // typed-coarse rather than panicking).
            Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) }
        }
    } else {
        Ty::Nil
    };
    let sig_params: Vec<(Symbol, Ty)> = params
        .iter()
        .map(|p| (p.name.clone(), Ty::Untyped))
        .collect();
    // All actions (public + private) are Method — bodies are
    // imperative and computed. AttributeReader is reserved for
    // pure ivar-backed reads that can lower to a TS field.
    MethodDef {
        name: Symbol::from(method_name),
        receiver: MethodReceiver::Instance,
        params,
        body,
        signature: Some(crate::lower::typing::fn_sig(sig_params, ret_ty)),
        effects: a.effects.clone(),
        enclosing_class: Some(controller.name.0.clone()),
        kind: AccessorKind::Method,
    }
}

/// Apply the controller-body rewrite pipeline in declared order:
///
/// 1. `unwrap_respond_to` — drop `respond_to do |format| format.html
///    {…}; format.json {…} end` wrappers, keeping the HTML branch.
/// 2. `synthesize_implicit_render` — append `render :<action>` when
///    the body has no top-level terminal (Rails' implicit-render).
/// 3. `rewrite_render_to_views` — `render :sym, **kw` →
///    `render(Views::<Module>.<sym>(<ivars>), **kw)`. Uses the action's
///    ivar scope (body + every `before_action` filter target that fires)
///    to determine the positional args of the Views call.
/// 4. `rewrite_params` — `params` → `@params`, `params.expect(...)` →
///    indexed/require-permit forms.
/// 5. `rewrite_redirect_to` — polymorphic `redirect_to @x` →
///    `redirect_to(RouteHelpers.<x>_path(@x.id), ...)`.
/// 6. `rewrite_assoc_through_parent` — `@parent.assoc.build(args)` →
///    3-statement `attrs = …; attrs[:fk] = @parent.id; @x = Class.new(attrs)`.
///    `@parent.assoc.find(args)` → `@x = Class.find(args); if @x.fk !=
///    @parent.id; head(:not_found); return; end`.
/// 7. `rewrite_drop_includes` — drop `.includes(…)` from method chains.
///    Spinel has no relation-level eager-load; access is lazy by default.
/// 8. `rewrite_order_to_sort_by` — `<recv>.order(field: dir)` →
///    `<recv'>.sort_by { |a| a.field.to_s }<.reverse>` (`<recv'>` =
///    recv with `.all` prepended if recv is a bare Const).
/// 9. `rewrite_params_helpers_to_h` — wrap bare `<x>_params` calls with
///    `.to_h`. Spinel's strong-params chain returns a Parameters-like
///    object; model constructors expect a plain Hash.
/// 10. `rewrite_destroy_bang` — `<recv>.destroy!` → `<recv>.destroy`.
///    Spinel's runtime model has only one destroy variant.
/// 11. `rewrite_route_helpers` — bare `<x>_path` → `RouteHelpers.<x>_path`
///    (covers `articles_path` and the like that appear outside
///    redirect_to's first arg).
///
/// Run in this order because each pass leaves the IR in a shape the
/// next pass expects: render-views needs the synthesized symbol-form
/// call to rewrite; redirect_to rewrite needs the bare ivar before
/// route_helpers prefixes it; route_helpers needs to skip already-
/// rewritten `RouteHelpers.x_path(...)` calls (they have a recv now).
fn lower_action_body(
    body: &Expr,
    controller: &Controller,
    action_name: &str,
    privs: &[Action],
    is_public: bool,
    params_specs: &BTreeMap<Symbol, ParamsSpec>,
) -> Expr {
    let unwrapped = unwrap_respond_to(body);
    let with_render = if is_public {
        let synth = synthesize_implicit_render(&unwrapped, action_name);
        let ivars = ivars_in_scope(controller, action_name, &synth, privs);
        let module_name = views_module_name(controller);
        rewrite_render_to_views(&synth, module_name.as_deref(), &ivars)
    } else {
        unwrapped
    };
    let with_params = rewrite_params(&with_render);
    // After bare `params.expect(...)` / `params.require(:r).permit(...)`
    // canonicalize via `rewrite_params`, replace each permit chain with
    // the typed factory `<Resource>Params.from_raw(@params)`. The
    // controller's `<resource>_params` helper body becomes that single
    // call; downstream call sites see a typed value, not a Hash.
    let with_typed_params = self::params::rewrite_to_from_raw(&with_params, params_specs);
    let with_redirects = rewrite_redirect_to(&with_typed_params);
    // Rewrite `<Model>.new(<resource>_params)` → `<Model>.from_params(<resource>_params)`
    // BEFORE the assoc-through-parent rewrite, so the build path picks
    // up the typed factory shape rather than the legacy attrs-Hash.
    let with_from_params =
        rewrite_model_new_to_from_params(&with_redirects, privs, params_specs);
    let with_assoc =
        rewrite_assoc_through_parent_typed(&with_from_params, privs, params_specs);
    let with_no_includes = rewrite_drop_includes(&with_assoc);
    let with_order = rewrite_order_to_sort_by(&with_no_includes);
    let with_destroy = rewrite_destroy_bang(&with_order);
    let with_routes = rewrite_route_helpers(&with_destroy);
    // Some rewrites (rewrite_assoc_through_parent in particular)
    // produce nested Seqs — `Seq { ..., Seq { stmts }, ... }`. The
    // body-typer's Seq walker only propagates ivar bindings from
    // immediate-child Assigns; nested Seqs swallow their own
    // bindings. Splice nested Seqs into their parent so each
    // assignment is visible to subsequent siblings.
    flatten_seqs(&with_routes)
}

/// Splice nested `Seq` nodes into their parent: `Seq { ..., Seq {
/// stmts }, ... }` becomes `Seq { ..., stmts..., ... }`. Recursive
/// so deeper nesting flattens too.
fn flatten_seqs(expr: &Expr) -> Expr {
    use crate::expr::ExprNode;
    fn flatten(e: &Expr) -> Expr {
        let new_node = match &*e.node {
            ExprNode::Seq { exprs } => {
                let mut flat: Vec<Expr> = Vec::new();
                for child in exprs.iter().map(flatten) {
                    if let ExprNode::Seq { exprs: inner } = &*child.node {
                        flat.extend(inner.iter().cloned());
                    } else {
                        flat.push(child);
                    }
                }
                ExprNode::Seq { exprs: flat }
            }
            ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
                cond: flatten(cond),
                then_branch: flatten(then_branch),
                else_branch: flatten(else_branch),
            },
            ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
                recv: recv.as_ref().map(flatten),
                method: method.clone(),
                args: args.iter().map(flatten).collect(),
                block: block.as_ref().map(flatten),
                parenthesized: *parenthesized,
            },
            ExprNode::Apply { fun, args, block } => ExprNode::Apply {
                fun: flatten(fun),
                args: args.iter().map(flatten).collect(),
                block: block.as_ref().map(flatten),
            },
            ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
                params: params.clone(),
                block_param: block_param.clone(),
                body: flatten(body),
                block_style: *block_style,
            },
            ExprNode::Assign { target, value } => ExprNode::Assign {
                target: target.clone(),
                value: flatten(value),
            },
            // Leaves and other composites pass through unchanged for now —
            // nested Seqs only come from the assoc rewrite and live at
            // top-level positions inside Seq/If/Lambda bodies. Extend
            // this when other rewrites introduce inner Seqs in different
            // positions.
            _ => return e.clone(),
        };
        Expr {
            span: e.span,
            node: Box::new(new_node),
            ty: e.ty.clone(),
            effects: e.effects.clone(),
            leading_blank_line: e.leading_blank_line,
            diagnostic: e.diagnostic.clone(),
        }
    }
    flatten(expr)
}
