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

use crate::App;
use crate::dialect::{HttpMethod, LibraryFunction, Param};
use crate::effect::EffectSet;
use crate::expr::{ArrayStyle, Expr, ExprNode, InterpPart, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::lower::routes::{flatten_routes, FlatRoute};
use crate::lower::typing::{fn_sig, lit_str, lit_sym, with_ty};
use crate::span::Span;
use crate::ty::Ty;

/// Build the `Routes` dispatch module — `Routes.table -> Array<Route>`
/// (one `Route` instance per concrete `(verb, pattern, controller,
/// action)`) and `Routes.root -> Route` (the shorthand `root "c#a"`
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
    let module_path = vec![Symbol::from("Routes")];
    // Same partition the per-target Spinel emit used: path "/" goes
    // to `Routes.root`, everything else to `Routes.table`. Callers
    // typically combine them at use site (`[Routes.root] +
    // Routes.table`).
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
pub(crate) fn controller_symbol(class_name: &str) -> String {
    let base = class_name.strip_suffix("Controller").unwrap_or(class_name);
    crate::naming::snake_case(base)
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
        funcs.push(build_helper_function(&module_path, &helper, route, app));
    }
    funcs
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
) -> LibraryFunction {
    let slug_id = model_overrides_to_param(route.controller.0.as_str(), helper_name, app);
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

    let mut params: Vec<Param> = seg_params
        .iter()
        .map(|p| Param::positional(Symbol::from(p.clone())))
        .collect();
    let mut sig_params: Vec<(Symbol, Ty)> = seg_params
        .iter()
        .map(|p| (Symbol::from(p.clone()), param_ty(p, slug_id)))
        .collect();
    let mut body = build_path_expr(path, &seg_params, slug_id);
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
        // <path> + (format ? ".#{format}" : "")
        let dot_format = Expr::new(
            Span::synthetic(),
            ExprNode::StringInterp {
                parts: vec![
                    InterpPart::Text { value: ".".to_string() },
                    InterpPart::Expr { expr: var_ref("format") },
                ],
            },
        );
        let suffix = Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: var_ref("format"),
                then_branch: dot_format,
                else_branch: lit_str(String::new()),
            },
        );
        body = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(body),
                method: Symbol::from("+"),
                args: vec![suffix],
                block: None,
                parenthesized: false,
            },
        );
    }
    LibraryFunction {
        module_path: module_path.to_vec(),
        name: Symbol::from(helper_name),
        params,
        body,
        signature: Some(fn_sig(sig_params, Ty::Str)),
        effects: EffectSet::default(),
        is_async: false,
    }
}

/// `id`-shape params (`id`, `<x>_id`) are integer; everything else is
/// a string. Matches the existing emitter convention — EXCEPT when
/// the route's model overrides `to_param` (`slug_id`): Rails fills
/// the segment from the override's (string) value, so the helper
/// takes a String.
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
fn build_path_expr(path: &str, path_params: &[String], slug_id: bool) -> Expr {
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
                    expr: var_ref_slug(&ident, slug_id),
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
