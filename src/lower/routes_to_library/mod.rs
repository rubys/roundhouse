//! Lower the flattened route table into a `RouteHelpers` LibraryClass
//! with one class method per named route. Bodies are typed
//! `StringInterp` expressions that build the path from the typed
//! `:param` segments — `article_path(id: Integer) -> String` produces
//! `"/articles/#{id}"`. The runtime previously hand-shipped this
//! shape; producing it from `app.routes` keeps it in sync with
//! `config/routes.rb` and removes the per-app stub.
//!
//! Self-describing IR: each path-param is typed (`Int` for `id`-shape
//! params, `Str` otherwise) and each method's signature is recorded
//! up front. The TS emitter renders these as `static` methods returning
//! `string`; downstream targets get the same shape.
//!
//! Module-shaped (no inheritance, no instance state) so it emits the
//! same way under every target's class-vs-module distinction.

pub mod direct;

use crate::App;
use crate::dialect::{HttpMethod, LibraryFunction, Param};
use crate::effect::EffectSet;
use crate::expr::{ArrayStyle, Expr, ExprNode, InterpPart, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::lower::routes::{flatten_routes, FlatRoute};
use crate::lower::typing::{fn_sig, lit_str, lit_sym, with_ty};
use crate::span::Span;
use crate::ty::Ty;

/// Build the `Routes` dispatch module — `RouteTable.table -> Array<Route>`
/// (one `Route` instance per concrete `(verb, pattern, controller,
/// action)`) and `RouteTable.root -> Route` (the shorthand `root "c#a"`
/// route, when present). Empty when `app.routes` has no entries.
///
/// Each entry is `ActionDispatch::Router::Route.new(...)` — a typed
/// class with `verb`/`pattern`/`controller`/`action` accessors —
/// rather than a `Hash[Symbol, untyped]`. Strict-typed targets (Rust,
/// Crystal) get a real per-field type at every access; permissive
/// targets (TS, Ruby) keep working without runtime change.
///
/// Separate from `RouteHelpers` (URL-helper functions like
/// `article_path(id)`) because the two artifacts serve different
/// consumers: helpers are called from view + controller bodies,
/// dispatch is read at startup by the HTTP router.
pub fn lower_routes_to_dispatch_functions(app: &App) -> Vec<LibraryFunction> {
    let flat = flatten_routes(app);
    if flat.is_empty() {
        return Vec::new();
    }
    let module_path = vec![Symbol::from("RouteTable")];
    // Same partition the per-target Spinel emit used: path "/" goes
    // to `RouteTable.root`, everything else to `RouteTable.table`. Callers
    // typically combine them at use site (`[RouteTable.root] +
    // RouteTable.table`).
    let (root_routes, table_routes): (Vec<&FlatRoute>, Vec<&FlatRoute>) =
        flat.iter().partition(|r| r.path == "/");

    let route_class_id = ClassId(Symbol::from("ActionDispatch::Router::Route"));
    let route_ty = Ty::Class {
        id: route_class_id.clone(),
        args: vec![],
    };

    let mut out: Vec<LibraryFunction> = Vec::new();

    let table_body = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Array {
                elements: table_routes
                    .iter()
                    .map(|r| build_route_new(r, &route_class_id, &route_ty))
                    .collect(),
                style: ArrayStyle::Brackets,
            },
        ),
        Ty::Array { elem: Box::new(route_ty.clone()) },
    );
    out.push(LibraryFunction {
        module_path: module_path.clone(),
        name: Symbol::from("table"),
        params: Vec::new(),
        body: table_body,
        signature: Some(fn_sig(
            vec![],
            Ty::Array { elem: Box::new(route_ty.clone()) },
        )),
        effects: EffectSet::default(),
        is_async: false,
    });

    if let Some(r) = root_routes.first() {
        let root_body = build_route_new(r, &route_class_id, &route_ty);
        out.push(LibraryFunction {
            module_path,
            name: Symbol::from("root"),
            params: Vec::new(),
            body: root_body,
            signature: Some(fn_sig(vec![], route_ty)),
            effects: EffectSet::default(),
            is_async: false,
        });
    }

    out
}

/// Build `ActionDispatch::Router::Route.new("GET", "/x", :articles,
/// :index)`. Per-field types are baked into the Route class definition
/// in `runtime/ruby/action_dispatch/router.rb` (and its RBS sidecar),
/// so strict-typed targets resolve each accessor against its declared
/// type rather than an untyped value channel. Positional (not kwarg)
/// args — per-target emitters convert kwarg-style def to positional
/// pub fn but don't unpack kwarg-style call sites; matches the
/// positional `initialize` signature.
fn build_route_new(r: &FlatRoute, class_id: &ClassId, route_ty: &Ty) -> Expr {
    let verb_str = match r.method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Head => "HEAD",
        HttpMethod::Options => "OPTIONS",
        HttpMethod::Any => "ANY",
    };
    let controller_sym = controller_symbol(r.controller.0.as_str());
    let mut args = vec![
        lit_str(verb_str.to_string()),
        lit_str(r.path.clone()),
        lit_sym(Symbol::from(controller_sym)),
        lit_sym(r.action.clone()),
    ];
    // Route-forced format rides as the optional 5th positional
    // (`Route.new(..., :rss)`); format-free routes stay 4-arg so
    // existing route tables emit byte-identical.
    if let Some(fmt) = &r.format {
        args.push(lit_sym(fmt.clone()));
    }
    // Digit-constrained params ride as the optional 6th positional, a
    // single space-joined string (`Route.new(..., nil, "id")`) — the
    // router rejects candidate segments that aren't all digits (Roda
    // `Integer` matcher, Rails digit-class `constraints:`). A scalar
    // `String` (not `Array[String]`) keeps the optional tail one type
    // across strict/AOT targets; constraint-free routes keep their
    // 4-/5-arg shape.
    if !r.int_params.is_empty() {
        if r.format.is_none() {
            let mut nil = Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Nil },
            );
            nil.ty = Some(Ty::Nil);
            args.push(nil);
        }
        args.push(lit_str(r.int_params.join(" ")));
    }
    let class_path: Vec<Symbol> = class_id
        .0
        .as_str()
        .split("::")
        .map(Symbol::from)
        .collect();
    let recv = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: class_path },
    );
    with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from("new"),
                args,
                block: None,
                parenthesized: true,
            },
        ),
        route_ty.clone(),
    )
}

