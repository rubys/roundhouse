//! Flow-sensitive type narrowing through nil and class checks.
//!
//! When the body-typer sees an `if` whose condition pattern-matches
//! as a narrowing predicate (`x.nil?`, `x == nil`, `!x.nil?`,
//! `x.is_a?(T)`, etc.), the branches are analyzed under derived
//! [`Ctx`]s where the variable's type has been narrowed to reflect
//! what the condition guarantees.
//!
//! Today's predicates cover the nil / class-check family. Negation
//! is recognized (`!x.nil?`). Multi-clause conditions
//! (`x.nil? && y.nil?`), guard-clause early returns, and `case`/`when`
//! narrowing are not yet implemented — each can slot in here when a
//! case forces them.
//!
//! Called from the `If` arm in the body-typer's `compute` match.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

use super::Ctx;

/// A variable reference that narrowing can target: either a local
/// binding (`x`) or an instance variable (`@x`).
pub(super) enum VarKey {
    Local(Symbol),
    Ivar(Symbol),
}

/// A condition that narrows a variable's type in the branches of an
/// `if`. Only nil-shaped and class-shaped predicates are recognized —
/// more complex conditions fall through with no narrowing applied.
pub(super) enum NarrowPred {
    /// `x.nil?` or `x == nil` — true in then, false in else.
    IsNil(VarKey),
    /// `!x.nil?` or `x != nil` — false in then, true in else.
    IsNotNil(VarKey),
    /// `x.is_a?(T)` — narrow to T in then, remove T from union in else.
    IsA(VarKey, Ty),
    /// `!x.is_a?(T)` — inverse.
    IsNotA(VarKey, Ty),
}

