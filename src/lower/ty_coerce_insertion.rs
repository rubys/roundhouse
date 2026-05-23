//! Explicit type-coercion insertion across LibraryClass method bodies.
//!
//! Walks each Send in each method body and, where a positional arg's
//! `Ty` is narrower than the callee's declared param `Ty`, wraps the
//! arg in `ExprNode::Cast { value, target_ty }`. Downstream emitters
//! consume the Cast nodes per-target — rust2 widens
//! `HashMap<K,V>` via `into_iter().map().collect()`, go2 produces a
//! `map[string]any` conversion, TS/Crystal/Ruby treat Cast as identity
//! (their typers handle widening natively, so the Cast node is a
//! pass-through).
//!
//! This replaces emit-time back-propagation that derives the same
//! information from arg-vs-param Ty comparisons at every call site in
//! every emitter. Landing the typing intent once in the IR means each
//! emitter just consumes a uniform construct.
//!
//! Stage 2: Hash-widening family. When a callee's positional param is
//! declared `Hash[_, untyped]` (`Ty::Hash { value: Untyped, .. }`) and
//! the arg's inferred `Ty` is a different concrete Hash shape (or the
//! arg is an inline Hash literal), wrap the arg in `Cast`. Other
//! coercion families (`T → Option<T>` Some-wrap, `Sym → Str` key
//! rewrites) land in subsequent stages.

use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::{ParamKind, Ty};
use std::collections::HashMap;

use crate::lower::controller_to_library::util::map_expr;

/// (ClassName, method_name) → param_tys. ClassName is the last segment
/// of the LC's name (e.g. `ViewHelpers` for `ActionView::ViewHelpers`)
/// to match how `Const { path }` recv arms look up at emit time.
type CalleeRegistry = HashMap<String, HashMap<String, Vec<Ty>>>;

/// Insert `ExprNode::Cast` wrappers at call-site arg positions where
/// the callee's declared param Ty widens the arg's Ty. Mutates `lcs`
/// in place.
pub fn insert_ty_coercions(lcs: &mut [LibraryClass]) {
    let registry = build_registry(lcs);
    for lc in lcs.iter_mut() {
        for method in &mut lc.methods {
            method.body = rewrite_expr(&method.body, &registry);
        }
    }
}

fn build_registry(lcs: &[LibraryClass]) -> CalleeRegistry {
    let mut out: CalleeRegistry = HashMap::new();
    for lc in lcs {
        let raw = lc.name.0.as_str();
        let class_name = raw.rsplit("::").next().unwrap_or(raw).to_string();
        let entry = out.entry(class_name).or_default();
        for m in &lc.methods {
            let param_tys: Vec<Ty> = match m.signature.as_ref() {
                Some(Ty::Fn { params, .. }) => params
                    .iter()
                    .filter(|p| {
                        !matches!(p.kind, ParamKind::Block | ParamKind::KeywordRest)
                    })
                    .map(|p| p.ty.clone())
                    .collect(),
                _ => continue,
            };
            entry.insert(m.name.as_str().to_string(), param_tys);
        }
    }
    out
}

fn rewrite_expr(body: &Expr, registry: &CalleeRegistry) -> Expr {
    map_expr(body, &|e: &Expr| -> Option<Expr> {
        let ExprNode::Send { recv, method, args, block, parenthesized } = &*e.node else {
            return None;
        };
        // Only handle Const-recv class method calls for now — that's the
        // canonical `ViewHelpers::render_attrs(form_attrs)` shape. Sibling
        // SelfRef/implicit-self resolution would require per-class context;
        // deferred until the Const case validates the mechanism end-to-end.
        let class_name = match recv.as_ref().map(|r| &*r.node) {
            Some(ExprNode::Const { path }) => {
                path.last().map(|s| s.as_str().to_string())?
            }
            _ => return None,
        };
        let param_tys = registry.get(&class_name)?.get(method.as_str())?;
        let mut new_args = Vec::with_capacity(args.len());
        let mut changed = false;
        for (idx, arg) in args.iter().enumerate() {
            let Some(param_ty) = param_tys.get(idx) else {
                new_args.push(arg.clone());
                continue;
            };
            if needs_hash_widening(param_ty, arg) {
                new_args.push(wrap_in_cast(arg, param_ty));
                changed = true;
            } else {
                new_args.push(arg.clone());
            }
        }
        if !changed {
            return None;
        }
        Some(Expr {
            span: e.span,
            node: Box::new(ExprNode::Send {
                recv: recv.clone(),
                method: method.clone(),
                args: new_args,
                block: block.clone(),
                parenthesized: *parenthesized,
            }),
            ty: e.ty.clone(),
            effects: e.effects.clone(),
            leading_blank_line: e.leading_blank_line,
            diagnostic: e.diagnostic.clone(),
            str_coercion: e.str_coercion,
        })
    })
}

/// Hash-widening trigger: param is `Hash[_, untyped]` AND arg is either
/// a Hash literal OR a value whose inferred Ty is a different Hash
/// shape (concrete value-ty, not also Untyped). The "different shape"
/// check avoids wrapping `Hash[String, untyped]` flowing into
/// `Hash[String, untyped]` (a no-op widen).
fn needs_hash_widening(param_ty: &Ty, arg: &Expr) -> bool {
    let Ty::Hash { value: pv, .. } = param_ty else {
        return false;
    };
    if !matches!(pv.as_ref(), Ty::Untyped) {
        return false;
    }
    // Skip args already wrapped in Cast — idempotency.
    if matches!(&*arg.node, ExprNode::Cast { .. }) {
        return false;
    }
    if matches!(&*arg.node, ExprNode::Hash { .. }) {
        return true;
    }
    if let Some(Ty::Hash { value: av, .. }) = arg.ty.as_ref() {
        if !matches!(av.as_ref(), Ty::Untyped) {
            return true;
        }
    }
    false
}

fn wrap_in_cast(arg: &Expr, target_ty: &Ty) -> Expr {
    Expr {
        span: arg.span,
        node: Box::new(ExprNode::Cast {
            value: arg.clone(),
            target_ty: target_ty.clone(),
        }),
        ty: Some(target_ty.clone()),
        effects: arg.effects.clone(),
        leading_blank_line: arg.leading_blank_line,
        diagnostic: None,
        str_coercion: None,
    }
}

// Suppress unused-import warnings for symbols reserved for future
// stages (kwarg-key Cast nodes will need Span::synthetic + Symbol).
#[allow(dead_code)]
fn _reserved_use() {
    let _ = Span::synthetic;
    let _: Option<Symbol> = None;
}