/// `ArticlesController` → `articles` (the controller-symbol form
/// the spinel router uses). Mirrors the existing per-target convention.
/// Namespaced controllers flatten with underscores
/// (`Mod::ActivitiesController` → `mod_activities`) — a plain-ident
/// symbol every target's symbol literal can carry (`:mod::activities`
/// parses as scope resolution, not a symbol, under Ruby).
pub(crate) fn controller_symbol(class_name: &str) -> String {
    let base = class_name.strip_suffix("Controller").unwrap_or(class_name);
    crate::naming::underscore(base).replace('/', "_")
}

/// Build the `RouteHelpers` module from `app.routes` as a list of
/// `LibraryFunction`s, one per named route. Empty when the app has
/// no routes.
pub fn lower_routes_to_library_functions(app: &App) -> Vec<LibraryFunction> {
    let flat = flatten_routes(app);
    if flat.is_empty() {
        return Vec::new();
    }
    let module_path = vec![Symbol::from("RouteHelpers")];
    // Dedupe: multiple HTTP verbs on the same path collapse to a
    // single helper (`articles` for both index/create — same URL).
    // First-occurrence wins; the as_name + path are identical so the
    // function body is the same.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut funcs: Vec<LibraryFunction> = Vec::new();
    // Helper name -> (every segment name it can take, how many
    // positionals the GENERATED helper requires). The query survey
    // needs both: to tell an option naming a SEGMENT from a query
    // param, and to ignore a call that doesn't fill the segments.
    //
    // Segments UNION across same-named routes and `required` is
    // first-wins, because `flatten_routes` expands a Rails optional
    // group into several routes that share one helper — lobsters'
    // `get "/mod/notes(/:period)"` yields one route with `period` and
    // one without, and the generated helper is the FIRST (`seen` below).
    // Taking the last one's segments instead made `period` look like a
    // query param and emitted `mod_notes_path(period = nil, period:
    // nil)` — a duplicate parameter name.
    let mut helper_shape: std::collections::HashMap<String, (Vec<String>, usize)> =
        Default::default();
    for r in flat.iter().filter(|r| r.named) {
        // Both spellings: `_url` is the same helper with the host in
        // front (the view/controller lowering rewrites it to `"http://…"
        // + <name>_path(…)`), and campfire calls `new_session_url(
        // email_address: …)`. That rewrite happens after this survey, so
        // a `_url`-only key would be missed — register the alias and
        // record its demand against the `_path` helper that is generated.
        for suffix in ["_path", "_url"] {
            let entry = helper_shape
                .entry(format!("{}{suffix}", r.as_name))
                .or_insert_with(|| (Vec::new(), r.required_params.min(r.path_params.len())));
            for p in &r.path_params {
                if !entry.0.iter().any(|s| s == p.as_str()) {
                    entry.0.push(p.to_string());
                }
            }
        }
    }
    // Format variants are registered in the survey BEFORE it runs, and
    // that ordering is load-bearing: `route_format_suffix` has already
    // renamed `x_path(bot_key: k, format: :json)` to
    // `x_json_path(bot_key: k)`, so a survey keyed on the BASE name no
    // longer sees that call at all. A helper whose only call site
    // carried a format then lost its query keys and the variant was
    // generated without them (`unknown keyword: :bot_key`). Registering
    // the variant under its own name collects them against the variant,
    // where they belong.
    let declared: std::collections::HashSet<String> =
        flat.iter().filter(|r| r.named).map(|r| format!("{}_path", r.as_name)).collect();
    let variants = format_variant_demand(app, &declared);
    for (name, (as_name, _)) in &variants {
        let base = helper_shape.get(&format!("{as_name}_path")).cloned();
        if let Some(shape) = base {
            let stem = name.strip_suffix("_path").unwrap_or(name);
            for suffix in ["_path", "_url"] {
                helper_shape.entry(format!("{stem}{suffix}")).or_insert_with(|| shape.clone());
            }
        }
    }
    let query_keys = query_param_demand(app, &helper_shape);
    for route in &flat {
        // Unnamed dynamic routes (`get "/comments/page/:page"`, no `as:`)
        // get no helper in Rails — their action-name fallback would
        // shadow a real static route's helper under first-wins dedupe
        // (`comments_path` for `/replies/comments/page/:page` hiding
        // `/comments`).
        if !route.named {
            continue;
        }
        let helper = format!("{}_path", route.as_name);
        if !seen.insert(helper.clone()) {
            continue;
        }
        funcs.push(build_helper_function(
            &module_path,
            &helper,
            route,
            app,
            query_keys.get(&helper).map(|v| v.as_slice()).unwrap_or(&[]),
            None,
        ));
    }
    // Format variants — one function per (route, format) a call site
    // actually asked for. See `format_variant_demand`.
    for (name, (as_name, ext)) in &variants {
        let Some(route) = flat.iter().find(|r| r.named && &r.as_name == as_name) else {
            continue;
        };
        if !seen.insert(name.clone()) {
            continue;
        }
        funcs.push(build_helper_function(
            &module_path,
            name,
            route,
            app,
            query_keys.get(name).map(|v| v.as_slice()).unwrap_or(&[]),
            Some(ext),
        ));
    }
    // Hash-form `url_for` resolvers ride along here rather than at each
    // target's call site: every emitter that wants route helpers wants
    // these too, and a per-target wiring is nine places to forget one.
    funcs.extend(lower_url_option_helpers(app));
    // `direct :name do … end` helpers — not routes, so they never reach
    // `flat`; they ride here because they live in the same module and
    // their `route_for` bodies need the flattened table to resolve.
    funcs.extend(direct::lower_direct_helpers(&module_path, app, &flat));
    // These bodies carry APP source (a `direct` block's expressions —
    // campfire's `v: Current.account&.updated_at&.to_fs(:number)`), but
    // they are synthesized HERE, after the post-analyze hook has already
    // walked every body it owns. So the Time grounding has to be applied
    // to them on the way out; otherwise the one place `to_fs` is emitted
    // by us, rather than written by the app, is the one place it never
    // gets lowered.
    for f in &mut funcs {
        crate::lower::time_current::rewrite_time_current(&mut f.body, &app.time_formats);
    }
    funcs
}

/// The generated resolver for a hash-form `url_for`. `extras` are the
/// option keys beside `controller:`/`action:`, sorted — one function per
/// distinct set, so each keeps TYPED params instead of taking a
/// `Hash[Symbol, untyped]` bag. Shared with the view lowerer, which
/// emits the call.
pub fn url_options_helper_name(extras: &[String]) -> String {
    if extras.is_empty() {
        "path_for_controller_action".to_string()
    } else {
        format!("path_for_controller_action_{}", extras.join("_"))
    }
}

