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
mod rewrites;
mod util;

use crate::dialect::{
    Action, Controller, ControllerBodyItem, Filter, FilterKind, LibraryClass, MethodDef,
    MethodReceiver, Param,
};
use crate::expr::Expr;
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;
use crate::lower::controller::body::{synthesize_implicit_render, unwrap_respond_to};

use self::process_action::synthesize_process_action;
use self::rewrites::{
    rewrite_assoc_through_parent, rewrite_destroy_bang, rewrite_drop_includes,
    rewrite_order_to_sort_by, rewrite_params, rewrite_params_helpers_to_h,
    rewrite_redirect_to, rewrite_render_to_views, rewrite_route_helpers,
};
use self::util::{ivars_in_scope, method_name_for_action, views_module_name};

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
    let mut all_methods: Vec<(Vec<MethodDef>, &Controller)> = Vec::new();
    for controller in controllers {
        let methods = build_methods(controller);
        all_methods.push((methods, controller));
    }

    let mut classes: std::collections::HashMap<ClassId, crate::analyze::ClassInfo> =
        std::collections::HashMap::new();
    // Self-info for each controller (its own synthesized methods).
    for (methods, controller) in &all_methods {
        let mut info = crate::analyze::ClassInfo::default();
        for m in methods {
            if let Some(sig) = &m.signature {
                match m.receiver {
                    MethodReceiver::Instance => {
                        info.instance_methods.insert(m.name.clone(), sig.clone());
                    }
                    MethodReceiver::Class => {
                        info.class_methods.insert(m.name.clone(), sig.clone());
                    }
                }
            }
        }
        // ApplicationController baseline — render/redirect_to/head/params
        // surface in every action body, and the typer needs signatures
        // to dispatch through SelfRef.
        insert_baseline_controller_methods(&mut info);
        classes.insert(controller.name.clone(), info);
    }
    for (id, info) in extras {
        classes.insert(id, info);
    }

    let empty_ivars: std::collections::HashMap<Symbol, Ty> =
        std::collections::HashMap::new();
    let mut out = Vec::new();
    for (mut methods, controller) in all_methods {
        for method in &mut methods {
            crate::lower::typing::type_method_body(method, &classes, &empty_ivars);
        }
        out.push(LibraryClass {
            name: controller.name.clone(),
            is_module: false,
            parent: controller.parent.clone(),
            includes: Vec::new(),
            methods,
        });
    }
    out
}

/// Single-controller entry point — kept for tests and call sites that
/// don't need cross-class typing. For whole-app emit, use
/// `lower_controllers_to_library_classes`.
pub fn lower_controller_to_library_class(controller: &Controller) -> LibraryClass {
    let methods = build_methods(controller);
    LibraryClass {
        name: controller.name.clone(),
        is_module: false,
        parent: controller.parent.clone(),
        includes: Vec::new(),
        methods,
    }
}

fn build_methods(controller: &Controller) -> Vec<MethodDef> {
    let mut methods: Vec<MethodDef> = Vec::new();

    let (publics, privs) = split_public_private_actions(controller);
    let before_filters: Vec<&Filter> = controller
        .filters()
        .filter(|f| matches!(f.kind, FilterKind::Before))
        .collect();

    if !publics.is_empty() || !before_filters.is_empty() {
        methods.push(synthesize_process_action(
            &before_filters,
            &publics,
            controller.name.0.clone(),
        ));
    }

    for a in &publics {
        methods.push(action_to_method(a, controller, &privs, /*is_public=*/ true));
    }
    for a in &privs {
        methods.push(action_to_method(a, controller, &privs, /*is_public=*/ false));
    }

    methods
}

/// ApplicationController baseline — methods every action body may
/// reference via implicit-self dispatch. Signatures are loose
/// (`Untyped` for kwargs, return Nil for terminal helpers); refining
/// per-arg types lands when a routing-table-aware typer surfaces.
fn insert_baseline_controller_methods(info: &mut crate::analyze::ClassInfo) {
    use crate::lower::typing::fn_sig;
    let any_hash = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    let opts = vec![(Symbol::from("opts"), any_hash.clone())];

    // Terminals — render/redirect/head/render_404 all return Nil.
    info.instance_methods
        .entry(Symbol::from("render"))
        .or_insert_with(|| fn_sig(opts.clone(), Ty::Nil));
    info.instance_methods
        .entry(Symbol::from("redirect_to"))
        .or_insert_with(|| {
            fn_sig(
                vec![
                    (Symbol::from("location"), Ty::Untyped),
                    (Symbol::from("opts"), any_hash.clone()),
                ],
                Ty::Nil,
            )
        });
    info.instance_methods
        .entry(Symbol::from("head"))
        .or_insert_with(|| fn_sig(vec![(Symbol::from("status"), Ty::Sym)], Ty::Nil));

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
) -> MethodDef {
    let method_name = method_name_for_action(a.name.as_str());
    let params: Vec<Param> = a
        .params
        .fields
        .iter()
        .map(|(n, _)| Param::positional(n.clone()))
        .collect();
    let body = lower_action_body(&a.body, controller, a.name.as_str(), privs, is_public);
    // Action params type to Untyped for now — Rails action signatures
    // are conventionally `def show(id)` with all-string CGI inputs;
    // refinement to per-route param types can ride on a later
    // routing-table-aware pass. Return type Nil: every action body
    // terminates in render/redirect (or is implicit-render-synthesized
    // to do so), and both helpers return Nil after side-effects.
    let sig_params: Vec<(Symbol, Ty)> = params
        .iter()
        .map(|p| (p.name.clone(), Ty::Untyped))
        .collect();
    MethodDef {
        name: Symbol::from(method_name),
        receiver: MethodReceiver::Instance,
        params,
        body,
        signature: Some(crate::lower::typing::fn_sig(sig_params, Ty::Nil)),
        effects: a.effects.clone(),
        enclosing_class: Some(controller.name.0.clone()),
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
    let with_redirects = rewrite_redirect_to(&with_params);
    let with_assoc = rewrite_assoc_through_parent(&with_redirects);
    let with_no_includes = rewrite_drop_includes(&with_assoc);
    let with_order = rewrite_order_to_sort_by(&with_no_includes);
    let with_params_to_h = rewrite_params_helpers_to_h(&with_order, privs);
    let with_destroy = rewrite_destroy_bang(&with_params_to_h);
    rewrite_route_helpers(&with_destroy)
}
