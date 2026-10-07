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

use crate::expr::{BoolOpKind, Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

use super::Ctx;

/// A variable reference that narrowing can target: either a local
/// binding (`x`) or an instance variable (`@x`).
#[derive(Clone)]
pub(super) enum VarKey {
    Local(Symbol),
    Ivar(Symbol),
    /// A receiver-less, zero-argument read of an attribute of `self`
    /// (`expires_at.present? && expires_at > now` in a model: a column or
    /// `attr_accessor` reader). It is not a binding, so the type the
    /// condition saw rides along, and a narrowing of it binds the name as
    /// a local for the guarded region (bare `x` resolves local-first).
    Reader(Symbol, Ty),
}

/// A condition that narrows a variable's type in the branches of an
/// `if`. Only nil-shaped and class-shaped predicates are recognized —
/// more complex conditions fall through with no narrowing applied.
#[derive(Clone)]
pub(super) enum NarrowPred {
    /// A compound condition. `if_true` holds when the whole condition is
    /// true, `if_false` when it is false; every entry is a leaf predicate
    /// applied as if TRUE. `a && b`: both hold when true, nothing certain
    /// when false (either may have failed). `a || b`: both are false when
    /// false, nothing certain when true. Negation swaps the two lists.
    Compound { if_true: Vec<NarrowPred>, if_false: Vec<NarrowPred> },
    /// `x.nil?` or `x == nil` — true in then, false in else.
    IsNil(VarKey),
    /// `!x.nil?` or `x != nil` — false in then, true in else.
    IsNotNil(VarKey),
    /// `x.is_a?(T)` — narrow to T in then, remove T from union in else.
    IsA(VarKey, Ty),
    /// `!x.is_a?(T)` — inverse.
    IsNotA(VarKey, Ty),
    /// Bare variable as truthiness guard (`if x` / `x && x.foo`).
    /// True branch removes Nil from `x`'s union; false branch leaves
    /// only Nil. Ruby's truthiness also excludes `false`, but Bool
    /// isn't split between true/false in our type system, so we don't
    /// narrow Bool. Functionally equivalent to `IsNotNil` for the then
    /// side but recognized from a different surface shape.
    IsTruthy(VarKey),
    /// Negated truthiness (`if !x` / `unless x`).
    IsFalsy(VarKey),
    /// `x.present?` — Rails' `!blank?`. A present value is never nil, so the
    /// then side drops the nil arm; the else side is only "nil, false or
    /// empty", which says nothing certain about the type.
    IsPresent(VarKey),
    /// `x.blank?` / `!x.present?` — the inverse: the else side is non-nil.
    IsBlank(VarKey),
}

#[cfg(test)]
pub(super) fn extract_narrowing(cond: &Expr) -> Option<NarrowPred> {
    extract_narrowing_with(cond, &|_| None)
}

/// What a condition guarantees on each side. `is_reader` says whether a bare
/// name is an attribute reader of `self` (see [`VarKey::Reader`]).
pub(super) fn extract_narrowing_with(
    cond: &Expr,
    is_reader: &dyn Fn(&Symbol) -> Option<Ty>,
) -> Option<NarrowPred> {
    let extract_narrowing = |e: &Expr| extract_narrowing_with(e, is_reader);
    let var_key = |e: &Expr| var_key(e, is_reader);
    match &*cond.node {
        // Ruby's `!` is a method call: `!x` parses as `x.!`. So
        // `!x.nil?` is Send(method="!", recv=Some(Send(method="nil?", recv=Var(x)))).
        ExprNode::Send { recv: Some(inner), method, args, .. }
            if method.as_str() == "!" && args.is_empty() =>
        {
            extract_narrowing(inner).map(negate_pred)
        }
        // Lowerer-synthesized negation — the view predicate rewrite
        // (`src/lower/view_to_library/predicates.rs`) builds `!x` as
        // `Send { recv: None, method: "!", args: [operand] }` rather
        // than the parser's `operand.!` shape. Recognize that variant
        // too so a lowered `!notice.nil?` participates in narrowing.
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "!" && args.len() == 1 =>
        {
            extract_narrowing(&args[0]).map(negate_pred)
        }
        // `max_qty?` -- ActiveRecord's attribute query, true only when the
        // attribute is present (set, non-blank, non-zero), so it says what
        // `max_qty.present?` says.
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if args.is_empty() && method.as_str().len() > 1 && method.as_str().ends_with('?') =>
        {
            let attr = Symbol::from(&method.as_str()[..method.as_str().len() - 1]);
            is_reader(&attr).map(|ty| NarrowPred::IsPresent(VarKey::Reader(attr, ty)))
        }
        ExprNode::Send { recv: Some(target), method, args, .. } => {
            match (method.as_str(), args.as_slice()) {
                ("nil?", []) => var_key(target).map(NarrowPred::IsNil),
                ("present?", []) => var_key(target).map(NarrowPred::IsPresent),
                ("blank?", []) => var_key(target).map(NarrowPred::IsBlank),
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
        // `A && B` is true iff both are true, so a predicate in either
        // conjunct holds in the then-branch; when it is false either may
        // have failed, so the else-branch learns nothing. `A || B` is the
        // mirror image: false only if both are false. Predicates on
        // different variables compose (`a.present? && b.present? && a < b`).
        ExprNode::BoolOp { op, left, right, .. }
            if matches!(op, BoolOpKind::And | BoolOpKind::Or) =>
        {
            let sides = [extract_narrowing(left), extract_narrowing(right)];
            let mut on_true: Vec<NarrowPred> = Vec::new();
            let mut on_false: Vec<NarrowPred> = Vec::new();
            for p in sides.into_iter().flatten() {
                let (t, f) = p.sides();
                if matches!(op, BoolOpKind::And) {
                    on_true.extend(t);
                } else {
                    on_false.extend(f);
                }
            }
            if on_true.is_empty() && on_false.is_empty() {
                None
            } else {
                Some(NarrowPred::Compound { if_true: on_true, if_false: on_false })
            }
        }
        // Assignment as the condition: `if user = User.authenticate_by(…)`
        // (the Rails authentication generator's idiom) tests the
        // assigned value's truthiness, so the then-branch reads the
        // variable without its nil arm. The binding itself is seeded
        // by the caller from the assignment (see the `If` and `BoolOp`
        // arms' `collect_var_assignments_into`); this only names what
        // to narrow. Parenthesized `(x = …)` parses the same.
        ExprNode::Assign { target: LValue::Var { name, .. }, .. } => {
            Some(NarrowPred::IsTruthy(VarKey::Local(name.clone())))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, .. } => {
            Some(NarrowPred::IsTruthy(VarKey::Ivar(name.clone())))
        }
        // Bare variable / ivar / bareword as truthiness guard. Tried
        // last because the explicit predicate matches above are more
        // specific (`x.nil?` is also a Send-on-Var but with a method
        // name we recognize). `if @user` / `@user && @user.foo` /
        // `if notice` all flow through this fallback.
        _ => var_key(cond).map(NarrowPred::IsTruthy),
    }
}

impl NarrowPred {
    /// The leaf predicates that hold when this one is true, and when false.
    fn sides(self) -> (Vec<NarrowPred>, Vec<NarrowPred>) {
        match self {
            NarrowPred::Compound { if_true, if_false } => (if_true, if_false),
            leaf => (vec![leaf.clone()], vec![negate_pred(leaf)]),
        }
    }
}

fn negate_pred(p: NarrowPred) -> NarrowPred {
    match p {
        NarrowPred::Compound { if_true, if_false } => {
            NarrowPred::Compound { if_true: if_false, if_false: if_true }
        }
        NarrowPred::IsNil(k) => NarrowPred::IsNotNil(k),
        NarrowPred::IsNotNil(k) => NarrowPred::IsNil(k),
        NarrowPred::IsA(k, t) => NarrowPred::IsNotA(k, t),
        NarrowPred::IsNotA(k, t) => NarrowPred::IsA(k, t),
        NarrowPred::IsTruthy(k) => NarrowPred::IsFalsy(k),
        NarrowPred::IsFalsy(k) => NarrowPred::IsTruthy(k),
        NarrowPred::IsPresent(k) => NarrowPred::IsBlank(k),
        NarrowPred::IsBlank(k) => NarrowPred::IsPresent(k),
    }
}

fn var_key(e: &Expr, is_reader: &dyn Fn(&Symbol) -> Option<Ty>) -> Option<VarKey> {
    match &*e.node {
        ExprNode::Var { name, .. } => Some(VarKey::Local(name.clone())),
        ExprNode::Ivar { name } => Some(VarKey::Ivar(name.clone())),
        // Bareword implicit-self read — view-partial lowering emits
        // param references as `Send { recv: None, method: <name>,
        // args: [], block: None }`, the same shape `compute` (body/
        // mod.rs:406) resolves against `ctx.local_bindings`. Treat
        // them as variable-references for narrowing too: without this,
        // `if !notice.nil? && !notice.empty?` (lowered from
        // `if notice.present?`) doesn't tighten `notice: Option<String>`
        // to `String` in the body, leaving subsequent reads typed as
        // Option and the emitter producing `&(notice)` against an
        // `html_escape(&str)` site. `narrow_binding` no-ops when the
        // name isn't a binding, so this is safe for non-param barewords.
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if args.is_empty() =>
        {
            match is_reader(method) {
                Some(ty) => Some(VarKey::Reader(method.clone(), ty)),
                None => Some(VarKey::Local(method.clone())),
            }
        }
        _ => None,
    }
}

fn is_nil_lit(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
}

/// A constant path used as a class argument to `is_a?` — map built-in
/// class names to their structural types, user classes to `Ty::Class`.
///
/// `core` is path-shape only (`Array` / `::Array`). Callers must type the
/// condition first so Const analysis has already run `qualify_resolved_path`;
/// otherwise a lexically shadowed `Array` still looks bare and becomes a
/// structural container. Synthetic unit-test Const paths stay as written.
fn const_to_ty(e: &Expr) -> Option<Ty> {
    let ExprNode::Const { path } = &*e.node else {
        return None;
    };
    let name = path.last()?;
    let core = path.len() == 1 || (path.len() == 2 && path[0].as_str().is_empty());
    Some(match name.as_str() {
        "Integer" => Ty::Int,
        // `Numeric` covers Int and Float in Ruby's hierarchy. Union
        // both so subsequent dispatch resolves either via int_method
        // or as Float (universal methods cover the overlap).
        "Numeric" => Ty::Union { variants: vec![Ty::Int, Ty::Float] },
        "Float" => Ty::Float,
        "String" => Ty::Str,
        "Symbol" => Ty::Sym,
        "NilClass" => Ty::Nil,
        "TrueClass" | "FalseClass" => Ty::Bool,
        // `Array` and `Hash` must narrow to their parameterized IR
        // forms — `Ty::Class { id: Array }` falls through dispatch
        // (which goes through array_method only on `Ty::Array { .. }`).
        // Element type at narrowing time is `Ty::Untyped` (gradual
        // escape), not `Ty::Var` (inference gap): `is_a?(Hash)` only
        // tells us "this is a Hash of *some* shape," and downstream
        // dispatch should propagate that gradualness rather than
        // leave block params as Var.
        "Array" if core => Ty::Array { elem: Box::new(Ty::Untyped) },
        "Hash" if core => Ty::Hash {
            key: Box::new(Ty::Untyped),
            value: Box::new(Ty::Untyped),
        },
        _ => Ty::Class {
            id: ClassId(Symbol::from(
                path.iter()
                    .filter(|part| !part.as_str().is_empty())
                    .map(Symbol::as_str)
                    .collect::<Vec<_>>()
                    .join("::"),
            )),
            args: vec![],
        },
    })
}

pub(super) fn apply_narrowing(ctx: &Ctx, pred: &NarrowPred, then_branch: bool) -> Ctx {
    let mut new_ctx = ctx.clone();
    match pred {
        NarrowPred::Compound { if_true, if_false } => {
            for p in if then_branch { if_true } else { if_false } {
                new_ctx = apply_narrowing(&new_ctx, p, true);
            }
        }
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
        NarrowPred::IsPresent(k) | NarrowPred::IsBlank(k) => {
            let is_present = matches!(pred, NarrowPred::IsPresent(_));
            if is_present == then_branch {
                narrow_binding(&mut new_ctx, k, |current| remove_nil(current));
            }
        }
        NarrowPred::IsTruthy(k) | NarrowPred::IsFalsy(k) => {
            // Only narrow the truthy side. The falsy side would
            // normally narrow to `Ty::Nil`, but that turns subsequent
            // `@user.foo` in the else-branch into Nil-receiver
            // dispatch failures — surfacing pre-existing Rails-code
            // bugs as roundhouse errors. The truthy side is the
            // dominant pattern (`if @user; @user.foo; end`); the
            // falsy side rarely reads the variable at all.
            let is_is_truthy = matches!(pred, NarrowPred::IsTruthy(_));
            let narrow_to_truthy = is_is_truthy == then_branch;
            if narrow_to_truthy {
                narrow_binding(&mut new_ctx, k, |current| remove_nil(current));
            }
        }
    }
    new_ctx
}

fn narrow_binding<F: FnOnce(&Ty) -> Ty>(ctx: &mut Ctx, key: &VarKey, f: F) {
    let (name, bindings) = match key {
        VarKey::Local(n) => (n, &mut ctx.local_bindings),
        VarKey::Ivar(n) => (n, &mut ctx.ivar_bindings),
        VarKey::Reader(n, ty) => {
            // A real local of that name shadows the reader and narrows as
            // usual; otherwise the reader's own type is what narrows. Only
            // the "it is there" direction is recorded: a reader proved nil
            // is not worth rebinding, and re-reading it may say otherwise.
            if !ctx.local_bindings.contains_key(n) {
                let narrowed = f(ty);
                if narrowed != Ty::Nil && &narrowed != ty {
                    ctx.local_bindings.insert(n.clone(), narrowed);
                }
                return;
            }
            (n, &mut ctx.local_bindings)
        }
    };
    if let Some(current) = bindings.get(name).cloned() {
        let narrowed = f(&current);
        bindings.insert(name.clone(), narrowed);
    }
}

pub(crate) fn remove_nil(ty: &Ty) -> Ty {
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

/// Given a current type and a narrower one, return the intersection.
/// `String | Nil ∩ String = String`; matching Array/Hash spines keep
/// `current` (via [`ty_compatible`]) so known members survive; when no
/// variant is compatible, fall back to `narrower` on the assumption the
/// check would have succeeded (matches Ruby's `is_a?` at run time).
fn intersect_with(current: &Ty, narrower: &Ty) -> Ty {
    match current {
        Ty::Union { variants } => {
            // Keep only variants compatible with the narrower type.
            let kept: Vec<Ty> = variants
                .iter()
                .filter_map(|variant| intersect_variant(variant, narrower))
                .collect();
            if kept.is_empty() {
                narrower.clone()
            } else {
                super::union_many(kept)
            }
        }
        _ => intersect_variant(current, narrower).unwrap_or_else(|| narrower.clone()),
    }
}

/// Preserve known members; only ParamValue supplies a recursive contract.
/// Array/Hash spine matches go through [`ty_compatible`] (same owner as
/// else-branch [`remove_variant`]) rather than a duplicated match arm.
fn intersect_variant(current: &Ty, narrower: &Ty) -> Option<Ty> {
    match (current, narrower) {
        (Ty::Class { id, .. }, Ty::Array { .. }) if id.0.as_str() == super::send::PARAM_VALUE => {
            Some(Ty::Array { elem: Box::new(super::send::param_value_ty()) })
        }
        (Ty::Class { id, .. }, Ty::Hash { .. }) if id.0.as_str() == super::send::PARAM_VALUE => {
            Some(Ty::Hash { key: Box::new(Ty::Str), value: Box::new(super::send::param_value_ty()) })
        }
        (Ty::Untyped | Ty::Var { .. }, Ty::Array { .. } | Ty::Hash { .. }) => {
            Some(narrower.clone())
        }
        _ if ty_compatible(current, narrower) => Some(current.clone()),
        _ => None,
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

/// Equality, plus Array/Hash spine matches ignoring member types.
/// Owns the container-spine predicate for both [`intersect_variant`]
/// (then-branch: keep `current`) and [`remove_variant`] (else-branch:
/// drop every Array/Hash). Pre-subtyping approximation; full subtype
/// checks can replace it when polymorphism lands.
fn ty_compatible(a: &Ty, b: &Ty) -> bool {
    a == b
        || matches!(
            (a, b),
            (Ty::Array { .. }, Ty::Array { .. }) | (Ty::Hash { .. }, Ty::Hash { .. })
        )
}