/// Resolvers for the hash-form `url_for` — `link_to text, {controller:
/// controller_name, action: action_name, page: @page + 1}`, which is how
/// every lobsters index paginates.
///
/// Rails resolves that against the route set at render time. A compile-
/// time resolution is impossible here for the reason the shape exists at
/// all: `controller_name`/`action_name` are runtime reads, and the view
/// holding them (`home/index`) is rendered by eight different actions.
/// So generate the LOOKUP instead — a function per extra-key set,
/// carrying the table of every route whose dynamic segments are exactly
/// those extras, keyed on `"controller#action"`.
///
/// Left unresolved, the options hash reached the tag renderer and each
/// key became an HTML attribute: `<a href-controller="home"
/// href-action="newest" href-page="2">`, i.e. no href at all — a dead
/// "Page 2 >>" link on /, /newest, /recent, /comments, /replies and
/// /moderations.
pub fn lower_url_option_helpers(app: &App) -> Vec<LibraryFunction> {
    let mut extra_sets: Vec<Vec<String>> = Vec::new();
    for view in &app.views {
        collect_url_option_key_sets(&view.body, &mut extra_sets);
    }
    extra_sets.sort();
    extra_sets.dedup();
    if extra_sets.is_empty() {
        return Vec::new();
    }
    let flat = flatten_routes(app);
    let module_path = vec![Symbol::from("RouteHelpers")];
    extra_sets
        .iter()
        .map(|extras| build_url_options_function(&module_path, extras, &flat))
        .collect()
}

/// Every Hash literal in a view whose keys are all Symbols and include
/// both `controller` and `action` — Rails' url-options form. Yields the
/// remaining keys, sorted.
fn collect_url_option_key_sets(e: &Expr, out: &mut Vec<Vec<String>>) {
    if let ExprNode::Hash { entries, .. } = &*e.node {
        let keys: Option<Vec<String>> = entries
            .iter()
            .map(|(k, _)| match &*k.node {
                ExprNode::Lit { value: Literal::Sym { value } } => {
                    Some(value.as_str().to_string())
                }
                _ => None,
            })
            .collect();
        if let Some(keys) = keys {
            if keys.iter().any(|k| k == "controller") && keys.iter().any(|k| k == "action") {
                let mut extras: Vec<String> = keys
                    .into_iter()
                    .filter(|k| k != "controller" && k != "action")
                    .collect();
                extras.sort();
                out.push(extras);
            }
        }
    }
    e.node.for_each_child(&mut |c| collect_url_option_key_sets(c, out));
}

/// One resolver: `path_for_controller_action_page(controller, action,
/// page)`. Body is an if-chain over `"#{controller}##{action}"` — an
/// if-chain rather than a `case` so every target's expression emitter
/// renders it without pattern-match support. The fall-through raises,
/// which is what Rails does for an unroutable `url_for`
/// (`ActionController::UrlGenerationError`); returning some invented
/// path would ship a wrong link instead of reporting the gap.
fn build_url_options_function(
    module_path: &[Symbol],
    extras: &[String],
    flat: &[FlatRoute],
) -> LibraryFunction {
    let no_slugs = std::collections::HashSet::new();
    let key_expr = Expr::new(
        Span::synthetic(),
        ExprNode::StringInterp {
            parts: vec![
                InterpPart::Expr { expr: var_ref("controller") },
                InterpPart::Text { value: "#".to_string() },
                InterpPart::Expr { expr: var_ref("action") },
            ],
        },
    );
    // Candidate routes: a GET whose dynamic segments are exactly the
    // extras. `/newest/:user/page/:page` is not a candidate for the
    // `page`-only set — its `:user` has no value to fill.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut arms: Vec<(String, Expr)> = Vec::new();
    for route in flat {
        if route.method != HttpMethod::Get {
            continue;
        }
        let mut params = route.path_params.clone();
        params.sort();
        if params != extras {
            continue;
        }
        let key = format!(
            "{}#{}",
            controller_symbol(route.controller.0.as_str()),
            route.action.as_str()
        );
        if !seen.insert(key.clone()) {
            continue;
        }
        arms.push((key, build_path_expr(&route.path, &route.path_params, &no_slugs)));
    }
    // Innermost else. `raise` reads as a Send on every target that
    // emits this module.
    let unroutable = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from("raise"),
            args: vec![Expr::new(
                Span::synthetic(),
                ExprNode::StringInterp {
                    parts: vec![
                        InterpPart::Text { value: "no route matches ".to_string() },
                        InterpPart::Expr { expr: key_expr.clone() },
                    ],
                },
            )],
            block: None,
            parenthesized: false,
        },
    );
    // Fold the arms into a nested if/else, last arm outermost-last.
    let mut body = unroutable;
    for (key, path) in arms.into_iter().rev() {
        body = Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(key_expr.clone()),
                        method: Symbol::from("=="),
                        args: vec![lit_str(key)],
                        block: None,
                        parenthesized: false,
                    },
                ),
                then_branch: path,
                else_branch: body,
            },
        );
    }
    let names: Vec<String> = std::iter::once("controller".to_string())
        .chain(std::iter::once("action".to_string()))
        .chain(extras.iter().cloned())
        .collect();
    LibraryFunction {
        module_path: module_path.to_vec(),
        name: Symbol::from(url_options_helper_name(extras)),
        params: names
            .iter()
            .map(|n| Param::positional(Symbol::from(n.clone())))
            .collect(),
        body,
        signature: Some(fn_sig(
            names
                .iter()
                .map(|n| (Symbol::from(n.clone()), Ty::Str))
                .collect(),
            Ty::Str,
        )),
        effects: EffectSet::default(),
        is_async: false,
    }
}

/// Does the route's resource model override `to_param`? Rails feeds a
/// path helper's `:id` segment from `record.to_param`, so an override
/// (lobsters' Story#to_param → short_id) makes the helper's id param
/// String-shaped, not Integer. Controller → model by singularizing
/// the controller symbol (`StoriesController` → `story`); an
/// `as:`-named route can point at a foreign controller (lobsters'
/// `/domains/:id => home#for_domain, as: "domain"`), so the helper's
/// own name is the fallback resource lookup (`domain_path` → Domain).
fn model_overrides_to_param(controller: &str, helper_name: &str, app: &App) -> bool {
    let from_controller = crate::naming::singularize(&controller_symbol(controller));
    if named_model_overrides_to_param(&from_controller, app) {
        return true;
    }
    let base = helper_name.strip_suffix("_path").unwrap_or(helper_name);
    let word = base.rsplit('_').next().unwrap_or(base);
    named_model_overrides_to_param(&crate::naming::singularize(word), app)
}

