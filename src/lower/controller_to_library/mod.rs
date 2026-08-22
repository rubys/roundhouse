//! Lower a Rails-shape `Controller` into a post-lowering `LibraryClass`
//! whose body is a flat sequence of `MethodDef`s — the universal IR
//! shape every emitter consumes (see
//! `project_universal_post_lowering_ir.md`).
//!
//! The output target is the emitted `app/controllers/<name>.rb` in the
//! spinel-shape tree: a synthesized `process_action(action_name)`
//! dispatcher that conditionally invokes before-action filters and
//! case-dispatches to per-action methods, plus the public actions and
//! the private filter targets as ordinary methods.
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

mod broadcasts;
mod process_action;
pub mod params;
pub mod rewrites;
pub mod util;

use crate::dialect::{
    AccessorKind, Action, Controller, ControllerBodyItem, Filter, FilterKind, LibraryClass,
    MethodDef, MethodReceiver, Param,
};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;
use crate::lower::controller::body::{
    has_toplevel_terminal, synthesize_deferred_implicit_render, synthesize_implicit_render,
    unwrap_respond_to_with_format_dispatch, FormatBreadth,
};

use self::params::{helper_spec_map, ParamsSpec, ParamsSpecs};
use self::process_action::{halt_if_performed, synthesize_process_action, PreambleStmt};
use self::rewrites::{
    rewrite_assoc_through_parent_typed, rewrite_destroy_bang,
    rewrite_model_new_to_from_params, rewrite_update_to_typed_variant, rewrite_params,
    rewrite_redirect_to, rewrite_render_location_kwarg, rewrite_render_to_views,
    rewrite_route_helpers,
};
use self::util::{ivars_in_scope, method_name_for_action, views_module_name};

/// Collect the set of action symbols on `controller` that have a
/// `*.json.jbuilder` template under the controller's view directory.
/// Empty when no jbuilder templates apply. Used to gate the implicit-
/// render dispatch synthesis.
fn json_actions_for(
    controller: &Controller,
    views: &[crate::dialect::View],
) -> std::collections::HashSet<Symbol> {
    let mut out: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
    let module = match views_module_name(controller) {
        Some(m) => m,
        None => return out,
    };
    // `underscore`, not `snake_case`: a NAMESPACED controller's module
    // is `Rooms::Refreshes`, and only `underscore` turns that into the
    // `rooms/refreshes` a view name is keyed by — `snake_case` leaves
    // the `::` and the prefix matched nothing, so every namespaced
    // controller's json/turbo_stream templates were invisible and its
    // action fell through to the html branch and MissingTemplate.
    let dir = crate::naming::underscore(&module);
    let prefix = format!("{dir}/");
    for v in views {
        if v.format.as_str() != "json" {
            continue;
        }
        let name = v.name.as_str();
        if let Some(stem) = name.strip_prefix(&prefix) {
            // Partials (`_article`) shouldn't count as implicit-
            // render actions; they're rendered via `partial!` from
            // other templates, not from controller dispatch.
            if !stem.starts_with('_') {
                out.insert(Symbol::from(stem));
            }
        }
    }
    out
}

/// Actions with a `<action>.turbo_stream.erb` template, the same scan
/// `json_actions_for` does for jbuilder. Turbo negotiates
/// `text/vnd.turbo-stream.html` on a form submission and Rails renders
/// that template for it; without the dispatch, an action whose ONLY
/// template is the turbo_stream one falls through to the html branch and
/// raises MissingTemplate (campfire's `MessagesController#create` has no
/// `create.html.erb` at all).
fn turbo_stream_actions_for(
    controller: &Controller,
    views: &[crate::dialect::View],
) -> std::collections::HashSet<Symbol> {
    let mut out: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
    let module = match views_module_name(controller) {
        Some(m) => m,
        None => return out,
    };
    // See `json_actions_for`: `underscore` is what a namespaced
    // controller's module has to go through to match a view name.
    let dir = crate::naming::underscore(&module);
    let prefix = format!("{dir}/");
    for v in views {
        if v.format.as_str() != "turbo_stream" {
            continue;
        }
        if let Some(stem) = v.name.as_str().strip_prefix(&prefix) {
            if !stem.starts_with('_') {
                out.insert(Symbol::from(stem));
            }
        }
    }
    out
}

/// `(view-module, action-stem) -> ViewArgs` for the render rewrite.
/// Built once from the app's views; see `action_view_ivar_map`.
type PartialMap = std::collections::HashMap<
    (String, String),
    crate::lower::view_to_library::PartialCallContract,
>;
type ViewIvarMap =
    std::collections::HashMap<(String, String), crate::lower::view_to_library::ViewArgs>;

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
    lower_controllers_with_arel_and_views(controllers, extras, None, &[])
}

/// Variant that also accepts the app `Schema`. When provided, the
/// Arel pass runs over each typed action body, lifting statically-
/// resolvable AR call chains (`Article.includes(:c).order(col: :dir)`)
/// into inline SELECT/hydrate expansions over the `Db` primitive
/// surface. The legacy `rewrite_drop_includes` +
/// `rewrite_order_to_sort_by` then run as a fallback for chains
/// Arel doesn't recognize. See project_arel_compile_time_first.md.
///
/// `schema` None preserves legacy-only behavior — used by callers
/// that don't need the SQL-level chain emission (tests, dump_ir).
pub fn lower_controllers_with_arel(
    controllers: &[Controller],
    extras: Vec<(ClassId, crate::analyze::ClassInfo)>,
    schema: Option<&crate::schema::Schema>,
) -> Vec<LibraryClass> {
    lower_controllers_with_arel_and_views(controllers, extras, schema, &[])
}

/// Variant of `lower_controllers_with_arel` that also accepts the
/// app's `views` slice. The view list is scanned for
/// `*.json.jbuilder` templates so each controller's implicit-render
/// path can synthesize a format dispatch when the corresponding
/// `<action>.json.jbuilder` exists. Without this, `GET
/// /articles.json` would render html instead of the jbuilder
/// template.
pub fn lower_controllers_with_arel_and_views(
    controllers: &[Controller],
    extras: Vec<(ClassId, crate::analyze::ClassInfo)>,
    schema: Option<&crate::schema::Schema>,
    views: &[crate::dialect::View],
) -> Vec<LibraryClass> {
    lower_controllers_with_arel_views_and_assocs(controllers, extras, schema, views, &[])
}

/// As `lower_controllers_with_arel_and_views`, plus the app's
/// association graph so action-body `includes(:assoc)` chains lower to
/// eager-load preloads (issue #27). The 4-arg wrapper passes an empty
/// graph, preserving the legacy drop-includes behavior for callers that
/// haven't wired the graph yet.
pub fn lower_controllers_with_arel_views_and_assocs(
    controllers: &[Controller],
    extras: Vec<(ClassId, crate::analyze::ClassInfo)>,
    schema: Option<&crate::schema::Schema>,
    views: &[crate::dialect::View],
    assocs: &[crate::lower::model_associations::AssociationEdge],
) -> Vec<LibraryClass> {
    lower_controllers_with_arel_views_assocs_and_routes(
        controllers,
        extras,
        LowerControllerOptions { schema, views, assocs, ..Default::default() },
    )
}