pub(super) fn extract_narrowing(cond: &Expr) -> Option<NarrowPred> {
    match &*cond.node {
        // Ruby's `!` is a method call: `!x` parses as `x.!`. So
        // `!x.nil?` is Send(method="!", recv=Some(Send(method="nil?", recv=Var(x)))).
        ExprNode::Send { recv: Some(inner), method, args, .. }
            if method.as_str() == "!" && args.is_empty() =>
        {
            extract_narrowing(inner).map(negate_pred)
        }
        ExprNode::Send { recv: Some(target), method, args, .. } => {
            match (method.as_str(), args.as_slice()) {
                ("nil?", []) => var_key(target).map(NarrowPred::IsNil),
                ("==", [arg]) if is_nil_lit(arg) => {
                    var_key(target).map(NarrowPred::IsNil)
                }
                ("!=", [arg]) if is_nil_lit(arg) => {
                    var_key(target).map(NarrowPred::IsNotNil)
                }
                ("is_a?" | "kind_of?" | "instance_of?", [arg]) => {
                    let key = var_key(target)?;
                    let ty = const_to_ty(arg)?;
                    Some(NarrowPred::IsA(key, ty))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn negate_pred(p: NarrowPred) -> NarrowPred {
    match p {
        NarrowPred::IsNil(k) => NarrowPred::IsNotNil(k),
        NarrowPred::IsNotNil(k) => NarrowPred::IsNil(k),
        NarrowPred::IsA(k, t) => NarrowPred::IsNotA(k, t),
        NarrowPred::IsNotA(k, t) => NarrowPred::IsA(k, t),
    }
}

fn var_key(e: &Expr) -> Option<VarKey> {
    match &*e.node {
        ExprNode::Var { name, .. } => Some(VarKey::Local(name.clone())),
        ExprNode::Ivar { name } => Some(VarKey::Ivar(name.clone())),
        _ => None,
    }
}

fn is_nil_lit(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
}

/// A constant path used as a class argument to `is_a?` — map built-in
/// class names to their structural types, user classes to `Ty::Class`.
fn const_to_ty(e: &Expr) -> Option<Ty> {
    let ExprNode::Const { path } = &*e.node else {
        return None;
    };
    let name = path.last()?;
    Some(match name.as_str() {
        "Integer" | "Numeric" => Ty::Int,
        "Float" => Ty::Float,
        "String" => Ty::Str,
        "Symbol" => Ty::Sym,
        "NilClass" => Ty::Nil,
        "TrueClass" | "FalseClass" => Ty::Bool,
        other => Ty::Class {
            id: ClassId(Symbol::from(other)),
            args: vec![],
        },
    })
}

pub(super) fn apply_narrowing(ctx: &Ctx, pred: &NarrowPred, then_branch: bool) -> Ctx {
    let mut new_ctx = ctx.clone();
    match pred {
        NarrowPred::IsNil(k) | NarrowPred::IsNotNil(k) => {
            let is_is_nil = matches!(pred, NarrowPred::IsNil(_));
            let narrow_to_nil = is_is_nil == then_branch;
            narrow_binding(&mut new_ctx, k, |current| {
                if narrow_to_nil {
                    Ty::Nil
                } else {
                    remove_nil(current)
                }
            });
        }
        NarrowPred::IsA(k, ty) | NarrowPred::IsNotA(k, ty) => {
            let is_is_a = matches!(pred, NarrowPred::IsA(_, _));
            let narrow_to_ty = is_is_a == then_branch;
            narrow_binding(&mut new_ctx, k, |current| {
                if narrow_to_ty {
                    intersect_with(current, ty)
                } else {
                    remove_variant(current, ty)
                }
            });
        }
    }
    new_ctx
}

fn narrow_binding<F: FnOnce(&Ty) -> Ty>(ctx: &mut Ctx, key: &VarKey, f: F) {
    let (name, bindings) = match key {
        VarKey::Local(n) => (n, &mut ctx.local_bindings),
        VarKey::Ivar(n) => (n, &mut ctx.ivar_bindings),
    };
    if let Some(current) = bindings.get(name).cloned() {
        let narrowed = f(&current);
        bindings.insert(name.clone(), narrowed);
    }
}

fn remove_nil(ty: &Ty) -> Ty {
    match ty {
        Ty::Union { variants } => {
            let kept: Vec<Ty> = variants
                .iter()
                .filter(|v| !matches!(v, Ty::Nil))
                .cloned()
                .collect();
            match kept.len() {
                0 => Ty::Nil,
                1 => kept.into_iter().next().unwrap(),
                _ => Ty::Union { variants: kept },
            }
        }
        // Not a union — if the type is bare Nil, the "non-nil" branch
        // is unreachable in Ruby; we keep Nil here (the analyzer doesn't
        // flag contradictions). For non-Nil concrete types, no change.
        other => other.clone(),
    }
}

/// Given a current type and a narrower one, return the narrower form.
/// `String | Nil ∩ String = String`; `Post ∩ Post = Post`; anything
/// else returns the narrower type on the assumption the check would
/// have succeeded (matches Ruby's `is_a?` semantics at run time).
fn intersect_with(current: &Ty, narrower: &Ty) -> Ty {
    match current {
        Ty::Union { variants } => {
            // Keep only variants compatible with the narrower type.
            let kept: Vec<Ty> = variants
                .iter()
                .filter(|v| ty_compatible(v, narrower))
                .cloned()
                .collect();
            match kept.len() {
                0 => narrower.clone(),
                1 => kept.into_iter().next().unwrap(),
                _ => Ty::Union { variants: kept },
            }
        }
        _ => narrower.clone(),
    }
}

/// Remove variants matching `ty` from a union (for `is_a?` else-branch).
fn remove_variant(current: &Ty, ty: &Ty) -> Ty {
    match current {
        Ty::Union { variants } => {
            let kept: Vec<Ty> = variants
                .iter()
                .filter(|v| !ty_compatible(v, ty))
                .cloned()
                .collect();
            match kept.len() {
                0 => current.clone(),
                1 => kept.into_iter().next().unwrap(),
                _ => Ty::Union { variants: kept },
            }
        }
        _ => current.clone(),
    }
}

/// Structural equality on types — pre-subtyping approximation.
/// Used only by narrowing today; full subtype checks can replace it
/// when polymorphism lands.
fn ty_compatible(a: &Ty, b: &Ty) -> bool {
    a == b
}