fn named_model_overrides_to_param(resource: &str, app: &App) -> bool {
    let model_name = crate::naming::camelize(resource);
    app.models.iter().any(|m| {
        m.name.0.as_str() == model_name
            && m.body.iter().any(|item| matches!(
                item,
                crate::dialect::ModelBodyItem::Method { method, .. }
                    if method.name.as_str() == "to_param"
            ))
    })
}

fn build_helper_function(
    module_path: &[Symbol],
    helper_name: &str,
    route: &FlatRoute,
    app: &App,
    query_keys: &[QueryKey],
    // `Some("json")` builds the MONOMORPHIZED variant a call site's
    // `format: :json` asks for — see `format_variant_demand`. The
    // extension is a literal here, so it costs the base helper nothing.
    ext: Option<&str>,
) -> LibraryFunction {
    let slug_id = model_overrides_to_param(route.controller.0.as_str(), helper_name, app);
    // Slugness is PER PARAM: a nested parent's segment names its model
    // directly (`story_id` in `/stories/:story_id/suggestions` → Story,
    // whose to_param is a short_id slug) — the owning route's
    // controller/helper say nothing about the parent. `<x>_id` params
    // consult model `<x>`; bare `id` (and unmatched stems) keep the
    // route-level heuristic. Typing a slug segment Int made every
    // strict-target call site passing `story.short_id` a C type error.
    let param_is_slug = |p: &str| -> bool {
        if let Some(stem) = p.strip_suffix("_id") {
            if !stem.is_empty() && named_model_overrides_to_param(stem, app) {
                return true;
            }
        }
        slug_id
    };
    let slug_params: std::collections::HashSet<String> = route
        .path_params
        .iter()
        .filter(|p| param_is_slug(p.as_str()))
        .cloned()
        .collect();
    // A trailing `(.:format)` is Rails' OPTIONAL format suffix, not a
    // path segment: the helper takes `format = nil` last and appends
    // `.<format>` only when given (`domain_path(d)` → "/domains/d",
    // `comments_path(:rss)` → "/comments.rss"). Without this the
    // literal parens land in the URL and `format` is demanded of every
    // caller.
    let has_format = route.path.ends_with("(.:format)");
    let path = route.path.strip_suffix("(.:format)").unwrap_or(&route.path);
    let seg_params: Vec<String> = route
        .path_params
        .iter()
        .filter(|p| !(has_format && p.as_str() == "format"))
        .cloned()
        .collect();

    // Leading required-param count (clamped: a dropped `format` seg may
    // have been counted required upstream). Params beyond it come from
    // trailing Rails optional groups and take `nil` defaults.
    let required = route.required_params.min(seg_params.len());
    let nil_default = || {
        Expr::new(Span::synthetic(), ExprNode::Lit { value: crate::expr::Literal::Nil })
    };
    // `scope defaults: { user_id: "me" }` — the segment has a value
    // Rails supplies, so the helper takes it OPTIONALLY. campfire's
    // `resource :profile` lives under exactly that scope and every call
    // site writes a bare `user_profile_url`.
    //
    // Only honored when the defaulted params form a SUFFIX of the
    // signature. NOT because Ruby forbids the other shape — it does
    // not, `def f(a = "me", b)` is legal and binds `f(x)` to
    // `a="me", b=x`, which is exactly Rails' rule. The restriction is
    // for the STRICT targets: Rust has no default arguments, so the
    // emitter fills them at the CALL SITE by padding MISSING TRAILING
    // args (`expr::mod::current_class_method_param_tys`). A leading
    // default would be appended at the end instead of filled in place,
    // silently swapping the segments in the URL.
    //
    // What it costs, in campfire: push_subscriptions is routed under
    // `scope defaults: { user_id: "me" }`, so Rails accepts
    // `user_push_subscription_path(record)` — one arg for a two-segment
    // member route. Here that param stays required and the call raises
    // `wrong number of arguments (given 1, expected 2)`. The collection
    // helper is fine (its only defaulted param IS the suffix). Lifting
    // this means teaching the call-site padders to fill by position
    // from the left, not appending.
    let default_for = |p: &String| -> Option<&String> {
        route.param_defaults.iter().find(|(n, _)| n == p).map(|(_, v)| v)
    };
    let defaults_are_a_suffix = seg_params
        .iter()
        .position(|p| default_for(p).is_some())
        .is_none_or(|first| seg_params[first..].iter().all(|p| default_for(p).is_some()));
    let seg_default = |p: &String| -> Option<Expr> {
        if !defaults_are_a_suffix {
            return None;
        }
        default_for(p).map(|v| {
            Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: crate::expr::Literal::Str { value: v.clone() } },
            )
        })
    };
    // A defaulted segment that is NOT part of the suffix becomes a
    // KEYWORD, and leaves the positional list entirely.
    //
    // campfire routes push_subscriptions under `scope defaults: {
    // user_id: "me" }`, so Rails accepts
    // `user_push_subscription_path(record)` — one argument for a
    // two-segment member route. Keeping `user_id` positional-with-a-
    // default is what Ruby would do and what the strict targets cannot:
    // Rust has no default arguments, so the emitter fills them at the
    // CALL SITE by padding MISSING TRAILING args, and a LEADING default
    // would be appended at the end instead of filled in place, silently
    // swapping the segments in the URL. Every call site in the corpus
    // passes only the non-defaulted segments, so the keyword form binds
    // all of them and the padder never sees the ambiguity.
    //
    // WHAT IT COSTS, and the direction is the point: Rails also accepts
    // `user_push_subscription_path(user, record)`, filling the segments
    // left to right. Against a keyword that is an ARITY ERROR — loud,
    // at the call site, naming the helper — rather than a URL with its
    // segments swapped. A wrong URL that returns 200 for the wrong
    // record is the failure this shape must not have.
    let keyworded = |p: &String| -> bool { !defaults_are_a_suffix && default_for(p).is_some() };
    let keyword_default = |p: &String| -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: crate::expr::Literal::Str {
                    value: default_for(p).cloned().unwrap_or_default(),
                },
            },
        )
    };
    let mut params: Vec<Param> = seg_params
        .iter()
        .enumerate()
        .filter(|(_, p)| !keyworded(p))
        .map(|(i, p)| {
            let sym = Symbol::from(p.clone());
            if let Some(d) = seg_default(p) {
                Param::with_default(sym, d)
            } else if i < required {
                Param::positional(sym)
            } else {
                Param::with_default(sym, nil_default())
            }
        })
        .collect();
    let mut sig_params: Vec<(Symbol, Ty)> = seg_params
        .iter()
        .enumerate()
        .filter(|(_, p)| !keyworded(p))
        .map(|(i, p)| {
            let base = param_ty(p, slug_params.contains(p.as_str()));
            let ty = if i < required {
                base
            } else {
                Ty::Union { variants: vec![base, Ty::Nil] }
            };
            (Symbol::from(p.clone()), ty)
        })
        .collect();
    // The keyword half, in segment order, appended after the
    // positionals — `def f(id, user_id: "me")`.
    let mut segment_keyword_sig: Vec<crate::ty::Param> = Vec::new();
    for p in seg_params.iter().filter(|p| keyworded(p)) {
        let sym = Symbol::from(p.clone());
        params.push(Param::keyword(sym.clone(), Some(keyword_default(p))));
        segment_keyword_sig.push(crate::ty::Param {
            name: sym,
            ty: param_ty(p, slug_params.contains(p.as_str())),
            kind: crate::ty::ParamKind::Keyword { required: false },
        });
    }
    let mut body = if required < seg_params.len() {
        build_optional_path_expr(path, &seg_params, required, &slug_params)
    } else {
        build_path_expr(path, &seg_params, &slug_params)
    };
    // Rails' `(.:format)`: a route path spelled the `rails routes` way
    // ends in `(.:format)`, which gives the helper a trailing optional
    // positional — `comments_path(:rss)` → "/comments.rss".
    //
    // The KEYWORD spelling (`x_path(format: :json)`) is NOT a parameter:
    // `lower::route_format_suffix` turns it into a string concatenation
    // at the CALL SITE before this pass runs. Growing a parameter for it
    // was the obvious move and the wrong one — Rust and Go have no
    // default arguments, so widening a helper's signature broke every
    // call site that omitted the format, in every app, to serve the few
    // that pass one.
    if has_format {
        let format_sym = Symbol::from("format");
        params.push(Param::with_default(
            format_sym.clone(),
            Expr::new(Span::synthetic(), ExprNode::Lit { value: crate::expr::Literal::Nil }),
        ));
        sig_params.push((
            format_sym.clone(),
            Ty::Union { variants: vec![Ty::Str, Ty::Nil] },
        ));
        // <path> + (format.to_s == "" ? "" : ".#{format}")
        //
        // Not bare `format`: the param is `String | Nil`, and a target
        // whose `if` wants a real Bool rejects Ruby truthiness on it.
        // Not `format.nil?` either — Go renders an omitted optional as
        // the zero value, so `nil?` folds to `false` there and every
        // caller who omitted the format gets a trailing dot in the URL.
        // `to_s == ""` reads "absent" correctly whether the target
        // spells absence as nil, None, or "".
        //
        // Every node carries `Ty::Str`: a conditional whose type is left
        // for the emitter to infer renders as the target's top type (Go
        // answered `interface{}` and then refused to `+` it onto a
        // string), and the IR knows the answer perfectly well.
        let dot_format = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::StringInterp {
                    parts: vec![
                        InterpPart::Text { value: ".".to_string() },
                        InterpPart::Expr { expr: var_ref("format") },
                    ],
                },
            ),
            Ty::Str,
        );
        let format_to_s = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(var_ref("format")),
                    method: Symbol::from("to_s"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            ),
            Ty::Str,
        );
        let is_absent = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(format_to_s),
                    method: Symbol::from("=="),
                    args: vec![lit_str(String::new())],
                    block: None,
                    parenthesized: false,
                },
            ),
            Ty::Bool,
        );
        let suffix = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: is_absent,
                    then_branch: lit_str(String::new()),
                    else_branch: dot_format,
                },
            ),
            Ty::Str,
        );
        body = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(body),
                    method: Symbol::from("+"),
                    args: vec![suffix],
                    block: None,
                    parenthesized: false,
                },
            ),
            Ty::Str,
        );
    }
    // The monomorphized extension goes exactly where the dynamic one
    // above would: after the PATH and before the query string. That
    // placement is the whole point of this variant — `x_path(room_id: 2)
    // + ".json"` at the call site put it after `?room_id=2`, so the
    // parameter VALUE became "2.json" and the lookup missed.
    if let Some(ext) = ext {
        body = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(body),
                    method: Symbol::from("+"),
                    args: vec![lit_str(format!(".{ext}"))],
                    block: None,
                    parenthesized: false,
                },
            ),
            Ty::Str,
        );
    }
    // Options a call site passes that name no segment become the query
    // string. KEYWORD params with `nil` defaults, so the call sites
    // already written (`autocompletable_users_path(room_id: room.id)`)
    // bind unchanged and every other caller is unaffected.
    let mut query_sig: Vec<crate::ty::Param> = Vec::new();
    if !query_keys.is_empty() {
        for key in query_keys {
            let sym = Symbol::from(key.name.clone());
            params.push(Param::keyword(sym.clone(), Some(nil_default())));
            // KEYWORD in the signature too, not just in the `def`.
            // `fn_sig` types every param `Required`, which renders the
            // `.rbs` as a positional — spinel binds from the signature,
            // so a keyword declared positionally is a call it cannot
            // make.
            // An array key takes `Array[T]`, where T is what the
            // SINGULAR key would have been: `user_ids: [ user.id ]`
            // carries ids, and `param_ty` is name-based, so the
            // singular is what it must be asked about.
            let value_ty = if key.array {
                Ty::Array { elem: Box::new(param_ty(singular_key(&key.name), false)) }
            } else {
                param_ty(&key.name, false)
            };
            query_sig.push(crate::ty::Param {
                name: sym,
                ty: Ty::Union { variants: vec![value_ty, Ty::Nil] },
                kind: crate::ty::ParamKind::Keyword { required: false },
            });
        }
        body = append_query_string(body, query_keys);
    }
    // The signature must agree with the `def` above about which params
    // are optional: `fn_sig` marks every param Required, so a helper
    // whose Ruby reads `def f(x = nil)` was declaring `(T x)` in its
    // `.rbs`. Harmless while nothing checked, and not harmless on a
    // strict target reading the sidecar as a seed — a 0-arg call site
    // against a Required declaration is the exact contradiction
    // spinel#3977 was about. `optional_names` is the set the `def`
    // actually defaulted, defaults-from-`scope` and trailing
    // nil-optionals alike.
    let optional_names: std::collections::HashSet<Symbol> = params
        .iter()
        .filter(|p| p.default.is_some())
        .map(|p| p.name.clone())
        .collect();
    let mark_optional = |t: Ty| -> Ty {
        let Ty::Fn { params, block, ret, effects } = t else { return t };
        Ty::Fn {
            params: params
                .into_iter()
                .map(|mut tp| {
                    if optional_names.contains(&tp.name) {
                        tp.kind = crate::ty::ParamKind::Optional;
                    }
                    tp
                })
                .collect(),
            block,
            ret,
            effects,
        }
    };
    // Segment keywords ride with the query keywords: both are keyword
    // params in the `def`, and `fn_sig` types every param Required, so
    // declaring one positionally is a call spinel cannot make.
    query_sig.splice(0..0, segment_keyword_sig);
    let signature = match mark_optional(fn_sig(sig_params, Ty::Str)) {
        Ty::Fn { mut params, block, ret, effects } if !query_sig.is_empty() => {
            params.extend(query_sig);
            Ty::Fn { params, block, ret, effects }
        }
        other => other,
    };
    LibraryFunction {
        module_path: module_path.to_vec(),
        name: Symbol::from(helper_name),
        params,
        body,
        signature: Some(signature),
        effects: EffectSet::default(),
        is_async: false,
    }
}

