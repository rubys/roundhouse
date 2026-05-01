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
use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver, Param};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::lower::routes::{flatten_routes, FlatRoute};
use crate::lower::typing::{fn_sig, lit_str, with_ty};
use crate::span::Span;
use crate::ty::Ty;

/// Build a `RouteHelpers` LibraryClass from `app.routes`. Returns
/// `None` when the app has no routes.
pub fn lower_routes_to_library_class(app: &App) -> Option<LibraryClass> {
    let flat = flatten_routes(app);
    if flat.is_empty() {
        return None;
    }
    let owner = ClassId(Symbol::from("RouteHelpers"));
    // Dedupe: multiple HTTP verbs on the same path collapse to a
    // single helper (`articles` for both index/create — same URL).
    // First-occurrence wins; the as_name + path are identical so the
    // method body is the same.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut methods: Vec<MethodDef> = Vec::new();
    for route in &flat {
        let helper = format!("{}_path", route.as_name);
        if !seen.insert(helper.clone()) {
            continue;
        }
        methods.push(build_helper_method(&owner, &helper, route));
    }
    Some(LibraryClass {
        name: owner,
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods,
    })
}

fn build_helper_method(owner: &ClassId, helper_name: &str, route: &FlatRoute) -> MethodDef {
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
    MethodDef {
        name: Symbol::from(helper_name),
        receiver: MethodReceiver::Class,
        params,
        body,
        signature: Some(fn_sig(sig_params, Ty::Str)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
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
