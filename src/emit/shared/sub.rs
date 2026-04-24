//! Shared classifier for `-` emission.
//!
//! Ruby's `-` is receiver-overloaded but much narrower than `+`:
//! Numeric (Int/Float) subtract, Array - Array is set difference,
//! and everything else raises. No strings, no hashes.
//!
//! Callers consult [`classify_sub`] on the two operands' `.ty`
//! annotations (populated by the body-typer) and switch on the
//! returned [`SubCase`]. Missing type info falls back to
//! [`SubCase::Unknown`] — emitters then render as native infix,
//! which is always safe for targets whose own `-` matches Ruby's
//! numeric case.

use crate::expr::Expr;
use crate::ty::Ty;

/// What kind of `-` this is, in enough detail for a target emitter
/// to pick the right output form.
pub enum SubCase<'a> {
    /// Int - Int or Float - Float — emit as native `-`.
    Numeric,
    /// Int - Float or Float - Int — most targets auto-coerce; Rust
    /// and Go need explicit casts on the Int side.
    NumericPromote,
    /// Array[T] - Array[T] with matching element type — Ruby's set
    /// difference: elements of lhs that don't appear in rhs.
    /// No native equivalent in most targets; each renders a
    /// filter/contains combinator.
    ArrayDifference {
        /// Shared element type. Emitters that need to declare
        /// intermediate storage (Rust, Go) read it here.
        elem: &'a Ty,
    },
    /// Both sides typed concretely but `-` isn't defined in Ruby
    /// for that pair (`"a" - "b"`, `Hash - Hash`, `Array - Int`,
    /// …). Ruby raises at run time; callers emit a target-language
    /// raise-equivalent so the compiled program also raises.
    Incompatible,
    /// Type info missing on either side — fall back to native infix.
    Unknown,
}

pub fn classify_sub<'a>(lhs: &'a Expr, rhs: &'a Expr) -> SubCase<'a> {
    let lhs_ty = lhs.ty.as_ref();
    let rhs_ty = rhs.ty.as_ref();

    let is_unknown = |t: Option<&Ty>| matches!(t, None | Some(Ty::Var { .. }));
    if is_unknown(lhs_ty) || is_unknown(rhs_ty) {
        return SubCase::Unknown;
    }

    let lhs_ty = lhs_ty.unwrap();
    let rhs_ty = rhs_ty.unwrap();

    match (lhs_ty, rhs_ty) {
        (Ty::Int, Ty::Int) | (Ty::Float, Ty::Float) => SubCase::Numeric,
        (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int) => SubCase::NumericPromote,
        (Ty::Array { elem: l }, Ty::Array { elem: r }) if l == r => {
            SubCase::ArrayDifference { elem: l.as_ref() }
        }
        _ => SubCase::Incompatible,
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
            ExprNode::Var { id: VarId(0), name: Symbol::from(name) },
            ty,
        )
    }

    fn int_lit(v: i64) -> Expr {
        typed(ExprNode::Lit { value: Literal::Int { value: v } }, Ty::Int)
    }

    fn untyped_var(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: Symbol::from(name) },
        )
    }

    #[test]
    fn int_minus_int_is_numeric() {
        assert!(matches!(
            classify_sub(&int_lit(5), &int_lit(2)),
            SubCase::Numeric
        ));
    }

    #[test]
    fn float_minus_float_is_numeric() {
        let l = var_typed("a", Ty::Float);
        let r = var_typed("b", Ty::Float);
        assert!(matches!(classify_sub(&l, &r), SubCase::Numeric));
    }

    #[test]
    fn int_minus_float_is_numeric_promote() {
        let l = var_typed("a", Ty::Int);
        let r = var_typed("b", Ty::Float);
        assert!(matches!(classify_sub(&l, &r), SubCase::NumericPromote));
    }

    #[test]
    fn array_minus_array_matching_elem_is_array_difference() {
        let a_ty = Ty::Array { elem: Box::new(Ty::Int) };
        let l = var_typed("a", a_ty.clone());
        let r = var_typed("b", a_ty);
        let SubCase::ArrayDifference { elem } = classify_sub(&l, &r) else {
            panic!("expected ArrayDifference");
        };
        assert_eq!(*elem, Ty::Int);
    }

    #[test]
    fn array_minus_array_different_elem_is_incompatible() {
        let l = var_typed("a", Ty::Array { elem: Box::new(Ty::Int) });
        let r = var_typed("b", Ty::Array { elem: Box::new(Ty::Str) });
        assert!(matches!(classify_sub(&l, &r), SubCase::Incompatible));
    }

    #[test]
    fn str_minus_str_is_incompatible() {
        // Ruby's String doesn't define `-` — NoMethodError.
        let l = var_typed("a", Ty::Str);
        let r = var_typed("b", Ty::Str);
        assert!(matches!(classify_sub(&l, &r), SubCase::Incompatible));
    }

    #[test]
    fn hash_minus_hash_is_incompatible() {
        let h = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Int) };
        let l = var_typed("a", h.clone());
        let r = var_typed("b", h);
        assert!(matches!(classify_sub(&l, &r), SubCase::Incompatible));
    }

    #[test]
    fn int_minus_str_is_incompatible() {
        let l = var_typed("a", Ty::Int);
        let r = var_typed("b", Ty::Str);
        assert!(matches!(classify_sub(&l, &r), SubCase::Incompatible));
    }

    #[test]
    fn unknown_lhs_is_unknown() {
        let l = untyped_var("a");
        let r = int_lit(2);
        assert!(matches!(classify_sub(&l, &r), SubCase::Unknown));
    }

    #[test]
    fn unknown_rhs_is_unknown() {
        let l = int_lit(1);
        let r = untyped_var("b");
        assert!(matches!(classify_sub(&l, &r), SubCase::Unknown));
    }
}