/// Keyword names a call site may pass that are NOT query params, so
/// the demand survey must not turn them into one.
///
///   * `format:` is already a parameter — the trailing `(.:format)`
///     group gives the helper a `format = nil` and appends `.json`;
///   * `anchor:` is Rails' FRAGMENT (`#tag`), which is not part of the
///     query string and belongs to whoever models fragments.
///
/// A key naming one of the route's own path segments is excluded at the
/// call-site survey instead (it varies per route).
const NON_QUERY_OPTIONS: &[&str] = &["format", "anchor"];

/// One query key a helper must accept, and whether every call site
/// passes it an ARRAY (`user_ids: [ user.id ]`), which Rails renders as
/// repeated `k[]=v` pairs rather than one `k=v`.
///
/// Mixed usage across call sites resolves to SCALAR: one helper, one
/// signature, and the scalar rendering is the one that was already
/// shipping. A wrong URL is worse than a missing helper — the same
/// reasoning that kept arrays out entirely until now.
#[derive(Clone, Debug)]
pub(crate) struct QueryKey {
    pub(crate) name: String,
    pub(crate) array: bool,
}

/// Query-string keys each route helper is actually called with —
/// `autocompletable_users_path(room_id: room.id)` demands `room_id`.
///
/// Rails turns every option that is not a path segment into a query
/// param at call time; a generated helper has a fixed signature, so the
/// keys have to be known to appear in it. DEMAND-GATED for the same
/// reason the association-scope pass is: a helper nobody passes options
/// to keeps exactly the signature it had, so no corpus call site moves.
///
/// Surveyed over every body INCLUDING views, which is where most URL
/// helpers are called from and which the hook-body walk does not cover.
fn query_param_demand(
    app: &App,
    helpers: &std::collections::HashMap<String, (Vec<String>, usize)>,
) -> std::collections::HashMap<String, Vec<QueryKey>> {
    type Demand = std::collections::HashMap<String, std::collections::BTreeMap<String, bool>>;
    let mut out: Demand = Default::default();
    let mut collect = |e: &Expr| {
        fn walk(
            e: &Expr,
            helpers: &std::collections::HashMap<String, (Vec<String>, usize)>,
            out: &mut Demand,
        ) {
            if let ExprNode::Send { recv: None, method, args, .. } = &*e.node {
                if let Some((segments, required)) = helpers.get(method.as_str()) {
                    if let Some(last) = args.last() {
                        // A call that does not fill the helper's required
                        // segments is not a query call — lobsters writes
                        // `user_path(:user => name)` against `/u/:username`,
                        // which names no segment and supplies none either.
                        // Rails raises on it; adding a `user:` keyword
                        // would only change WHICH error it is, on a page
                        // that renders (a junk URL) today.
                        if args.len() - 1 < *required {
                            e.node.for_each_child(&mut |c| walk(c, helpers, out));
                            return;
                        }
                        if let ExprNode::Hash { entries, kwargs: true } = &*last.node {
                            for (k, v) in entries {
                                let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node
                                else {
                                    continue;
                                };
                                let key = key.as_str();
                                if NON_QUERY_OPTIONS.contains(&key)
                                    || segments.iter().any(|s| s.as_str() == key)
                                {
                                    continue;
                                }
                                // An Array value renders as `k[]=a&k[]=b`
                                // in Rails (lobsters/campfire both write
                                // one), which is why the key carries
                                // whether every site passes one — the
                                // helper needs a different parameter TYPE
                                // and a different rendering, not just a
                                // different value.
                                let is_array = matches!(&*v.node, ExprNode::Array { .. });
                                let helper = method
                                    .as_str()
                                    .strip_suffix("_url")
                                    .map(|stem| format!("{stem}_path"))
                                    .unwrap_or_else(|| method.as_str().to_string());
                                let seen = out
                                    .entry(helper)
                                    .or_default()
                                    .entry(key.to_string())
                                    .or_insert(is_array);
                                // Any scalar site demotes the key: see
                                // `QueryKey`.
                                *seen = *seen && is_array;
                            }
                        }
                    }
                }
            }
            e.node.for_each_child(&mut |c| walk(c, helpers, out));
        }
        walk(e, helpers, &mut out);
    };
    for_each_route_call_site(app, &mut collect);
    out.into_iter()
        .map(|(helper, keys)| {
            (
                helper,
                keys.into_iter()
                    .map(|(name, array)| QueryKey { name, array })
                    .collect(),
            )
        })
        .collect()
}

