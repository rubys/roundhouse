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
use crate::ident::{Symbol, VarId};
use crate::lower::routes::{flatten_routes, FlatRoute};
use crate::lower::typing::{fn_sig, lit_str, lit_sym, with_ty};
use crate::span::Span;
use crate::ty::Ty;

/// Build the `Routes` dispatch module — `Routes.table -> Array<Hash>`
/// (one hash per concrete `(method, pattern, controller, action)`)
/// and `Routes.root -> Hash` (the shorthand `root "c#a"` route, when
/// present). Empty when `app.routes` has no entries.
///
/// Symbol-keyed hashes (`{ method:, pattern:, controller:, action: }`)
/// matching the spinel-blog runtime's `Router.match(method, path,
/// table)` convention. TS callers see the same data shape via the
/// universal LibraryFunction emit (string keys in TS, symbol keys
/// in Ruby — Sym renders as a quoted string in TS object literals).
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

    let route_hash_ty = Ty::Hash {
        key: Box::new(Ty::Sym),
        value: Box::new(Ty::Untyped),
    };

    let mut out: Vec<LibraryFunction> = Vec::new();

    let table_body = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Array {
                elements: table_routes
                    .iter()
                    .map(|r| build_route_hash(r, &route_hash_ty))
                    .collect(),
                style: ArrayStyle::Brackets,
            },
        ),
        Ty::Array { elem: Box::new(route_hash_ty.clone()) },
    );
    out.push(LibraryFunction {
        module_path: module_path.clone(),
        name: Symbol::from("table"),
        params: Vec::new(),
        body: table_body,
        signature: Some(fn_sig(
            vec![],
            Ty::Array { elem: Box::new(route_hash_ty.clone()) },
        )),
        effects: EffectSet::default(),
        is_async: false,
    });

    if let Some(r) = root_routes.first() {
        let root_body = build_route_hash(r, &route_hash_ty);
        out.push(LibraryFunction {
            module_path,
            name: Symbol::from("root"),
            params: Vec::new(),
            body: root_body,
            signature: Some(fn_sig(vec![], route_hash_ty)),
            effects: EffectSet::default(),
            is_async: false,
        });
    }

    out
}

/// Build one route hash literal: `{ method: "GET", pattern: "/x",
/// controller: :articles, action: :index }`. Method is a string;
/// controller and action are symbols matching what the spinel
/// router's `Router.match` expects.
fn build_route_hash(r: &FlatRoute, hash_ty: &Ty) -> Expr {
    let method_str = match r.method {
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
    let entries = vec![
        (
            lit_sym(Symbol::from("method")),
            lit_str(method_str.to_string()),
        ),
        (
            lit_sym(Symbol::from("pattern")),
            lit_str(r.path.clone()),
        ),
        (
            lit_sym(Symbol::from("controller")),
            lit_sym(Symbol::from(controller_sym)),
        ),
        (
            lit_sym(Symbol::from("action")),
            lit_sym(r.action.clone()),
        ),
    ];
    with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Hash { entries, kwargs: false },
        ),
        hash_ty.clone(),
    )
}

/// `ArticlesController` → `articles` (the controller-symbol form
/// the spinel router uses). Mirrors the existing per-target convention.
fn controller_symbol(class_name: &str) -> String {
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
        let helper = format!("{}_path", route.as_name);
        if !seen.insert(helper.clone()) {
            continue;
        }
        funcs.push(build_helper_function(&module_path, &helper, route));
    }
    funcs
}

fn build_helper_function(
    module_path: &[Symbol],
    helper_name: &str,
    route: &FlatRoute,
) -> LibraryFunction {
    let params: Vec<Param> = route
        .path_params
        .iter()
        .map(|p| Param::positional(Symbol::from(p.clone())))
        .collect();
    let sig_params: Vec<(Symbol, Ty)> = route
        .path_params
        .iter()
        .map(|p| (Symbol::from(p.clone()), param_ty(p)))
        .collect();
    let body = build_path_expr(&route.path, &route.path_params);
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
/// a string. Matches the existing emitter convention.
fn param_ty(name: &str) -> Ty {
    if name == "id" || name.ends_with("_id") {
        Ty::Int
    } else {
        Ty::Str
    }
}

/// Walk the path template and build a `StringInterp` expression with
/// literal text segments and `Var` substitutions for `:param`s. A
/// param-less path collapses to a plain `Lit::Str`.
fn build_path_expr(path: &str, path_params: &[String]) -> Expr {
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
                    expr: var_ref(&ident),
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
    let sym = Symbol::from(name);
    with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: sym },
        ),
        param_ty(name),
    )
}

// Avoid unused-import noise — `Literal` is referenced via lit_str helper only.
#[allow(dead_code)]
const _: Option<Literal> = None;
