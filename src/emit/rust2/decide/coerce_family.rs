//! Stage 4 — `OPTION_WRAP` decide-pass walker.
//!
//! Migrates the **Family 6 branch-A** (`T → Option<T>` Some-wrap,
//! owned-producing arg) decision from emit-time Ty-inspection in
//! `expr/send/coerce.rs::coerce_arg_for_param_ty` into a decide-pass
//! IR-stamp using the `OPTION_WRAP` bit (rust2-local, position 35).
//!
//! Walker resolves the callee's param-Ty list per recv shape, then
//! per arg index checks whether `(arg, param_ty)` matches Family 6
//! conditions. Stamps `OPTION_WRAP` on each matching arg; render's
//! `coerce.rs` Family 6 short-circuits when the bit is set.
//!
//! ## Recv-shape coverage
//!
//! - **`Const::method(args)`** — cross-LC lookup via
//!   `EmitCtx::lookup_param_tys` (registry populated by
//!   `collect_global_class_methods`, available inside `with_emit_ctx`).
//! - **`self.method(args)`** / **implicit-self `method(args)`** —
//!   sibling method on the current class. The walker self-computes
//!   a local `method → Vec<Ty>` map from the LC's own methods, same
//!   shape `library.rs::collect_class_method_param_tys` builds at
//!   class scope entry.
//! - **`var.method(args)` / `@ivar.method(args)`** with the recv's
//!   `Ty` resolving to `Ty::Class { id }` — cross-LC lookup via
//!   `EmitCtx::lookup_param_tys(id.last_segment(), method)`. Uses
//!   the same last-segment rule as `collect_global_class_methods`
//!   for the class-name key.
//!
//! ## Family 6 conditions (mirrored from coerce.rs)
//!
//! - `param_ty` is `Option<U>` (`Union { Nil, U }`) with concrete `U`
//!   (not `Untyped`)
//! - arg peeked through any `Cast { target_ty: Option<U> }` wrapper
//! - inner is `Var` / `Send` / `Ivar` (owned-producing)
//! - `probe.ty == Some(U)` exactly — protects against double-wrapping
//!   when the arg's own type is already `Option<U>`
//!
//! ## Out of scope
//!
//! - **Family 6 branch B** (literal Str/Sym arg → `Option<String>`
//!   with inner `.to_string()`). The render at `coerce.rs` keeps
//!   this branch; composes naturally with `STR_TO_OWNED` from Stage 2.
//! - **Other coerce families** (1 = Hash widen, 2 = Value→primitive,
//!   3 = primitive→Value, 5 = owned-T clone, 7 = Option<Str>→&str).
//!   They follow the same pattern; defer to follow-on commits that
//!   allocate `COERCE_FAMILY` bit-group variants.
//! - **Runtime files** (`runtime/ruby/*.rb` transpiled to Rust).
//!   Emitted before `with_emit_ctx` is entered, so the late decide
//!   pass doesn't reach them — they use coerce.rs Family 6 fallback.

use std::collections::HashMap;

use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode, InterpPart, LValue};
use crate::ident::Symbol;
use crate::ty::{ParamKind, Ty};

use super::super::EmitCtx;
use super::bits::OPTION_WRAP;

/// Walk every method body in every class and stamp `OPTION_WRAP`
/// per Family 6 branch A. Per-class state (sibling-method param
/// tys) is computed once per class from the LC itself, mirroring
/// the `library.rs::collect_class_method_param_tys` thread-local
/// build done at class-scope entry.
pub fn stamp(classes: &mut [LibraryClass], ctx: &EmitCtx) {
    for class in classes {
        let local = build_local_method_param_tys(&class.methods);
        for m in class.methods.iter_mut() {
            walk(&mut m.body, ctx, &local);
        }
    }
}