/// As `lower_controllers_with_arel_views_and_assocs`, plus a per-controller
/// map of route-reachable action names. When supplied, a public controller
/// method is treated as a routable action (implicit render + `process_action`
/// dispatch) ONLY if a route reaches it; other public methods are emitted as
/// plain helper methods (no implicit render). This is what lets a base
/// controller's `helper_method` / filter methods (e.g.
/// `ApplicationController#tags_filtered_by_cookie`) keep their real return
/// value instead of being clobbered by a synthesized `render`. `None`
/// preserves the legacy "every public method is an action" behavior for
/// callers that haven't wired routes yet.
/// The optional, feature-gated inputs to
/// [`lower_controllers_with_arel_views_assocs_and_routes`]. Each field
/// defaults to "feature off" (empty slice / `None` / `false`), matching
/// the legacy behavior the telescoping wrappers preserve — so a caller
/// wiring only some features writes `LowerControllerOptions { schema,
/// views, ..Default::default() }` instead of trailing `&[], None, false`
/// positional args.
#[derive(Default)]
pub struct LowerControllerOptions<'a> {
    /// App `Schema` — enables the Arel SQL-chain lowering pass.
    pub schema: Option<&'a crate::schema::Schema>,
    /// App views — scanned for `*.json.jbuilder` format dispatch and the
    /// view↔controller ivar contract.
    pub views: &'a [crate::dialect::View],
    /// App library classes — used to resolve controller-side partial
    /// render contracts.
    pub library_classes: &'a [crate::dialect::LibraryClass],
    /// Association graph — lowers `includes(:assoc)` to eager-load
    /// preloads (issue #27).
    pub assocs: &'a [crate::lower::model_associations::AssociationEdge],
    /// Per-controller route-reachable action names. `Some` restricts
    /// implicit-render/dispatch to routed actions; `None` is legacy
    /// "every public method is an action."
    pub routed_by_controller:
        Option<&'a std::collections::HashMap<ClassId, std::collections::HashSet<Symbol>>>,
    /// Whether to synthesize the full format-dispatch breadth.
    pub format_breadth: FormatBreadth,
    /// Per route helper, which positional segments are id-shaped —
    /// `crate::lower::routes::helper_id_segments`. Empty (the default)
    /// means the record→`.id` projection stays purely shape-directed,
    /// which is what it was before the table existed.
    pub route_id_segments: Option<&'a std::collections::HashMap<String, Vec<bool>>>,
}

