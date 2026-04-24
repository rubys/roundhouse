//! Shared classifier for `*` emission.
//!
//! Ruby's `*` is the most heavily overloaded arithmetic operator:
//! numeric multiplication, string repetition (`"a" * 3`), array
//! repetition (`[1, 2] * 3`), and array join (`[1, 2] * ","` returns
//! the string `"1,2"`). Each target needs a different emission form
//! for each case — some natively support the operator, some need a
//! method call, some need a multi-line IIFE.
//!
//! Callers consult [`classify_mul`] on the two operands' `.ty`
//! annotations (populated by the body-typer) and switch on the
//! returned [`MulCase`]. Missing type info falls back to
//! [`MulCase::Unknown`] — emitters then render as native infix.

use crate::expr::Expr;
use crate::ty::Ty;

/// What kind of `*` this is, in enough detail for a target emitter
/// to pick the right output form.
pub enum MulCase<'a> {
    /// Int * Int or Float * Float — emit as native `*`.
    Numeric,
    /// Int * Float or Float * Int — most targets auto-coerce; Rust
    /// and Go need explicit casts on the Int side.
    NumericPromote,
    /// Str * Int — Ruby's string repetition. Emitters use the
    /// per-target repeat idiom (`.repeat(n)`, `strings.Repeat`, etc.).
    StringRepeat,
    /// Array[T] * Int — Ruby's array repetition. Emitters use the
    /// per-target repeat idiom, which generally preserves `Array[T]`.
    ArrayRepeat {
        /// Shared element type. Emitters that need to declare
        /// intermediate storage (Rust, Go) read it here.
        elem: &'a Ty,
    },
    /// Array[T] * Str — Ruby's `Array#*` with a string argument is
    /// a join (returns String, not Array). Targets emit the per-
    /// target join idiom; when `elem` isn't already a string, the
    /// emission converts each element via the target's to-string.
    ArrayJoin {
        elem: &'a Ty,
    },
    /// Both sides typed concretely but `*` isn't defined in Ruby for
    /// that pair. Ruby raises at run time; callers emit a target-
    /// language raise-equivalent.
    Incompatible,
    /// Type info missing on either side — fall back to native infix.
    Unknown,
}

pub fn classify_mul<'a>(lhs: &'a Expr, rhs: &'a Expr) -> MulCase<'a> {
    let lhs_ty = lhs.ty.as_ref();
    let rhs_ty = rhs.ty.as_ref();

    let is_unknown = |t: Option<&Ty>| matches!(t, None | Some(Ty::Var { .. }));
    if is_unknown(lhs_ty) || is_unknown(rhs_ty) {
        return MulCase::Unknown;
    }

    let lhs_ty = lhs_ty.unwrap();
    let rhs_ty = rhs_ty.unwrap();

    match (lhs_ty, rhs_ty) {
        (Ty::Int, Ty::Int) | (Ty::Float, Ty::Float) => MulCase::Numeric,
        (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int) => MulCase::NumericPromote,
        (Ty::Str, Ty::Int) => MulCase::StringRepeat,
        (Ty::Array { elem }, Ty::Int) => MulCase::ArrayRepeat { elem: elem.as_ref() },
        (Ty::Array { elem }, Ty::Str) => MulCase::ArrayJoin { elem: elem.as_ref() },
        _ => MulCase::Incompatible,
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
    fn int_times_int_is_numeric() {
        assert!(matches!(classify_mul(&int_lit(2), &int_lit(3)), MulCase::Numeric));
    }

    #[test]
    fn float_times_float_is_numeric() {
        let l = var_typed("a", Ty::Float);
        let r = var_typed("b", Ty::Float);
        assert!(matches!(classify_mul(&l, &r), MulCase::Numeric));
    }

    #[test]
    fn int_times_float_is_numeric_promote() {
        let l = var_typed("a", Ty::Int);
        let r = var_typed("b", Ty::Float);
        assert!(matches!(classify_mul(&l, &r), MulCase::NumericPromote));
    }

    #[test]
    fn str_times_int_is_string_repeat() {
        let l = var_typed("a", Ty::Str);
        let r = var_typed("b", Ty::Int);
        assert!(matches!(classify_mul(&l, &r), MulCase::StringRepeat));
    }

    #[test]
    fn array_times_int_is_array_repeat() {
        let l = var_typed("a", Ty::Array { elem: Box::new(Ty::Int) });
        let r = var_typed("b", Ty::Int);
        let MulCase::ArrayRepeat { elem } = classify_mul(&l, &r) else {
            panic!("expected ArrayRepeat");
        };
        assert_eq!(*elem, Ty::Int);
    }

    #[test]
    fn array_times_str_is_array_join() {
        let l = var_typed("a", Ty::Array { elem: Box::new(Ty::Str) });
        let r = var_typed("b", Ty::Str);
        let MulCase::ArrayJoin { elem } = classify_mul(&l, &r) else {
            panic!("expected ArrayJoin");
        };
        assert_eq!(*elem, Ty::Str);
    }

    #[test]
    fn int_times_str_is_incompatible() {
        // Ruby: Int * Str → TypeError. The symmetric case (Str * Int)
        // is StringRepeat, but Int * Str is not defined.
        let l = var_typed("a", Ty::Int);
        let r = var_typed("b", Ty::Str);
        assert!(matches!(classify_mul(&l, &r), MulCase::Incompatible));
    }

    #[test]
    fn hash_times_hash_is_incompatible() {
        let h = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Int) };
        let l = var_typed("a", h.clone());
        let r = var_typed("b", h);
        assert!(matches!(classify_mul(&l, &r), MulCase::Incompatible));
    }

    #[test]
    fn bool_times_bool_is_incompatible() {
        let l = var_typed("a", Ty::Bool);
        let r = var_typed("b", Ty::Bool);
        assert!(matches!(classify_mul(&l, &r), MulCase::Incompatible));
    }

    #[test]
    fn unknown_lhs_is_unknown() {
        let l = untyped_var("a");
        let r = int_lit(2);
        assert!(matches!(classify_mul(&l, &r), MulCase::Unknown));
    }

    #[test]
    fn unknown_rhs_is_unknown() {
        let l = int_lit(1);
        let r = untyped_var("b");
        assert!(matches!(classify_mul(&l, &r), MulCase::Unknown));
    }
}