/// Build a `method-name → Vec<Param-Ty>` map from a class's own
/// methods. Mirrors `library.rs::collect_class_method_param_tys`:
/// drops Block / KeywordRest positions and aliases `initialize` →
/// `new` so `Self::new(args)` / `Article::new(args)` resolve to
/// the constructor's param list.
fn build_local_method_param_tys(
    methods: &[crate::dialect::MethodDef],
) -> HashMap<String, Vec<Ty>> {
    let mut out: HashMap<String, Vec<Ty>> = HashMap::new();
    for m in methods {
        let tys: Vec<Ty> = match m.signature.as_ref() {
            Some(Ty::Fn { params, .. }) => params
                .iter()
                .filter(|p| !matches!(p.kind, ParamKind::Block | ParamKind::KeywordRest))
                .map(|p| p.ty.clone())
                .collect(),
            _ => continue,
        };
        if m.name.as_str() == "initialize" {
            out.insert("new".to_string(), tys.clone());
        }
        out.insert(m.name.as_str().to_string(), tys);
    }
    out
}

fn walk(e: &mut Expr, ctx: &EmitCtx, local: &HashMap<String, Vec<Ty>>) {
    if let ExprNode::Send { recv, method, args, .. } = &mut *e.node {
        if let Some(param_tys) = resolve_param_tys(recv.as_ref(), method, ctx, local) {
            for (i, arg) in args.iter_mut().enumerate() {
                if let Some(param_ty) = param_tys.get(i) {
                    if should_option_wrap(arg, param_ty) {
                        arg.decisions |= OPTION_WRAP;
                    }
                }
            }
        }
    }
    walk_children(e, ctx, local);
}

/// Resolve the callee's param-Ty list based on the recv shape.
/// Mirrors the dispatch logic in `expr/send/mod.rs::emit_send`'s
/// arm chain but for read-only lookup (no Tys built; just looked
/// up).
fn resolve_param_tys(
    recv: Option<&Expr>,
    method: &Symbol,
    ctx: &EmitCtx,
    local: &HashMap<String, Vec<Ty>>,
) -> Option<Vec<Ty>> {
    let method_name = method.as_str();
    let recv_node = recv.map(|r| &*r.node);
    match recv_node {
        Some(ExprNode::Const { path }) => {
            // Cross-LC lookup. The registry keys on the LAST segment
            // of the path (matches `collect_global_class_methods`).
            let class = path.last()?.as_str();
            ctx.lookup_param_tys(class, method_name)
        }
        Some(ExprNode::SelfRef) | None => {
            // Sibling method on the current class. Implicit-self
            // bare calls (`Send { recv: None }`) also resolve here
            // when they're not param-shadowing (the param-shadow
            // case is a Var read, handled by emit_send's preamble
            // and doesn't reach a Send arg position with the param
            // name as the method).
            local.get(method_name).cloned()
        }
        Some(ExprNode::Var { .. } | ExprNode::Ivar { .. }) => {
            // Var/Ivar with known Class Ty → cross-LC lookup.
            let recv_expr = recv?;
            let id = match recv_expr.ty.as_ref()? {
                Ty::Class { id, .. } => id,
                _ => return None,
            };
            let raw = id.0.as_str();
            let class = raw.rsplit("::").next().unwrap_or(raw);
            ctx.lookup_param_tys(class, method_name)
        }
        _ => None,
    }
}