pub fn lower_controllers_with_arel_views_assocs_and_routes(
    controllers: &[Controller],
    extras: Vec<(ClassId, crate::analyze::ClassInfo)>,
    opts: LowerControllerOptions,
) -> Vec<LibraryClass> {
    let LowerControllerOptions {
        schema,
        views,
        library_classes,
        assocs,
        routed_by_controller,
        format_breadth,
        route_id_segments,
    } = opts;
    // `None` (every wrapper's default) means the projection stays
    // purely shape-directed — what it was before this table existed.
    let empty_segments = std::collections::HashMap::new();
    let route_id_segments = route_id_segments.unwrap_or(&empty_segments);
    // Scan source-shape action bodies for `permit(...)` declarations.
    // Each unique resource yields one `<Resource>Params` synthesized
    // class plus the (resource, fields, class_id) record we need to
    // rewrite controller bodies + register the class with the typer.
    let params_specs = self::params::collect_specs(controllers);
    let params_lcs = self::params::synthesize_params_classes(&params_specs);

    // The view↔controller ivar contract: each action view's read-ivars,
    // so the render rewrite passes `@<name>` for each (matching the view's
    // generated parameter list). See view_to_library::action_view_ivar_map.
    let view_ivars = crate::lower::view_to_library::action_view_ivar_map(views, controllers);
    // Controller-side partial renders (`render partial: "commentbox",
    // locals: {…}`) bind against the partial's def-site parameter order.
    let partials: PartialMap =
        crate::lower::view_to_library::partial_call_contracts(views, controllers, library_classes);

    let mut all_methods: Vec<(Vec<MethodDef>, &Controller)> = Vec::new();
    for controller in controllers {
        let json_actions = json_actions_for(controller, views);
        let turbo_stream_actions = turbo_stream_actions_for(controller, views);
        // `Some(map)` → this controller's routed actions (empty set if it
        // has no routes, e.g. a base controller → all publics are helpers).
        // `None` → legacy: every public method is an action.
        let routed = routed_by_controller
            .map(|m| m.get(&controller.name).cloned().unwrap_or_default());
        let methods = build_methods(controller, controllers, &params_specs, &json_actions, &turbo_stream_actions, routed.as_ref(), &view_ivars, &partials, format_breadth, route_id_segments);
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

    // Jbuilder LCs (`Views::Articles.<action>_json`). The
    // respond_to-flattener inserts Sends to these into action bodies;
    // without them in the typing registry the body-typer leaves the
    // calls as TyVar and the typing-residual gate trips. Registered
    // AFTER `extras` so the existing `Views::Articles` entry (from
    // view_to_library, with `show`/`index`/`new`/`edit`) gets the
    // `_json` siblings merged in rather than overwritten.
    let app_stub = crate::App::new();
    for lc in crate::lower::lower_jbuilder_to_library_classes(views, &app_stub, Vec::new()) {
        let info = classes.entry(lc.name.clone()).or_default();
        for m in &lc.methods {
            if let Some(sig) = &m.signature {
                if matches!(m.receiver, MethodReceiver::Class) {
                    info.class_methods.insert(m.name.clone(), sig.clone());
                    info.class_method_kinds.insert(m.name.clone(), m.kind);
                } else {
                    info.instance_methods.insert(m.name.clone(), sig.clone());
                    info.instance_method_kinds.insert(m.name.clone(), m.kind);
                }
            }
        }
        // Last-segment alias for the typer's bare-Const resolver.
        let raw = lc.name.0.as_str();
        let last = raw.rsplit("::").next().unwrap_or(raw).to_string();
        if last != raw {
            let alias_id = ClassId(Symbol::from(last));
            let entry = classes.entry(alias_id).or_default();
            for m in &lc.methods {
                if let Some(sig) = &m.signature {
                    if matches!(m.receiver, MethodReceiver::Class) {
                        entry.class_methods.insert(m.name.clone(), sig.clone());
                        entry.class_method_kinds.insert(m.name.clone(), m.kind);
                    } else {
                        entry.instance_methods.insert(m.name.clone(), sig.clone());
                        entry.instance_method_kinds.insert(m.name.clone(), m.kind);
                    }
                }
            }
        }
    }

    // Ivar bindings: `@params` is framework-guaranteed (the lowerer
    // itself rewrites bare `params` → `@params` in action bodies, so
    // every controller has it; the dispatcher sets it to the raw
    // request-parsed Hash). This isn't a naming heuristic — it's a
    // fact about the framework that the lowerer KNOWS because it
    // produced the @params reference.
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
            key: Box::new(Ty::Str),
            value: Box::new(Ty::Untyped),
        },
    );
    // `@flash` is also framework-guaranteed: the render-rewrite emits
    // `@flash[:notice]` / `@flash[:alert]` as args to every Views
    // call, and `redirect_to`'s lowering writes `@flash[:notice] = …`
    // when the source had `notice: …`. Per Phase 2.5(b) the runtime
    // class is `ActionDispatch::Flash` (typed `notice`/`alert` fields
    // + HWIA-shape shims); type `@flash` as Flash so `@flash[:k]`
    // routes through `Flash#[]` for typed targets.
    framework_ivars.insert(
        Symbol::from("flash"),
        Ty::Class {
            id: ClassId(Symbol::from("ActionDispatch::Flash")),
            args: vec![],
        },
    );
    // `@session` ditto — per Phase 2.5(b), typed as the per-app
    // ActionDispatch::Session struct (empty for real-blog, HWIA-shape
    // shims preserved on the class for cross-target tests).
    framework_ivars.insert(
        Symbol::from("session"),
        Ty::Class {
            id: ClassId(Symbol::from("ActionDispatch::Session")),
            args: vec![],
        },
    );

    let mut out = Vec::new();
    for (mut methods, controller) in all_methods {
        for method in &mut methods {
            crate::lower::typing::type_method_body(method, &classes, &framework_ivars);
            // Stage 3: now that bodies are typed, rewrite
            // `<typed-params>[:field]` → `<typed-params>.field`.
            // Re-type after the rewrite so the synthesized
            // attr_reader Send carries its return type and any
            // chained dispatch picks up the concrete `Str`.
            method.body = self::params::rewrite_typed_bracket_to_field(
                &method.body, &params_specs,
            );
            crate::lower::typing::type_method_body(method, &classes, &framework_ivars);
            // `broadcast_prepend_to user, :rooms, target: [@room, :list],
            // partial: …` — the Rails broadcast API written from a
            // CONTROLLER, a home `lower::broadcast_calls` (models and
            // the concerns beside them) never visits. Emitted verbatim
            // until now, i.e. an undefined method: those actions raise
            // in a real server, not only under test.
            //
            // HERE, in the typed loop, and not in `lower_action_body`:
            // the model-side rewriter resolves its record from a
            // `belongs_to` on the owning model, and a controller has no
            // owner — what it has is the analyzer's `Ty::Class` stamp on
            // `user` / `@room`, which does not exist until
            // `type_method_body` has run. Placed before the arel pass
            // for the same reason its neighbours are: the pass re-types
            // afterwards, so the synthesized `Views::…` payload and
            // `<record>.id` reads carry their types downstream.
            method.body = self::broadcasts::rewrite_broadcast_to(
                &method.body,
                views_module_name(controller).as_deref(),
                &partials,
            );
            crate::lower::typing::type_method_body(method, &classes, &framework_ivars);
            // Arel pass — when schema is provided, lift recognized
            // AR call chains into inline SELECT/hydrate over the Db
            // primitive surface. Re-type after so the body-typer's
            // earlier annotations on the rewritten subtree refresh.
            if let Some(schema) = schema {
                crate::lower::arel::rewrite_arel_in_expr_with_assocs(
                    &mut method.body, schema, &classes, assocs,
                );
                crate::lower::typing::type_method_body(method, &classes, &framework_ivars);
            }
        }
        out.push(LibraryClass {
            name: controller.name.clone(),
            is_module: false,
            parent: controller.parent.clone(),
            includes: Vec::new(),
            methods,
            nullable_columns: Vec::new(),
            origin: None,
            constants: collect_class_constants(controller),
            unknown_calls: Vec::new(),
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
                // The companion presence slot each value slot carries.
                params_ivars.insert(self::params::provided_field(f), Ty::Bool);
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
    // No view list in this single-controller path → empty map; the render
    // rewrite falls back to in-scope ivars (legacy behavior for tests).
    let view_ivars: ViewIvarMap = std::collections::HashMap::new();
    let partials: PartialMap = std::collections::HashMap::new();
    let methods = build_methods(
        controller,
        std::slice::from_ref(controller),
        &specs,
        &std::collections::HashSet::new(),
        &std::collections::HashSet::new(),
        None,
        &view_ivars,
        &partials,
        FormatBreadth::NARROW,
        &std::collections::HashMap::new(),
    );
    LibraryClass {
        name: controller.name.clone(),
        is_module: false,
        parent: controller.parent.clone(),
        includes: Vec::new(),
        methods,
        nullable_columns: Vec::new(),
        origin: None,
        constants: collect_class_constants(controller),
        unknown_calls: Vec::new(),
    }
}

/// Collect class-level constant definitions (`NAME = <expr>`) from a
/// controller body. They ride in as `Unknown` items wrapping an `Assign`
/// to a single-segment `Const` lvalue; everything else (filters,
/// `caches_page`, …) stays dropped. Carried onto the `LibraryClass` so
/// refs like `ApplicationController::TAG_FILTER_COOKIE` resolve.
fn collect_class_constants(controller: &Controller) -> Vec<(Symbol, Expr)> {
    let mut out = Vec::new();
    for item in &controller.body {
        let ControllerBodyItem::Unknown { expr, .. } = item else { continue };
        if let ExprNode::Assign {
            target: crate::expr::LValue::Const { path },
            value,
        } = &*expr.node
        {
            if let [name] = path.as_slice() {
                out.push((name.clone(), value.clone()));
            }
        }
    }
    out
}

fn build_methods(
    controller: &Controller,
    all_controllers: &[Controller],
    params_specs: &ParamsSpecs,
    json_actions: &std::collections::HashSet<Symbol>,
    turbo_stream_actions: &std::collections::HashSet<Symbol>,
    routed: Option<&std::collections::HashSet<Symbol>>,
    view_ivars: &ViewIvarMap,
    partials: &PartialMap,
    // respond_to BREADTH — format.rss branches + inline `render json:`
    // preserved under the request_format dispatch. ONLY the CRuby/JRuby
    // trees pass true: the widened arms call the CRuby-overlay
    // JsonRender, which the spinel AOT compile (same emit family,
    // routed-aware too) cannot resolve. Everyone else keeps the narrow
    // html(+simple-json) flatten, emit unchanged.
    format_breadth: FormatBreadth,
    route_id_segments: &std::collections::HashMap<String, Vec<bool>>,
) -> Vec<MethodDef> {
    let mut methods: Vec<MethodDef> = Vec::new();

    // Names this controller's ancestry DEFINES that the route-helper
    // rewrite would otherwise claim by suffix alone.
    let shadows = route_helper_shadows(controller, all_controllers);

    let (publics_all, privs) = split_public_private_actions(controller);
    // Params helpers resolve through Ruby's MRO: `Rooms::OpensController`
    // writes `@room.update! room_params`, and `room_params` is defined on
    // `RoomsController`. The helper→spec map behind
    // `rewrite_update_to_typed_variant` / `rewrite_model_new_to_from_params`
    // sees only the actions handed to it, so a subclass call site matched
    // nothing and kept a shape whose argument type no longer fit.
    //
    // Deliberately NOT folded into `privs`: that list is also what gets
    // EMITTED (`privs_kept`) and what filter inlining resolves against,
    // so merging inherited helpers there would emit a second copy of
    // `room_params` on every subclass. Own actions first (a subclass may
    // override), then ancestors nearest-first, deduped by name — Ruby's
    // own lookup order.
    let params_privs: Vec<Action> = {
        let chain = ancestor_chain(controller, all_controllers);
        let mut seen: std::collections::HashSet<Symbol> =
            privs.iter().map(|a| a.name.clone()).collect();
        let mut out = privs.clone();
        for c in chain.iter().rev() {
            let (pubs, ancestor_privs) = split_public_private_actions(c);
            for a in ancestor_privs.iter().chain(pubs.iter()) {
                if a.name.as_str().ends_with("_params") && seen.insert(a.name.clone()) {
                    out.push(a.clone());
                }
            }
        }
        out
    };
    // With route info, a public method is a routable action only if a route
    // reaches it; the rest are helper/filter methods that must keep their
    // return value (no synthesized render) — emitted like privates. Without
    // route info, every public is an action (legacy behavior).
    let (publics, helper_publics): (Vec<Action>, Vec<Action>) = match routed {
        Some(set) => publics_all.into_iter().partition(|a| set.contains(&a.name)),
        None => (publics_all, Vec::new()),
    };
    let before_filters: Vec<&Filter> = controller
        .filters()
        .filter(|f| matches!(f.kind, FilterKind::Before))
        .collect();

    // Inline before_action filter bodies into each action that
    // fires them. This pushes the assignment to `@article` (etc) into
    // the action body, where the body-typer's Seq walk picks it up
    // and types subsequent reads correctly. Self-describing IR — no
    // convention-based ivar naming heuristic needed downstream.
    // Order safety is decided ONCE per controller — see
    // `own_filter_inlining_is_ordered`. When it declines, every own
    // filter falls through to the preamble, which emits them in
    // declaration order.
    let inlining_ordered = own_filter_inlining_is_ordered(controller, &privs);
    let resolve_own = |name: &Symbol| {
        privs
            .iter()
            .chain(publics.iter())
            .find(|a| &a.name == name)
            .map(|a| a.body.clone())
    };
    let publics_inlined: Vec<Action> = publics
        .iter()
        .map(|a| {
            if inlining_ordered {
                inline_before_filters(a, &before_filters, &privs, &resolve_own)
            } else {
                a.clone()
            }
        })
        .collect();

    // Filter targets that are PURELY filter targets (called only via
    // before_action, never from an action body) are dead after
    // inlining — drop them from the emitted methods. Filter targets
    // that are also called from action bodies (e.g., `_params`
    // helpers — actually those don't appear in before_filters, but
    // be defensive) stay.
    let filter_target_names: std::collections::HashSet<&Symbol> =
        before_filters.iter().map(|f| &f.target).collect();
    // A subclassed controller keeps its filter targets: descendants
    // inherit the before_action and their preambles call the target
    // BY NAME (`default_periods` in ModNotesController's dispatcher,
    // defined on ModController) — inline-and-drop would leave those
    // calls dangling.
    let has_descendants = all_controllers
        .iter()
        .any(|c| ancestor_chain(c, all_controllers).iter().any(|p| p.name == controller.name));
    let privs_kept: Vec<Action> = privs
        .iter()
        .filter(|a| {
            // A target is dead only if inlining actually consumed it.
            // With inlining declined the preamble CALLS it by name.
            has_descendants || !inlining_ordered || !filter_target_names.contains(&a.name)
        })
        .cloned()
        .collect();

    // Actions this controller reaches only through its parent. Rails
    // dispatches them; the `case` in `synthesize_process_action` has to
    // carry an arm or they fall off the end and answer 200. Nearest
    // ancestor wins, and anything this controller defines itself (public
    // OR private — a private override is still the method Ruby finds)
    // is not inherited.
    let own_names: std::collections::HashSet<Symbol> =
        controller.actions().map(|a| a.name.clone()).collect();
    // The ROUTES decide, not visibility. "Public" in this IR means
    // "declared above the `private` marker", and lobsters'
    // ApplicationController declares `authenticate_user`,
    // `require_logged_in_user` and a dozen other FILTER TARGETS that
    // way — gating on visibility put a dispatch arm for every one of
    // them into 25 controllers. An action is what the router can send
    // here; anything else is a method that happens to be public.
    //
    // `routed` is this controller's routed action names, already
    // plumbed here for the format dispatch. `None` means the caller had
    // no route table, and then nothing is inherited — declining is the
    // previous behavior, and guessing without the router is what put
    // `when :authenticate_user` in 25 files.
    let mut inherited: Vec<Symbol> = Vec::new();
    if let Some(routed) = routed {
        let mut seen: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
        for ancestor in ancestor_chain(controller, all_controllers).iter().rev() {
            let (ancestor_pubs, _) = split_public_private_actions(ancestor);
            for a in ancestor_pubs {
                if !routed.contains(&a.name)
                    || own_names.contains(&a.name)
                    || !seen.insert(a.name.clone())
                {
                    continue;
                }
                inherited.push(a.name.clone());
            }
        }
    }
    inherited.sort_by(|a, b| a.as_str().cmp(b.as_str()));

    // Actions whose default render belongs in the dispatcher because a
    // subclass reaches this body with `super`. Empty for every
    // controller nobody subclasses that way, which is all of them
    // outside campfire's one bot controller — and an empty set leaves
    // both the bodies and the dispatcher byte-identical.
    let deferred_renders = actions_reached_by_super(controller, all_controllers);
    let mut pending_dispatcher: Option<Vec<PreambleStmt>> = None;

    if !publics_inlined.is_empty() || !inherited.is_empty() {
        // The before_action preamble: everything the body-inlining above
        // can't reach — inherited filters (ApplicationController's
        // `authenticate_user` firing for subclass actions), own filters
        // whose targets are defined on an ancestor, and block-form
        // filters. Same-controller private-target filters stay inlined
        // (typing seeds the action bodies); a controller with none of the
        // former gets an empty preamble and a byte-identical dispatcher.
        let preamble = build_filter_preamble(
            controller,
            all_controllers,
            &privs,
            /*own_privs_inlined=*/ inlining_ordered,
        );
        pending_dispatcher = Some(preamble);
    }

    // Actions BEFORE the dispatcher: a deferred action hands its
    // synthesized tail back here, and the dispatcher needs it. The
    // dispatcher is spliced in at index 0 afterwards so the emitted
    // method order is unchanged.
    let dispatcher_at = methods.len();
    let mut deferred_tails: std::collections::HashMap<Symbol, Expr> =
        std::collections::HashMap::new();
    for a in &publics_inlined {
        methods.push(action_to_method(
            a, controller, all_controllers, &privs, &params_privs, /*is_public=*/ true,
            params_specs, json_actions,
            turbo_stream_actions, view_ivars,
            partials, format_breadth, &shadows, route_id_segments, &deferred_renders,
            &mut deferred_tails,
        ));
    }
    if let Some(preamble) = pending_dispatcher {
        methods.insert(
            dispatcher_at,
            synthesize_process_action(
                &preamble,
                &publics_inlined,
                &inherited,
                controller.name.0.clone(),
                &deferred_tails,
            ),
        );
    }
    // The empty set for both non-public loops below: `is_public: false`
    // already suppresses the implicit render, so there is never one to
    // defer.
    let no_deferred: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
    for a in &privs_kept {
        methods.push(action_to_method(
            a, controller, all_controllers, &privs, &params_privs, /*is_public=*/ false,
            params_specs, json_actions,
            turbo_stream_actions, view_ivars,
            partials, format_breadth, &shadows, route_id_segments, &no_deferred,
            &mut std::collections::HashMap::new(),
        ));
    }
    // Public methods no route reaches are helpers/filters, not actions:
    // emit them verbatim (no implicit render) so callers see their real
    // return value. (Whether before_action auto-runs them is handled by
    // the filter-chain work, not here.)
    for a in &helper_publics {
        methods.push(action_to_method(
            a, controller, all_controllers, &privs, &params_privs, /*is_public=*/ false,
            params_specs, json_actions,
            turbo_stream_actions, view_ivars,
            partials, format_breadth, &shadows, route_id_segments, &no_deferred,
            &mut std::collections::HashMap::new(),
        ));
    }

    // `helper_method :name` exposes a controller method to templates.
    // The lowered views are module functions with no controller
    // instance, so each ARG-PURE marked method (no ivar reads — the
    // corpus members take the record as a parameter) also gets a
    // class-side clone; the bare view call rewrites to
    // `DomainsController.caption_of_button(domain)` via
    // helper_method_index (registered at ingest). Ivar-reading marked
    // methods stay instance-only — their view calls remain honest
    // residue.
    for name in controller_helper_method_names(controller) {
        if let Some(m) = methods
            .iter()
            .find(|m| m.name == name && m.receiver == MethodReceiver::Instance)
        {
            let mut clone = m.clone();
            clone.receiver = MethodReceiver::Class;
            methods.push(clone);
        }
    }

    methods
}

/// Names a controller marks with `helper_method :x` whose public
/// method body is IVAR-FREE (pure over its arguments) — the set the
/// view-call rewrite and the class-side clone above serve. Shared with
/// ingest, which registers these in `app.helper_method_index`.
pub(crate) fn controller_helper_method_names(controller: &Controller) -> Vec<Symbol> {
    use crate::expr::Literal;

    fn has_ivar(e: &Expr) -> bool {
        if matches!(&*e.node, ExprNode::Ivar { .. }) {
            return true;
        }
        let mut found = false;
        e.node.for_each_child(&mut |c| {
            if has_ivar(c) {
                found = true;
            }
        });
        found
    }

    let mut marked: Vec<Symbol> = Vec::new();
    for item in &controller.body {
        let ControllerBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
            continue;
        };
        if method.as_str() != "helper_method" {
            continue;
        }
        for arg in args {
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*arg.node {
                marked.push(value.clone());
            }
        }
    }
    marked.retain(|name| {
        controller.actions().any(|a| {
            a.name == *name
                && !has_ivar(&a.body)
                && a.opt_params.iter().all(|(_, d)| !has_ivar(d))
        })
    });
    marked
}

/// Does `f` apply to the action named `action_name`?
fn filter_applies(f: &Filter, action_name: &Symbol) -> bool {
    if !f.only.is_empty() {
        f.only.contains(action_name)
    } else if !f.except.is_empty() {
        !f.except.contains(action_name)
    } else {
        true
    }
}

/// May this controller's own filters be inlined into its action bodies
/// at all?
///
/// Inlining moves a filter INTO the action, and the preamble runs
/// BEFORE the action — so an inlined filter always runs after every
/// preamble one, whatever the source said. That is fine while all of a
/// controller's own inlinable filters are declared before its
/// non-inlinable ones, and wrong the moment they are not.
///
/// campfire's RoomsController is the counter-example that made this a
/// bug rather than a theory:
///
/// ```ruby
/// before_action :set_room,                  only: %i[ show destroy ]
/// before_action :ensure_can_administer,     only: %i[ destroy ]
/// before_action :remember_last_room_visited, only: :show
/// ```
///
/// The first two target this controller's own private methods and were
/// inlined; the third's target lives on ApplicationController (a
/// concern included there), so it stayed in the preamble and ran FIRST
/// — reading `@room` before `set_room` had set it, and dying on nil.
///
/// The judgment is per-CONTROLLER, not per-action: the preamble is one
/// statement list shared by every action, so "inline for `show` but not
/// for `destroy`" would need the preamble's `only:`/`except:` scoping
/// rewritten per filter. Declining wholesale costs the ivar typing that
/// inlining seeds (`@room` reads as untyped again) and nothing else —
/// the preamble emits the same calls in declaration order, with the
/// halt checks it already builds.
///
/// ANCESTORS' filters are not consulted: Rails runs them before the
/// controller's own, which is exactly where the preamble puts them.
fn own_filter_inlining_is_ordered(controller: &Controller, privs: &[Action]) -> bool {
    let own_priv = |target: &Symbol| privs.iter().any(|a| &a.name == target);
    let mut seen_inlinable = false;
    for f in controller.filters().filter(|f| matches!(f.kind, FilterKind::Before)) {
        if own_priv(&f.target) {
            seen_inlinable = true;
        } else if seen_inlinable {
            return false;
        }
    }
    true
}

/// Return a copy of `action` with every applicable before_action
/// filter target's body prepended to the action body. A filter
/// applies when its `only:` includes the action name, or its
/// `except:` doesn't, or it has neither (unconditional).
///
/// A prepended body that can render/redirect/head is followed by
/// `return if performed?`, exactly as the preamble does for the filters
/// it owns. Rails halts the chain on a filter that responds, and an
/// inlined one had been losing that: campfire's `ensure_can_administer`
/// emitted `head(:forbidden)` and then `@room.destroy` ran anyway, so a
/// non-administrator got a 403 AND the room was deleted.
fn inline_before_filters(
    action: &Action,
    filters: &[&Filter],
    privs: &[Action],
    resolve: &dyn Fn(&Symbol) -> Option<Expr>,
) -> Action {
    let action_name = &action.name;
    let mut prepended: Vec<Expr> = Vec::new();
    for f in filters {
        if !filter_applies(f, action_name) {
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
        // Same resolver-following check the preamble uses, so a filter
        // that delegates its redirect still halts.
        if can_respond_within(&target.body, resolve, &mut std::collections::BTreeSet::new()) {
            prepended.push(halt_if_performed());
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
    // The combined Seq wraps this action's statements (plus prepended
    // filter statements that carry their own spans) — it attributes to
    // the action body it was derived from.
    new_action.body = Expr::new(
        action.body.span,
        ExprNode::Seq { exprs: combined },
    );
    new_action
}

/// Assemble the before_action preamble for `controller`'s dispatcher —
/// the filters `inline_before_filters` can't reach. Order matches Rails:
/// ancestors' filters first (root-most ancestor first), then the
/// controller's own, each set in declaration order. Covered here:
///
///   - inherited filters (declared on an ancestor — ApplicationController's
///     `before_action :authenticate_user` firing for subclass actions);
///   - own filters whose target method is defined on an ancestor
///     (`before_action :require_logged_in_user, only: [...]` where the
///     method lives on ApplicationController);
///   - own block-form filters (`before_action { @page = page }`), read
///     from the Unknown body item the ingester round-trips them as.
///
/// Own filters whose targets are this controller's private methods are
/// excluded — those are inlined into the action bodies upstream (the
/// body-typer seeds ivar types from them), so a controller with only
/// those (the blog shape) gets an empty preamble and a byte-identical
/// dispatcher. `skip_before_action` targets anywhere in the chain drop
/// the matching filter. A filter naming a method that resolves nowhere
/// in the chain (a framework built-in like `verify_authenticity_token`)
/// is dropped, matching the previous silently-skipped behavior.
fn build_filter_preamble(
    controller: &Controller,
    all_controllers: &[Controller],
    own_privs: &[Action],
    own_privs_inlined: bool,
) -> Vec<PreambleStmt> {
    let chain = ancestor_chain(controller, all_controllers);

    // Skips, kept whole rather than reduced to a set of names: a skip
    // carries `only:`/`except:` of its own, and campfire's
    // `allow_unauthenticated_access only: %i[new create]` means the
    // filter still runs everywhere else. Dropping the filter outright
    // would sign every other action out of its own authentication —
    // `destroy` (sign OUT) among them.
    let skips: Vec<&Filter> = chain
        .iter()
        .copied()
        .chain(std::iter::once(controller))
        .flat_map(|c| c.filters())
        .filter(|f| matches!(f.kind, FilterKind::Skip))
        .collect();

    // Resolve a filter target's body — self first, then nearest ancestor
    // (Ruby method resolution order) — so the halting check can be
    // scoped to filters that can actually render/redirect.
    let find_target = |name: &Symbol| -> Option<Action> {
        let mut scopes: Vec<&Controller> = vec![controller];
        scopes.extend(chain.iter().rev().copied());
        for c in scopes {
            let (pubs, privs) = split_public_private_actions(c);
            if let Some(a) = privs.iter().chain(pubs.iter()).find(|a| &a.name == name) {
                return Some(a.clone());
            }
        }
        None
    };

    let mut preamble: Vec<PreambleStmt> = Vec::new();
    let push_call = |f: &Filter, preamble: &mut Vec<PreambleStmt>| {
        // Apply every skip naming this target. An unscoped skip removes
        // the filter; a scoped one narrows where it still runs, which
        // the dispatcher already enforces per action (`filter_dispatch_stmt`
        // emits the only/except guard).
        let mut f = f.clone();
        for skip in skips.iter().filter(|s| s.target == f.target) {
            if skip.only.is_empty() && skip.except.is_empty() {
                return;
            }
            for a in &skip.only {
                if !f.except.contains(a) {
                    f.except.push(a.clone());
                }
            }
            // `skip … except: [:x]` skips everywhere BUT x, so what
            // survives runs only for x.
            if !skip.except.is_empty() {
                f.only = if f.only.is_empty() {
                    skip.except.clone()
                } else {
                    f.only.iter().filter(|a| skip.except.contains(a)).cloned().collect()
                };
                if f.only.is_empty() {
                    return;
                }
            }
        }
        let f = &f;
        let Some(target) = find_target(&f.target) else {
            return;
        };
        preamble.push(PreambleStmt::Call {
            filter: f.clone(),
            // Follows receiverless calls into the same controller's own
            // methods — `find_target` is the resolver the chain already
            // uses, so a filter that delegates its redirect (campfire's
            // `require_authentication`) still halts the chain.
            halt_check: can_respond_within(
                &target.body,
                &|name| find_target(name).map(|a| a.body),
                &mut std::collections::BTreeSet::new(),
            ),
        });
    };

    for anc in &chain {
        for f in anc.filters().filter(|f| matches!(f.kind, FilterKind::Before)) {
            push_call(f, &mut preamble);
        }
    }
    let own_priv_targets: std::collections::HashSet<&Symbol> =
        own_privs.iter().map(|a| &a.name).collect();
    for item in &controller.body {
        match item {
            ControllerBodyItem::Filter { filter, .. }
                if matches!(filter.kind, FilterKind::Before) =>
            {
                if own_privs_inlined && own_priv_targets.contains(&filter.target) {
                    continue; // inlined into action bodies upstream
                }
                push_call(filter, &mut preamble);
            }
            ControllerBodyItem::Unknown { expr, .. } => {
                if let Some((body, only, except)) = block_form_filter(expr) {
                    let halt_check = can_respond(&body);
                    preamble.push(PreambleStmt::Block { body, only, except, halt_check });
                }
            }
            _ => {}
        }
    }
    preamble
}

/// Method names ending in `_path` / `_url` that this controller or one
/// of its ancestors DEFINES — the names `rewrite_route_helpers` must not
/// treat as route helpers. Rails injects route helpers by module
/// inclusion, so a `def` anywhere in the ancestry wins over them; the
/// suffix heuristic alone does not know that. campfire's
/// `post_authenticating_url` (a private method on the Authentication
/// concern, spliced into ApplicationController) and `logo_path` are the
/// corpus members that made this visible.
fn route_helper_shadows(
    controller: &Controller,
    all: &[Controller],
) -> std::collections::HashSet<Symbol> {
    ancestor_chain(controller, all)
        .into_iter()
        .chain(std::iter::once(controller))
        .flat_map(|c| c.body.iter())
        .filter_map(|item| match item {
            ControllerBodyItem::Action { action, .. } => Some(&action.name),
            _ => None,
        })
        .filter(|n| n.as_str().ends_with("_path") || n.as_str().ends_with("_url"))
        .cloned()
        .collect()
}

/// Walk `parent` links root-first (`[ApplicationController]` for a
/// typical leaf controller). A parent that isn't among the ingested
/// controllers (ActionController::Base) ends the walk; the depth cap
/// guards against parent cycles.
/// `render formats: :svg` → `render :<action>, format: :svg`.
///
/// Rails' `formats:` names a format with no template: "render THIS
/// request's template in that format". The internal shape every
/// downstream pass already understands is a symbol template plus the
/// singular `format:` marker, which `rewrite_render_to_views` binds to
/// `Views::…show_svg` and `mime_for_format` tags with the right MIME. So
/// this only has to supply the template NAME.
///
/// Which name is the whole difficulty. campfire writes it inside a
/// PRIVATE helper — `render_initials`, called from `show` — so the
/// enclosing method's own name is not the template. The rule is the
/// UNIQUE CALLER: if exactly one of the controller's public actions
/// calls this helper, that action's template is the one Rails would
/// have rendered. Zero callers or several and it declines, because then
/// the answer genuinely depends on the request and guessing it would
/// bind a template the app never asked for.
///
/// A PUBLIC action writing `render formats:` is its own template, which
/// falls out of the same rule with no special case.
fn formats_only_render_template(
    controller: &Controller,
    method_name: &Symbol,
    is_public: bool,
) -> Option<Symbol> {
    if is_public {
        return Some(method_name.clone());
    }
    let mut callers = controller
        .actions()
        .filter(|a| &a.name != method_name && body_calls_method(&a.body, method_name));
    let first = callers.next()?;
    if callers.next().is_some() {
        return None;
    }
    Some(first.name.clone())
}

/// Does this body call `name` with no receiver (or on `self`)?
fn body_calls_method(body: &Expr, name: &Symbol) -> bool {
    let hit = matches!(
        &*body.node,
        ExprNode::Send { recv, method, .. }
            if method == name
                && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
    );
    if hit {
        return true;
    }
    let mut found = false;
    body.node.for_each_child(&mut |c| {
        if !found && body_calls_method(c, name) {
            found = true;
        }
    });
    found
}

/// Rewrite a receiverless `render(formats: :X)` — and nothing else in
/// the call — to `render(:<template>, format: :X)`.
fn resolve_formats_only_render(e: &mut Expr, template: &Symbol) {
    e.node.for_each_child_mut(&mut |c| resolve_formats_only_render(c, template));
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &mut *e.node else {
        return;
    };
    if method.as_str() != "render" || args.len() != 1 {
        return;
    }
    let ExprNode::Hash { entries, kwargs: true } = &*args[0].node else { return };
    let [(k, v)] = &entries[..] else { return };
    let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { return };
    if key.as_str() != "formats" {
        return;
    }
    let ExprNode::Lit { value: Literal::Sym { value: fmt } } = &*v.node else { return };
    let span = e.span;
    let template_arg = Expr::new(
        span,
        ExprNode::Lit { value: Literal::Sym { value: template.clone() } },
    );
    let format_kwargs = Expr::new(
        span,
        ExprNode::Hash {
            entries: vec![(
                Expr::new(
                    span,
                    ExprNode::Lit { value: Literal::Sym { value: Symbol::from("format") } },
                ),
                Expr::new(
                    span,
                    ExprNode::Lit { value: Literal::Sym { value: fmt.clone() } },
                ),
            )],
            kwargs: true,
        },
    );
    *args = vec![template_arg, format_kwargs];
}

/// The params spec an OVERRIDING `<x>_params` helper should yield —
/// the one an ancestor's helper of the same name declares.
///
/// Only consulted when THIS controller's own body declares no permit
/// list for that helper: a subclass that writes a full
/// `require(:r).permit(...)` chain has its own spec and needs nothing
/// from its parent.
fn inherited_params_spec<'a>(
    controller: &Controller,
    all: &[Controller],
    helper: &Symbol,
    specs: &'a ParamsSpecs,
) -> Option<&'a ParamsSpec> {
    let own: Vec<crate::dialect::Action> = controller.actions().cloned().collect();
    if helper_spec_map(&own, specs).contains_key(helper) {
        return None;
    }
    for ancestor in ancestor_chain(controller, all).iter().rev() {
        let acts: Vec<crate::dialect::Action> = ancestor.actions().cloned().collect();
        if let Some(spec) = helper_spec_map(&acts, specs).get(helper) {
            return Some(*spec);
        }
    }
    None
}

/// Actions of `controller` that a SUBCLASS overrides and reaches with
/// `super` — the ones whose implicit render must run in the DISPATCHER
/// rather than at the end of the action body.
///
/// Rails runs the default render AFTER the action returns
/// (`send_action` then `default_render` unless `performed?`), and we
/// inline it into the body instead. Those are the same thing until a
/// subclass writes
///
/// ```text
/// def create
///   super          # parent body runs...
///   head :created  # ...and THIS is the response
/// end
/// ```
///
/// where the inlined version fires the parent's default render while
/// `super` is still on the stack — before the subclass can respond at
/// all. campfire's `Messages::ByBotsController#create` is exactly that,
/// and every bot message died on `ActionView::MissingTemplate` for an
/// HTML request the subclass was about to answer with `head :created`.
///
/// DEMAND-GATED, and the demand is tiny: lobsters has no `super` in any
/// action, campfire has this one. A controller nobody subclasses this
/// way keeps a byte-identical body and dispatcher.
fn actions_reached_by_super(
    controller: &Controller,
    all: &[Controller],
) -> std::collections::HashSet<Symbol> {
    let mut out = std::collections::HashSet::new();
    let defines = |name: &Symbol| controller.actions().any(|a| &a.name == name);
    for sub in all {
        if sub.name == controller.name {
            continue;
        }
        if !ancestor_chain(sub, all).iter().any(|c| c.name == controller.name) {
            continue;
        }
        for a in sub.actions() {
            if body_calls_super(&a.body) && defines(&a.name) {
                out.insert(a.name.clone());
            }
        }
    }
    out
}

/// Does this body reach `super` anywhere? Not just at top level — the
/// campfire case is a bare `super` as the first statement, but a
/// `super` inside a conditional reaches the parent body just the same,
/// and the question here is only "can the parent's body run as a
/// callee", which any occurrence answers.
fn body_calls_super(body: &Expr) -> bool {
    if matches!(&*body.node, ExprNode::Super { .. }) {
        return true;
    }
    let mut found = false;
    body.node.for_each_child(&mut |c| {
        if !found && body_calls_super(c) {
            found = true;
        }
    });
    found
}

fn ancestor_chain<'a>(controller: &Controller, all: &'a [Controller]) -> Vec<&'a Controller> {
    let mut chain: Vec<&'a Controller> = Vec::new();
    let mut cur = controller.parent.as_ref();
    while let Some(pid) = cur {
        if chain.len() >= 8 {
            break;
        }
        let Some(p) = all.iter().find(|c| &c.name == pid) else { break };
        chain.push(p);
        cur = p.parent.as_ref();
    }
    chain.reverse();
    chain
}

