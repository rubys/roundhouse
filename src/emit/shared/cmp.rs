//! Shared classifier for comparison operators (`<`, `<=`, `>`, `>=`).
//!
//! All four operators share the same type semantics — Ruby's
//! `Comparable` module is what wires them up, and the result for any
//! operand pair is either "works natively in every target" (both
//! numeric with the same concrete type, both strings, both symbols),
//! "needs a promotion cast in statically-typed targets" (Int vs
//! Float), or "Ruby raises" (`1 < "hello"`, ordering on unrelated
//! classes, etc.).
//!
//! Callers consult [`classify_cmp`] on the two operands' `.ty`
//! annotations and switch on the returned [`CmpCase`]. Missing type
//! info falls back to [`CmpCase::Unknown`], which targets render as
//! native infix — always safe.

use crate::expr::Expr;
use crate::ty::Ty;

pub enum CmpCase {
    /// Both operands have the same type that every target's native
    /// comparison operator handles correctly (Int, Float, Str, Sym).
    /// Emit as native `a op b`.
    SameType,
    /// Int vs Float or Float vs Int — most targets coerce silently;
    /// Rust and Go need explicit casts on the Int side.
    NumericPromote,
    /// Known concrete types but `Comparable` doesn't cover the pair
    /// (`1 < "hello"`, Array comparison we don't handle yet, etc.).
    /// Ruby raises at runtime; callers annotate a diagnostic and
    /// emit a target-side raise.
    Incompatible,
    /// Type info missing on at least one side — fall back to native
    /// infix. Conservative and target-safe.
    Unknown,
}

pub fn classify_cmp(lhs: &Expr, rhs: &Expr) -> CmpCase {
    let lhs_ty = lhs.ty.as_ref();
    let rhs_ty = rhs.ty.as_ref();

    // Untyped is the gradual-escape hatch: caller used RBS `untyped`
    // (or the analyzer couldn't refine the type past it). Treat as
    // Unknown so the emit falls through to native infix — same
    // posture as missing type info. Otherwise the catch-all arm
    // below would mis-flag e.g. `untyped <= Integer` as Incompatible
    // and the runtime would throw on a perfectly valid comparison.
    let is_unknown = |t: Option<&Ty>| {
        matches!(
            t,
            None | Some(Ty::Var { .. }) | Some(Ty::Untyped),
        )
    };
    if is_unknown(lhs_ty) || is_unknown(rhs_ty) {
        return CmpCase::Unknown;
    }

    let lhs_ty = lhs_ty.unwrap();
    let rhs_ty = rhs_ty.unwrap();

    match (lhs_ty, rhs_ty) {
        (Ty::Int, Ty::Int) | (Ty::Float, Ty::Float) => CmpCase::SameType,
        (Ty::Str, Ty::Str) | (Ty::Sym, Ty::Sym) => CmpCase::SameType,
        (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int) => CmpCase::NumericPromote,
        _ => CmpCase::Incompatible,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{ExprNode, Literal};
    use crate::ident::{Symbol, VarId};
    use crate::span::Span;

    fn typed(node: ExprNode, ty: Ty) -> Expr {
        let mut e = Expr::new(Span::synthetic(), node);
        e.ty = Some(ty);
        e
    }

    fn var_typed(name: &str, ty: Ty) -> Expr {
        typed(
            ExprNode::Var {
                id: VarId(0),
                name: Symbol::from(name),
            },
            ty,
        )
    }

    fn int_lit(v: i64) -> Expr {
        typed(ExprNode::Lit { value: Literal::Int { value: v } }, Ty::Int)
    }

    fn untyped_var(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Var {
                id: VarId(0),
                name: Symbol::from(name),
            },
        )
    }

    #[test]
    fn int_vs_int_is_same_type() {
        assert!(matches!(classify_cmp(&int_lit(1), &int_lit(2)), CmpCase::SameType));
    }

    #[test]
    fn str_vs_str_is_same_type() {
        let l = var_typed("a", Ty::Str);
        let r = var_typed("b", Ty::Str);
        assert!(matches!(classify_cmp(&l, &r), CmpCase::SameType));
    }

    #[test]
    fn sym_vs_sym_is_same_type() {
        let l = var_typed("a", Ty::Sym);
        let r = var_typed("b", Ty::Sym);
        assert!(matches!(classify_cmp(&l, &r), CmpCase::SameType));
    }

    #[test]
    fn int_vs_float_is_numeric_promote() {
        let l = var_typed("a", Ty::Int);
        let r = var_typed("b", Ty::Float);
        assert!(matches!(classify_cmp(&l, &r), CmpCase::NumericPromote));
    }

    #[test]
    fn int_vs_str_is_incompatible() {
        let l = var_typed("a", Ty::Int);
        let r = var_typed("b", Ty::Str);
        assert!(matches!(classify_cmp(&l, &r), CmpCase::Incompatible));
    }

    #[test]
    fn array_vs_array_is_incompatible_for_now() {
        // Ruby DOES allow array comparison (element-wise), but target
        // emission varies too much to handle today (JS coerces to
        // string; Go doesn't support `<` on slices). Classify as
        // Incompatible so users get a loud diagnostic rather than
        // silently-wrong emission.
        let arr = Ty::Array { elem: Box::new(Ty::Int) };
        let l = var_typed("a", arr.clone());
        let r = var_typed("b", arr);
        assert!(matches!(classify_cmp(&l, &r), CmpCase::Incompatible));
    }

    #[test]
    fn unknown_lhs_is_unknown() {
        let l = untyped_var("a");
        let r = int_lit(1);
        assert!(matches!(classify_cmp(&l, &r), CmpCase::Unknown));
    }

    #[test]
    fn unknown_rhs_is_unknown() {
        let l = int_lit(1);
        let r = untyped_var("b");
        assert!(matches!(classify_cmp(&l, &r), CmpCase::Unknown));
    }

    #[test]
    fn bool_vs_bool_is_incompatible() {
        // Ruby's TrueClass/FalseClass aren't Comparable — `true < false`
        // raises NoMethodError. Classify accordingly.
        let l = var_typed("a", Ty::Bool);
        let r = var_typed("b", Ty::Bool);
        assert!(matches!(classify_cmp(&l, &r), CmpCase::Incompatible));
    }
}