fn walk_children(e: &mut Expr, ctx: &EmitCtx, local: &HashMap<String, Vec<Ty>>) {
    match &mut *e.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv.as_mut() {
                walk(r, ctx, local);
            }
            for a in args.iter_mut() {
                walk(a, ctx, local);
            }
            if let Some(b) = block.as_mut() {
                walk(b, ctx, local);
            }
        }
        ExprNode::Assign { target, value } => {
            walk_lvalue(target, ctx, local);
            walk(value, ctx, local);
        }
        ExprNode::MultiAssign { targets, value } => {
            for t in targets {
                walk_lvalue(t, ctx, local);
            }
            walk(value, ctx, local);
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk(k, ctx, local);
                walk(v, ctx, local);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                walk(el, ctx, local);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    walk(expr, ctx, local);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk(left, ctx, local);
            walk(right, ctx, local);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk(cond, ctx, local);
            walk(then_branch, ctx, local);
            walk(else_branch, ctx, local);
        }
        ExprNode::Case { scrutinee, arms } => {
            walk(scrutinee, ctx, local);
            for arm in arms {
                if let Some(g) = arm.guard.as_mut() {
                    walk(g, ctx, local);
                }
                walk(&mut arm.body, ctx, local);
            }
        }
        ExprNode::While { cond, body, .. } => {
            walk(cond, ctx, local);
            walk(body, ctx, local);
        }
        ExprNode::Seq { exprs } => {
            for x in exprs {
                walk(x, ctx, local);
            }
        }
        ExprNode::Lambda { body, .. } => walk(body, ctx, local),
        ExprNode::Return { value } => walk(value, ctx, local),
        ExprNode::Raise { value } => walk(value, ctx, local),
        ExprNode::Yield { args } => {
            for a in args {
                walk(a, ctx, local);
            }
        }
        ExprNode::Next { value } => {
            if let Some(v) = value.as_mut() {
                walk(v, ctx, local);
            }
        }
        ExprNode::Super { args } => {
            if let Some(arglist) = args.as_mut() {
                for a in arglist {
                    walk(a, ctx, local);
                }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            walk(body, ctx, local);
            for r in rescues {
                walk(&mut r.body, ctx, local);
            }
            if let Some(eb) = else_branch.as_mut() {
                walk(eb, ctx, local);
            }
            if let Some(en) = ensure.as_mut() {
                walk(en, ctx, local);
            }
        }
        ExprNode::RescueModifier { expr, fallback } => {
            walk(expr, ctx, local);
            walk(fallback, ctx, local);
        }
        ExprNode::Let { value, body, .. } => {
            walk(value, ctx, local);
            walk(body, ctx, local);
        }
        ExprNode::Apply { fun, args, block } => {
            walk(fun, ctx, local);
            for a in args {
                walk(a, ctx, local);
            }
            if let Some(b) = block.as_mut() {
                walk(b, ctx, local);
            }
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin.as_mut() {
                walk(b, ctx, local);
            }
            if let Some(en) = end.as_mut() {
                walk(en, ctx, local);
            }
        }
        ExprNode::Cast { value, .. } => walk(value, ctx, local),
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef => {}
    }
}

fn walk_lvalue(lv: &mut LValue, ctx: &EmitCtx, local: &HashMap<String, Vec<Ty>>) {
    match lv {
        LValue::Var { .. } | LValue::Ivar { .. } => {}
        LValue::Attr { recv, .. } => walk(recv, ctx, local),
        LValue::Index { recv, index } => {
            walk(recv, ctx, local);
            walk(index, ctx, local);
        }
    }
}

/// Family 6 branch A predicate. See module docstring for the gates.
fn should_option_wrap(arg: &Expr, param_ty: &Ty) -> bool {
    if !is_option_ty(param_ty) {
        return false;
    }
    let inner = peel_nil(param_ty);
    if matches!(inner, Ty::Untyped) {
        return false;
    }
    let probe: &Expr = if let ExprNode::Cast { value, .. } = &*arg.node {
        value
    } else {
        arg
    };
    let owned_producing = matches!(
        &*probe.node,
        ExprNode::Var { .. } | ExprNode::Send { .. } | ExprNode::Ivar { .. }
    );
    owned_producing && probe.ty.as_ref() == Some(inner)
}

fn is_option_ty(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Nil))
    )
}

fn peel_nil(ty: &Ty) -> &Ty {
    if let Ty::Union { variants } = ty {
        for v in variants {
            if !matches!(v, Ty::Nil) {
                return v;
            }
        }
    }
    ty
}