/// Does this filter body contain a respond-capable call (render /
/// redirect_to / head / render_404)? Scopes the `return if performed?`
/// halting check to filters that need it — pure-assignment filters
/// (and every blog controller) add no dispatch noise.
fn can_respond(body: &Expr) -> bool {
    can_respond_within(body, &|_| None, &mut std::collections::BTreeSet::new())
}

/// Whether this body can render/redirect/head — DIRECTLY, or through a
/// method it calls that can.
///
/// The one-level answer is not enough for the shape Rails apps actually
/// write: campfire's `require_authentication` is
/// `restore_authentication || bot_authentication || request_authentication`,
/// and only that third method redirects. Reading one level deep said the
/// filter cannot respond, so the preamble emitted no `return if
/// performed?` after it and every later filter — and then the action —
/// ran on an unauthenticated request that had already been sent a
/// redirect.
///
/// `resolve` maps a receiverless call to the body it names, when the
/// controller (or an ancestor) defines it; `seen` stops a cycle. Only
/// receiverless sends are followed: a call on another object is that
/// object's business, and Rails' halting is about THIS controller's
/// filter chain.
fn can_respond_within(
    body: &Expr,
    resolve: &dyn Fn(&Symbol) -> Option<Expr>,
    seen: &mut std::collections::BTreeSet<Symbol>,
) -> bool {
    fn walk(
        e: &Expr,
        found: &mut bool,
        resolve: &dyn Fn(&Symbol) -> Option<Expr>,
        seen: &mut std::collections::BTreeSet<Symbol>,
    ) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv, method, .. } = &*e.node {
            if matches!(method.as_str(), "render" | "redirect_to" | "head" | "render_404") {
                *found = true;
                return;
            }
            let self_call = match recv {
                None => true,
                Some(r) => matches!(&*r.node, ExprNode::SelfRef),
            };
            if self_call && seen.insert(method.clone()) {
                if let Some(callee) = resolve(method) {
                    walk(&callee, found, resolve, seen);
                    if *found {
                        return;
                    }
                }
            }
        }
        e.node.for_each_child(&mut |c| walk(c, found, resolve, seen));
    }
    let mut found = false;
    walk(body, &mut found, resolve, seen);
    found
}