/// The `(route, format)` pairs a call site asked for, as
/// `<as_name>_<ext>_path` -> `(as_name, ext)`.
///
/// `lower::route_format_suffix` has already rewritten
/// `x_path(…, format: :json)` to `x_json_path(…)`, so the demand is
/// readable straight off the call sites — the same survey shape
/// `query_param_demand` uses, and for the same reason: the generator
/// takes an `&App`, not a channel from the earlier pass.
///
/// MONOMORPHIZED rather than a `format:` parameter on the base helper.
/// Rust and Go have no default arguments, so widening one shared
/// signature charges every caller for the handful that asked; and the
/// concat form it replaced (`x_path(…) + ".json"`) put the extension
/// after the QUERY STRING, which is a different URL. A function per pair
/// costs nothing to the callers that never mention a format.
fn format_variant_demand(
    app: &App,
    declared: &std::collections::HashSet<String>,
) -> std::collections::BTreeMap<String, (String, String)> {
    let mut out: std::collections::BTreeMap<String, (String, String)> = Default::default();
    // `<as_name>` for every named route, longest first: `story_path` and
    // `story_comments_path` both end in `_path`, and a name must split
    // against the LONGEST route name it starts with or `story_comments`
    // reads as route `story` with format `comments`.
    let mut names: Vec<String> =
        flatten_routes(app).into_iter().filter(|r| r.named).map(|r| r.as_name).collect();
    names.sort();
    names.dedup();
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    let mut collect = |e: &Expr| {
        fn walk(
            e: &Expr,
            names: &[String],
            declared: &std::collections::HashSet<String>,
            out: &mut std::collections::BTreeMap<String, (String, String)>,
        ) {
            if let ExprNode::Send { recv: None, method, .. } = &*e.node {
                let raw = method.as_str();
                if let Some(stem) = raw.strip_suffix("_path").or_else(|| raw.strip_suffix("_url")) {
                    // A real helper of that name wins — never shadow a
                    // route the app actually declared.
                    if !declared.contains(&format!("{stem}_path")) {
                        for n in names {
                            if let Some(ext) = stem.strip_prefix(n.as_str()).and_then(|r| r.strip_prefix('_')) {
                                if !ext.is_empty()
                                    && ext.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                                {
                                    out.insert(
                                        format!("{stem}_path"),
                                        (n.clone(), ext.to_string()),
                                    );
                                }
                                break;
                            }
                        }
                    }
                }
            }
            e.node.for_each_child(&mut |c| walk(c, names, declared, out));
        }
        walk(e, &names, declared, &mut out);
    };
    for_each_route_call_site(app, &mut collect);
    out
}

