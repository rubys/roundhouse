//! Stage 4 (initial slice) — `OPTION_WRAP` decide-pass walker.
//!
//! Migrates the **Family 6 (owned-producing)** branch of
//! `expr/send/coerce.rs::coerce_arg_for_param_ty` from emit-time
//! Ty-inspection into a decide-pass IR-stamp. For each `Send` whose
//! callee resolves through `EmitCtx::lookup_param_tys`, walks the
//! arg list and stamps `OPTION_WRAP` on each arg whose
//! `(arg, param_ty)` pair matches the Family 6 conditions:
//!
//! - `param_ty` is `Option<U>` (`Union { Nil, U }`)
//! - `U` is not `Untyped`
//! - arg's IR shape is owned-producing (`Var` / `Send` / `Ivar`),
//!   peek-through `Cast` for the `lower::ty_coerce_insertion`-
//!   wrapped sites
//! - arg's body-typer `Ty` matches `U` exactly
//!
//! Render reads the bit at the single `Family 6` site in coerce.rs;
//! when set, the emit is `Some({raw})` directly — no re-derivation
//! from Ty.
//!
//! Today's scope: **Const-recv calls only**. Other recv shapes
//! (`SelfRef` instance-method, Var-recv class-method) flow through
//! `current_class_method_param_tys` / `class_method_param_ty`
//! thread-locals that EmitCtx Phase 1 (#24) hasn't yet lifted. Those
//! paths keep their emit-time Family 6 fallback for now. When
//! EmitCtx Phase 2 lifts the per-class registries, this walker's
//! coverage expands to include them.
//!
//! Other coerce families (1 = Hash widen, 2 = Value→primitive,
//! 3 = primitive→Value, 5 = owned-T clone, 7 = Option<Str>→&str)
//! stay in coerce.rs at emit time. A follow-on commit promotes
//! them to a `COERCE_FAMILY` bit-group on `Expr.decisions`.
//!
//! Runs AFTER `with_emit_ctx` installs the registry; the per-app-
//! code-category decide_classes invocations from earlier in
//! `rust2.rs::emit` only handle bits that don't need cross-LC
//! lookup (parens, str_color, last_use). This walker is wired via
//! `decide_classes_late` and runs once on each app-code LC slice.

use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode, InterpPart, LValue};
use crate::ty::Ty;

use super::super::EmitCtx;
use super::bits::OPTION_WRAP;

/// Walk every method body in every class and stamp `OPTION_WRAP`
/// per Family 6 branch A on Const-recv Send arg positions.
pub fn stamp(classes: &mut [LibraryClass], ctx: &EmitCtx) {
    for class in classes {
        for m in class.methods.iter_mut() {
            walk(&mut m.body, ctx);
        }
    }
}

fn walk(e: &mut Expr, ctx: &EmitCtx) {
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &mut *e.node {
        if let ExprNode::Const { path } = &*r.node {
            if let Some(class) = path.last().map(|s| s.as_str().to_string()) {
                if let Some(param_tys) = ctx.lookup_param_tys(&class, method.as_str()) {
                    for (i, arg) in args.iter_mut().enumerate() {
                        if let Some(param_ty) = param_tys.get(i) {
                            if should_option_wrap(arg, param_ty) {
                                arg.decisions |= OPTION_WRAP;
                            }
                        }
                    }
                }
            }
        }
    }
    walk_children(e, ctx);
}

fn walk_children(e: &mut Expr, ctx: &EmitCtx) {
    match &mut *e.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv.as_mut() {
                walk(r, ctx);
            }
            for a in args.iter_mut() {
                walk(a, ctx);
            }
            if let Some(b) = block.as_mut() {
                walk(b, ctx);
            }
        }
        ExprNode::Assign { target, value } => {
            walk_lvalue(target, ctx);
            walk(value, ctx);
        }
        ExprNode::MultiAssign { targets, value } => {
            for t in targets {
                walk_lvalue(t, ctx);
            }
            walk(value, ctx);
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk(k, ctx);
                walk(v, ctx);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                walk(el, ctx);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    walk(expr, ctx);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk(left, ctx);
            walk(right, ctx);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk(cond, ctx);
            walk(then_branch, ctx);
            walk(else_branch, ctx);
        }
        ExprNode::Case { scrutinee, arms } => {
            walk(scrutinee, ctx);
            for arm in arms {
                if let Some(g) = arm.guard.as_mut() {
                    walk(g, ctx);
                }
                walk(&mut arm.body, ctx);
            }
        }
        ExprNode::While { cond, body, .. } => {
            walk(cond, ctx);
            walk(body, ctx);
        }
        ExprNode::Seq { exprs } => {
            for x in exprs {
                walk(x, ctx);
            }
        }
        ExprNode::Lambda { body, .. } => walk(body, ctx),
        ExprNode::Return { value } => walk(value, ctx),
        ExprNode::Raise { value } => walk(value, ctx),
        ExprNode::Yield { args } => {
            for a in args {
                walk(a, ctx);
            }
        }
        ExprNode::Next { value } => {
            if let Some(v) = value.as_mut() {
                walk(v, ctx);
            }
        }
        ExprNode::Super { args } => {
            if let Some(arglist) = args.as_mut() {
                for a in arglist {
                    walk(a, ctx);
                }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            walk(body, ctx);
            for r in rescues {
                walk(&mut r.body, ctx);
            }
            if let Some(eb) = else_branch.as_mut() {
                walk(eb, ctx);
            }
            if let Some(en) = ensure.as_mut() {
                walk(en, ctx);
            }
        }
        ExprNode::RescueModifier { expr, fallback } => {
            walk(expr, ctx);
            walk(fallback, ctx);
        }
        ExprNode::Let { value, body, .. } => {
            walk(value, ctx);
            walk(body, ctx);
        }
        ExprNode::Apply { fun, args, block } => {
            walk(fun, ctx);
            for a in args {
                walk(a, ctx);
            }
            if let Some(b) = block.as_mut() {
                walk(b, ctx);
            }
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin.as_mut() {
                walk(b, ctx);
            }
            if let Some(en) = end.as_mut() {
                walk(en, ctx);
            }
        }
        ExprNode::Cast { value, .. } => walk(value, ctx),
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef => {}
    }
}

fn walk_lvalue(lv: &mut LValue, ctx: &EmitCtx) {
    match lv {
        LValue::Var { .. } | LValue::Ivar { .. } => {}
        LValue::Attr { recv, .. } => walk(recv, ctx),
        LValue::Index { recv, index } => {
            walk(recv, ctx);
            walk(index, ctx);
        }
    }
}

/// Family 6 branch A: should we wrap this arg in `Some(...)`?
///
/// Mirrors the conditions in `expr/send/coerce.rs::coerce_arg_for_
/// param_ty` lines 80–125. Two gates:
///
/// 1. `param_ty` is `Option<U>` and `U` is concrete (not `Untyped`)
/// 2. Peek-through `Cast` for the lowerer-wrapped form; check that
///    the inner is owned-producing (`Var` / `Send` / `Ivar`) and its
///    body-typer `Ty` matches `U` exactly.
///
/// The matching `arg.ty == Some(U)` (not the peeled type) is
/// load-bearing: an arg whose own type is already `Option<U>`
/// (`self.flash.get("notice")` returning `Option<String>`) must NOT
/// be double-wrapped to `Option<Option<U>>`. The body-typer's
/// matching `Option<U>` shape means the bare emit type-checks
/// without `Some(...)`.
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