/// Recognize `before_action { ... }` (optionally with `only:`/`except:`)
/// in an Unknown body item — the block form has no symbol target, so the
/// ingester round-trips it verbatim instead of producing a `Filter`.
/// Returns the block body plus the only/except scoping.
fn block_form_filter(expr: &Expr) -> Option<(Expr, Vec<Symbol>, Vec<Symbol>)> {
    let ExprNode::Send { recv: None, method, args, block: Some(b), .. } = &*expr.node else {
        return None;
    };
    if method.as_str() != "before_action" {
        return None;
    }
    let ExprNode::Lambda { body, .. } = &*b.node else {
        return None;
    };
    let mut only: Vec<Symbol> = Vec::new();
    let mut except: Vec<Symbol> = Vec::new();
    for a in args {
        let ExprNode::Hash { entries, .. } = &*a.node else { continue };
        for (k, v) in entries {
            let ExprNode::Lit { value: crate::expr::Literal::Sym { value: key } } = &*k.node
            else {
                continue;
            };
            match key.as_str() {
                "only" => only = filter_symbol_list(v),
                "except" => except = filter_symbol_list(v),
                _ => {}
            }
        }
    }
    Some((body.clone(), only, except))
}

/// `[:a, :b]` / `:a` → the symbol names; anything else → empty.
fn filter_symbol_list(e: &Expr) -> Vec<Symbol> {
    use crate::expr::Literal;
    match &*e.node {
        ExprNode::Array { elements, .. } => elements
            .iter()
            .filter_map(|el| match &*el.node {
                ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
                _ => None,
            })
            .collect(),
        ExprNode::Lit { value: Literal::Sym { value } } => vec![value.clone()],
        _ => Vec::new(),
    }
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

    // `performed?` — the before_action preamble's halting check
    // (`return if performed?` after a filter that can render/redirect).
    info.instance_methods
        .entry(Symbol::from("performed?"))
        .or_insert_with(|| fn_sig(vec![], Ty::Bool));

    // Implicit-`params` — actions read `@params` (the lowerer rewrote
    // bare `params` → `@params`) which the typer should treat as a
    // Hash-shaped object. The instance-method version is for cases
    // the rewrite missed.
    info.instance_methods
        .entry(Symbol::from("params"))
        .or_insert_with(|| fn_sig(vec![], any_hash));

    // `request_format` — accessor populated by main.rb from the path's
    // `.json` suffix sniff. Action bodies branch on `request_format ==
    // :json` after the Jbuilder-lowerer respond_to flatten; without a
    // signature here the body-typer leaves the bare call as TyVar.
    // Tagged AttributeReader so per-target emit (TS getter, Rust
    // field) treats it as a property read, not a method call.
    info.instance_methods
        .entry(Symbol::from("request_format"))
        .or_insert_with(|| fn_sig(vec![], Ty::Sym));
    info.instance_method_kinds
        .entry(Symbol::from("request_format"))
        .or_insert(AccessorKind::AttributeReader);
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
    all_controllers_for_params: &[Controller],
    privs: &[Action],
    // `privs` plus every inherited `*_params` helper — the MRO-correct
    // list the params-helper rewrites resolve against.
    params_privs: &[Action],
    is_public: bool,
    params_specs: &ParamsSpecs,
    json_actions: &std::collections::HashSet<Symbol>,
    turbo_stream_actions: &std::collections::HashSet<Symbol>,
    view_ivars: &ViewIvarMap,
    partials: &PartialMap,
    format_breadth: FormatBreadth,
    shadows: &std::collections::HashSet<Symbol>,
    route_id_segments: &std::collections::HashMap<String, Vec<bool>>,
    deferred_renders: &std::collections::HashSet<Symbol>,
    deferred_out: &mut std::collections::HashMap<Symbol, Expr>,
) -> MethodDef {
    let method_name = method_name_for_action(a.name.as_str());
    // Required positionals first, then optional positionals with their
    // defaults — so `def get_from_cache(opts = {})` round-trips instead of
    // emitting `def get_from_cache` and crashing the body that reads `opts`.
    let mut params: Vec<Param> = a
        .params
        .fields
        .iter()
        .map(|(n, _)| Param::positional(n.clone()))
        .collect();
    for (n, default) in &a.opt_params {
        params.push(Param::with_default(n.clone(), default.clone()));
    }
    // Order matters: turbo_stream is tested before json, so an action
    // with both templates picks the one the request actually asked for.
    let mut variants: Vec<&str> = Vec::new();
    if turbo_stream_actions.contains(&a.name) {
        variants.push("turbo_stream");
    }
    if json_actions.contains(&a.name) {
        variants.push("json");
    }
    // An overriding `<x>_params` yields its parent's params class —
    // see `lower_overriding_params_helper`.
    let formats_template =
        formats_only_render_template(controller, &a.name, is_public);
    let inherited_spec = if a.name.as_str().ends_with("_params") {
        inherited_params_spec(controller, all_controllers_for_params, &a.name, params_specs)
    } else {
        None
    };
    let (body, deferred_tail) = lower_action_body(
        &a.body,
        controller,
        a.name.as_str(),
        privs,
        params_privs,
        is_public,
        params_specs,
        &variants,
        view_ivars,
        partials,
        format_breadth,
        shadows,
        route_id_segments,
        deferred_renders.contains(&a.name),
        inherited_spec,
        formats_template.as_ref(),
    );
    if let Some(tail) = deferred_tail {
        deferred_out.insert(a.name.clone(), tail);
    }
    // Action params type to Untyped for now — Rails action signatures
    // are conventionally `def show(id)` with all-string CGI inputs;
    // refinement to per-route param types can ride on a later
    // routing-table-aware pass.
    //
    // Return type:
    //   - Public actions terminate in render/redirect (synthesized or
    //     explicit) → Nil.
    //   - Private `_params` helpers return the typed `<Resource>Params`
    //     class (callers do `Model.from_params(comment_params)`). The
    //     class comes from the permit list in the helper's OWN BODY, not
    //     from stripping `_params` off its name: campfire's `bot_params`
    //     permits `:user`, and one resource can carry several lists.
    //   - Other private actions default to Nil; refine when a
    //     forcing fixture surfaces.
    let ret_ty = if !is_public && method_name.ends_with("_params") {
        let spec = self::params::first_permit_in(&a.body)
            .and_then(|(resource, fields)| params_specs.find(&resource, &fields));
        if let Some(spec) = spec {
            Ty::Class { id: spec.class_id.clone(), args: vec![] }
        } else {
            // Fallback for helpers whose permit list we didn't recognize
            // (campfire's `role_params` builds a bare Hash) — stays
            // typed-coarse rather than panicking.
            Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) }
        }
    } else if is_public {
        // Routed actions terminate in render/redirect → Nil.
        Ty::Nil
    } else {
        // Helper/private methods RETURN VALUES — lobsters'
        // get_from_cache yields the (stories, show_more) pair its
        // actions destructure, user_token_link builds a URL. The old
        // blanket Nil was a WRONG PIN the AOT trusted: spinel refused
        // `@a, @b = get_from_cache(...)` as a nil destructure (and the
        // massign repro matrix showed every honest shape passes).
        // Untyped lets the compiler infer from the body instead.
        Ty::Untyped
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
        is_async: false,
            mutates_self: false,
            block_param: a.block_param.clone().map(Param::positional),
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
    params_privs: &[Action],
    is_public: bool,
    params_specs: &ParamsSpecs,
    variants: &[&str],
    view_ivars: &ViewIvarMap,
    partials: &PartialMap,
    format_breadth: FormatBreadth,
    shadows: &std::collections::HashSet<Symbol>,
    route_id_segments: &std::collections::HashMap<String, Vec<bool>>,
    defer_implicit_render: bool,
    inherited_params_spec: Option<&ParamsSpec>,
    formats_only_render: Option<&Symbol>,
) -> (Expr, Option<Expr>) {
    // BEFORE everything else: this turns a helper the permit recognizer
    // could not read into one that yields the params class, so every
    // rewrite below sees the shape it expects.
    let body = &match inherited_params_spec {
        Some(spec) => self::params::lower_overriding_params_helper(body, spec),
        None => body.clone(),
    };
    // BEFORE the render rewrite: `render formats: :svg` becomes the
    // internal `render :<action>, format: :svg` that pass already binds.
    let body = &match formats_only_render {
        Some(template) => {
            let mut copy = body.clone();
            resolve_formats_only_render(&mut copy, template);
            copy
        }
        None => body.clone(),
    };
    let unwrapped = unwrap_respond_to_with_format_dispatch(body, format_breadth);
    // `is_public` gates the SYNTHESIS — a private helper's caller does
    // the rendering, so nothing is appended to its body — and nothing
    // else. The render REWRITE runs on both: a private helper that
    // calls `render_to_string(partial: "x", locals: {…})` needs the
    // same def-site binding a public action's `render partial:` gets,
    // and campfire writes exactly that (`each_user_and_html_for`
    // renders the room partial ONCE and broadcasts the string to each
    // member). Gating the rewrite too left `render_to_string` standing
    // in the emit as an undefined method — the doc on `action_to_method`
    // has always said "implicit-render synthesis", which is the
    // behaviour this restores.
    // Synthesized even when it is going to be MOVED: the tail has to
    // ride every rewrite below with the rest of the body — the render
    // rewrite in particular, which is what turns `render :create` into
    // `Views::Messages.create_turbo_stream(@message, …)` using this
    // action's ivar scope. Building it in the dispatcher instead emitted
    // a bare `render(:create)` that resolves to nothing.
    // Does this body respond through a private HELPER? `contains_terminal`
    // recognizes only a literal `render`/`redirect_to`/`head`, so a body
    // whose whole job is to pick a helper — campfire's avatar `show`
    // choosing between `send_webp_blob_file`, `render_default_bot` and
    // `render_initials` — looked terminal-free and got the UNGUARDED
    // synthesized tail. It then raised MissingTemplate over the response
    // the helper had just produced.
    //
    // The always-guarded shape is the fix and costs one `performed?`
    // check. Gated on actually calling such a helper rather than applied
    // everywhere, so a body with no terminal anywhere keeps the bare tail
    // it has always emitted.
    let responds_via_helper = privs
        .iter()
        .any(|p| has_toplevel_terminal(&p.body) && body_calls_method(body, &p.name));
    // Does ANY template exist for this action, in any format?
    //
    // Rails' `default_render` splits three ways and only the last is a
    // 204: a template for THIS format renders it; templates in OTHER
    // formats but not this one raise `ActionController::UnknownFormat`;
    // no template at all logs "No template found" and heads
    // `:no_content`. Asking only about HTML collapsed the middle case
    // into the last and turned a turbo_stream-only action's HTML branch
    // into a 204 — which `turbo_stream_views.rs` caught, correctly.
    //
    // So the raise stands whenever the action has SOME template, which
    // keeps that middle case at its current approximation (MissingTemplate
    // where Rails says UnknownFormat — a closer answer is its own change).
    // Only a genuinely templateless action, like campfire's
    // `Messages::Boosts#destroy`, reaches `head :no_content`.
    let module_key = views_module_name(controller).unwrap_or_default();
    let any_template_exists = view_ivars
        .contains_key(&(module_key.clone(), action_name.to_string()))
        || variants.iter().any(|v| {
            view_ivars.contains_key(&(module_key.clone(), format!("{action_name}_{v}")))
        });
    let base = if !is_public {
        unwrapped
    } else if defer_implicit_render || responds_via_helper {
        // Always guarded — see `synthesize_deferred_implicit_render`.
        // Two reasons to want that: the tail is about to move to the
        // dispatcher because something else (a subclass past `super`) may
        // respond, or a private helper in this body already has.
        synthesize_deferred_implicit_render(&unwrapped, action_name, variants, any_template_exists)
    } else {
        crate::lower::controller::body::synthesize_implicit_render_with_html(
            &unwrapped, action_name, variants, any_template_exists,
        )
    };
    let module_name = views_module_name(controller);
    let with_render = {
        let ivars = ivars_in_scope(controller, action_name, &base, privs);
        rewrite_render_to_views(
            &base,
            module_name.as_deref(),
            &ivars,
            view_ivars,
            partials,
            action_name,
        )
    };

    // Render `location: @ivar` kwarg → `RouteHelpers.<x>_path(@x.id)`
    // — Rails' POST-201 idiom (`render :show, status: :created,
    // location: @article`) passes a record where the runtime's render
    // wants a path string. Same polymorphic transform as
    // `rewrite_redirect_to`, just on the kwarg position rather than
    // the first positional arg.
    let with_render = rewrite_render_location_kwarg(&with_render);
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
        rewrite_model_new_to_from_params(&with_redirects, params_privs, params_specs);
    // `@user.update user_params` — retarget to the typed method sized to
    // that helper's permit list. Plain `update` is Rails' attribute-Hash
    // contract and no longer takes a params object.
    let with_from_params =
        rewrite_update_to_typed_variant(&with_from_params, params_privs, params_specs);
    let with_assoc =
        rewrite_assoc_through_parent_typed(&with_from_params, params_privs, params_specs);
    // The legacy chain rewrites (`rewrite_drop_includes` +
    // `rewrite_order_to_sort_by`) used to land here. They've moved
    // to a post-typing pass in the per-method loop so the Arel pass
    // gets first crack at the original chain shape. Legacy stays as
    // a fallback for anything Arel doesn't recognize. See
    // project_arel_compile_time_first.md.
    let with_destroy = rewrite_destroy_bang(&with_assoc);
    let with_routes = rewrite_route_helpers(&with_destroy, shadows, route_id_segments);
    // Some rewrites (rewrite_assoc_through_parent in particular)
    // produce nested Seqs — `Seq { ..., Seq { stmts }, ... }`. The
    // body-typer's Seq walker only propagates ivar bindings from
    // immediate-child Assigns; nested Seqs swallow their own
    // bindings. Splice nested Seqs into their parent so each
    // assignment is visible to subsequent siblings.
    let lowered = flatten_seqs(&with_routes);
    if !defer_implicit_render {
        return (lowered, None);
    }
    // Split the synthesized tail back off, now that it has been through
    // every rewrite the body had. It is the LAST statement — that is
    // where `synthesize_implicit_render`'s `append_statement` put it —
    // and it is only ever split off an action the synthesis actually
    // appended to, so a body that already terminated keeps its shape.
    split_trailing_statement(lowered)
}

/// `Seq { a, b, tail }` → `(Seq { a, b }, Some(tail))`. Anything that is
/// not a multi-statement Seq is returned untouched with no tail: the
/// synthesis appends, so a body it declined to touch has nothing to give
/// back.
fn split_trailing_statement(body: Expr) -> (Expr, Option<Expr>) {
    let ExprNode::Seq { exprs } = &*body.node else {
        return (body, None);
    };
    if exprs.len() < 2 {
        return (body, None);
    }
    let mut rest = exprs.clone();
    let tail = rest.pop().expect("len >= 2");
    let mut trimmed = Expr::new(body.span, ExprNode::Seq { exprs: rest });
    trimmed.ty = body.ty.clone();
    trimmed.effects = body.effects.clone();
    (trimmed, Some(tail))
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
            hint: e.hint,
            decisions: e.decisions,
        }
    }
    flatten(expr)
}