/// Every body a route helper can be called from. `for_each_hook_body_ref`
/// covers models/controllers/library classes, views carry most URL calls
/// — and TEST bodies call them too, which is where campfire's
/// `room_messages_url(@room, format: :turbo_stream)` lives. A test that
/// the emit ships is a call site like any other; leaving it out of the
/// survey means the helper it calls is built without the parameter it
/// needs, and the test fails on arity.
fn for_each_route_call_site(app: &App, f: &mut impl FnMut(&Expr)) {
    crate::lower::for_each_hook_body_ref(app, f);
    for view in &app.views {
        f(&view.body);
    }
    for tm in &app.test_modules {
        if let Some(setup) = &tm.setup {
            f(setup);
        }
        for t in &tm.tests {
            f(&t.body);
        }
        for m in &tm.helpers {
            f(&m.body);
        }
    }
}

/// Append `?k=v&k2=v2` for the query keys this helper accepts, skipping
/// each one the caller left `nil`.
///
/// Built as ONE expression rather than statements: each key contributes
/// a conditional whose separator is `?` when every earlier key was also
/// omitted and `&` otherwise, so any subset of the keys renders a
/// well-formed query string. Values go through the same `url_encode` the
/// view helpers use — a raw `q=` value carrying a space or `&` would
/// otherwise produce a URL that means something else.
fn append_query_string(path: Expr, keys: &[QueryKey]) -> Expr {
    let mut out = path;
    for (i, key) in keys.iter().enumerate() {
        let is_nil = |k: &str| -> Expr {
            send_method(var_ref(k), "nil?", Vec::new())
        };
        let key_name = key.name.as_str();
        // `?` only while nothing earlier was rendered. The first key
        // needs no test — nothing can precede it.
        let separator = if i == 0 {
            None
        } else {
            Some({
            let mut cond = is_nil(&keys[0].name);
            for earlier in keys[1..i].iter().map(|k| k.name.as_str()) {
                cond = Expr::new(
                    Span::synthetic(),
                    ExprNode::BoolOp {
                        op: crate::expr::BoolOpKind::And,
                        surface: Default::default(),
                        left: cond,
                        right: is_nil(earlier),
                    },
                );
            }
            Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond,
                    then_branch: lit_str("?".to_string()),
                    else_branch: lit_str("&".to_string()),
                },
            )
            })
        };
        let url_encode = |value: Expr| -> Expr {
            Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(Expr::new(
                        Span::synthetic(),
                        ExprNode::Const {
                            path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")],
                        },
                    )),
                    method: Symbol::from("url_encode"),
                    args: vec![send_method(value, "to_s", Vec::new())],
                    block: None,
                    parenthesized: true,
                },
            )
        };
        // Rails renders an Array option as one `k[]=v` pair per
        // element, percent-encoding the brackets. Built with
        // `map { … }.join("&")` rather than a loop: the same
        // records-to-strings shape the association `_ids` reader uses,
        // and the one every target compiles (a `while` over an index
        // does not survive the block-free targets).
        let pair = if key.array {
            let elem = Symbol::from("rh_query_value");
            let body = send_method(
                lit_str(format!("{key_name}%5B%5D=")),
                "+",
                vec![url_encode(Expr::new(
                    Span::synthetic(),
                    ExprNode::Var { id: crate::ident::VarId(0), name: elem.clone() },
                ))],
            );
            let mapped = Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(var_ref(key_name)),
                    method: Symbol::from("map"),
                    args: Vec::new(),
                    block: Some(Expr::new(
                        Span::synthetic(),
                        ExprNode::Lambda {
                            params: vec![elem],
                            block_param: None,
                            body,
                            block_style: crate::expr::BlockStyle::Brace,
                        },
                    )),
                    parenthesized: false,
                },
            );
            let joined = send_method(mapped, "join", vec![lit_str("&".to_string())]);
            let mut parts: Vec<InterpPart> = Vec::new();
            match separator {
                None => parts.push(InterpPart::Text { value: "?".to_string() }),
                Some(sep) => parts.push(InterpPart::Expr { expr: sep }),
            }
            parts.push(InterpPart::Expr { expr: joined });
            Expr::new(Span::synthetic(), ExprNode::StringInterp { parts })
        } else {
            let mut parts: Vec<InterpPart> = Vec::new();
            match separator {
                None => parts.push(InterpPart::Text { value: format!("?{key_name}=") }),
                Some(sep) => {
                    parts.push(InterpPart::Expr { expr: sep });
                    parts.push(InterpPart::Text { value: format!("{key_name}=") });
                }
            }
            parts.push(InterpPart::Expr { expr: url_encode(var_ref(key_name)) });
            Expr::new(Span::synthetic(), ExprNode::StringInterp { parts })
        };
        let arm = Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: send_method(var_ref(key_name), "nil?", Vec::new()),
                then_branch: lit_str(String::new()),
                else_branch: pair,
            },
        );
        out = send_method(out, "+", vec![arm]);
    }
    out
}

fn send_method(recv: Expr, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

/// `id`-shape params (`id`, `<x>_id`) are integer; everything else is
/// a string. Matches the existing emitter convention — EXCEPT when
/// the route's model overrides `to_param` (`slug_id`): Rails fills
/// the segment from the override's (string) value, so the helper
/// takes a String.
/// `user_ids` -> `user_id`, so `param_ty`'s name-based rule sees the
/// shape it knows. Anything not plural is its own singular.
fn singular_key(name: &str) -> &str {
    name.strip_suffix('s').unwrap_or(name)
}

fn param_ty(name: &str, slug_id: bool) -> Ty {
    if name == "id" || name.ends_with("_id") {
        if slug_id { Ty::Str } else { Ty::Int }
    } else {
        Ty::Str
    }
}

/// Walk the path template and build a `StringInterp` expression with
/// literal text segments and `Var` substitutions for `:param`s. A
/// param-less path collapses to a plain `Lit::Str`.
fn build_path_expr(
    path: &str,
    path_params: &[String],
    slug_params: &std::collections::HashSet<String>,
) -> Expr {
    if path_params.is_empty() {
        return lit_str(path.to_string());
    }
    let mut parts: Vec<InterpPart> = Vec::new();
    let mut buf = String::new();
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' {
            // Read identifier
            let mut ident = String::new();
            while let Some(&nc) = chars.peek() {
                if nc.is_alphanumeric() || nc == '_' {
                    ident.push(nc);
                    chars.next();
                } else {
                    break;
                }
            }
            if !ident.is_empty() && path_params.iter().any(|p| p == &ident) {
                if !buf.is_empty() {
                    parts.push(InterpPart::Text { value: std::mem::take(&mut buf) });
                }
                parts.push(InterpPart::Expr {
                    expr: var_ref_slug(&ident, slug_params.contains(&ident)),
                });
            } else {
                buf.push(':');
                buf.push_str(&ident);
            }
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        parts.push(InterpPart::Text { value: buf });
    }
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::StringInterp { parts }),
        Ty::Str,
    )
}

/// Build a path expr for a route whose trailing params come from Rails
/// optional groups: the required prefix always renders, each optional
/// param's segment is appended only when the arg is non-nil.
/// `/top(/:length(/page/:page))` (required=0) →
/// `"/top" + (length.nil? ? "" : "/#{length}")
///        + (page.nil?   ? "" : "/page/#{page}")`.
/// The leading `/` of an optional group stays with its chunk, so
/// `top_path()` yields `"/top"`, not `"/top/"`.
fn build_optional_path_expr(
    path: &str,
    seg_params: &[String],
    required: usize,
    slug_params: &std::collections::HashSet<String>,
) -> Expr {
    let optional: std::collections::HashSet<&str> =
        seg_params[required..].iter().map(|s| s.as_str()).collect();
    let mut base_parts: Vec<InterpPart> = Vec::new();
    // (param-name, its conditionally-appended segment parts)
    let mut chunks: Vec<(String, Vec<InterpPart>)> = Vec::new();
    let mut buf = String::new();
    let mut in_optional = false;
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c != ':' {
            buf.push(c);
            continue;
        }
        let mut ident = String::new();
        while let Some(&nc) = chars.peek() {
            if nc.is_alphanumeric() || nc == '_' {
                ident.push(nc);
                chars.next();
            } else {
                break;
            }
        }
        if !seg_params.iter().any(|p| p == &ident) {
            buf.push(':');
            buf.push_str(&ident);
            continue;
        }
        if optional.contains(ident.as_str()) {
            let mut chunk: Vec<InterpPart> = Vec::new();
            if !in_optional {
                // First optional param: split pending text at its last `/`
                // — that slash opens the optional group and belongs to the
                // chunk; everything before it is the always-present base.
                let split = buf.rfind('/').unwrap_or(0);
                let (base_text, chunk_prefix) = buf.split_at(split);
                if !base_text.is_empty() {
                    base_parts.push(InterpPart::Text { value: base_text.to_string() });
                }
                if !chunk_prefix.is_empty() {
                    chunk.push(InterpPart::Text { value: chunk_prefix.to_string() });
                }
                in_optional = true;
            } else if !buf.is_empty() {
                chunk.push(InterpPart::Text { value: buf.clone() });
            }
            chunk.push(InterpPart::Expr { expr: var_ref_slug(&ident, slug_params.contains(&ident)) });
            chunks.push((ident.clone(), chunk));
            buf.clear();
        } else {
            // Required param — stays in the always-present base.
            if !buf.is_empty() {
                base_parts.push(InterpPart::Text { value: std::mem::take(&mut buf) });
            }
            base_parts.push(InterpPart::Expr { expr: var_ref_slug(&ident, slug_params.contains(&ident)) });
        }
    }
    if !buf.is_empty() {
        match chunks.last_mut() {
            Some((_, last)) => last.push(InterpPart::Text { value: buf }),
            None => base_parts.push(InterpPart::Text { value: buf }),
        }
    }
    let mut result = parts_to_expr(base_parts);
    for (pname, chunk) in chunks {
        // `<param>.nil? ? "" : "<segment>"`
        let cond = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(var_ref(&pname)),
                    method: Symbol::from("nil?"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            ),
            Ty::Bool,
        );
        let ternary = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond,
                    then_branch: lit_str(String::new()),
                    else_branch: parts_to_expr(chunk),
                },
            ),
            Ty::Str,
        );
        result = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(result),
                    method: Symbol::from("+"),
                    args: vec![ternary],
                    block: None,
                    parenthesized: false,
                },
            ),
            Ty::Str,
        );
    }
    result
}

/// Collapse `InterpPart`s to an expr: empty → `""`, a lone text →
/// `Lit::Str`, otherwise a `StringInterp` typed `Str`.
fn parts_to_expr(parts: Vec<InterpPart>) -> Expr {
    match parts.as_slice() {
        [] => lit_str(String::new()),
        [InterpPart::Text { value }] => lit_str(value.clone()),
        _ => with_ty(
            Expr::new(Span::synthetic(), ExprNode::StringInterp { parts }),
            Ty::Str,
        ),
    }
}

fn var_ref(name: &str) -> Expr {
    var_ref_slug(name, false)
}

fn var_ref_slug(name: &str, slug_id: bool) -> Expr {
    let sym = Symbol::from(name);
    with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: sym },
        ),
        param_ty(name, slug_id),
    )
}

// Avoid unused-import noise — `Literal` is referenced via lit_str helper only.
#[allow(dead_code)]
const _: Option<Literal> = None;
