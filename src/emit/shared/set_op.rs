//! Shared classifier for Array `&` and `|`.
//!
//! On an `Array[T]` receiver Ruby's `&` is set intersection and `|` is
//! set union, both order-preserving (receiver order first) and
//! duplicate-free. No target's native `&`/`|` means that: TypeScript's
//! is bitwise (two arrays coerce to `0`), Python's is undefined for
//! lists, and Rust has no operator on `Vec`. Emitters consult
//! [`classify_set_op`] and render a combinator for the array cases
//! (including mismatched element types — same rule as Array `-`);
//! anything else (Integer/Bool bit operations, gradual operands) keeps
//! the native infix. TypeScript currently dedupes via `Set` (identity
//! for object/array elements); Python/Rust use equality — nested-array
//! fidelity on TS is a follow-up, not a scalar regression.

use crate::expr::Expr;
use crate::ty::Ty;

pub enum SetOpCase<'a> {
    /// `Array & Array` — elements of lhs also in rhs, first
    /// occurrence kept. Element types need not match (Ruby compares
    /// by `eql?`); `elem` is the lhs representative, same convention
    /// as [`crate::emit::shared::sub::SubCase::ArrayDifference`].
    ArrayIntersect { elem: &'a Ty },
    /// `Array | Array` — lhs then rhs, first occurrence kept.
    ArrayUnion { elem: &'a Ty },
    /// Not an array set operation — native infix (Integer/Bool
    /// bitwise, gradual / untyped operands). Named `Unknown` to
    /// match the sibling classifiers' vocabulary.
    Unknown,
}

pub fn classify_set_op<'a>(method: &str, lhs: &'a Expr, rhs: &'a Expr) -> SetOpCase<'a> {
    match (lhs.ty.as_ref(), rhs.ty.as_ref()) {
        // Mirror `classify_sub`: any Array/Array pair is a set op;
        // do not require equal element types (that would fall through
        // to native `&`/`|` and recreate the TypeScript/Python bugs
        // this classifier exists to avoid).
        (Some(Ty::Array { elem: l }), Some(Ty::Array { .. })) => match method {
            "&" => SetOpCase::ArrayIntersect { elem: l.as_ref() },
            "|" => SetOpCase::ArrayUnion { elem: l.as_ref() },
            _ => SetOpCase::Unknown,
        },
        _ => SetOpCase::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::ExprNode;
    use crate::ident::{Symbol, VarId};
    use crate::span::Span;

    fn var_typed(name: &str, ty: Ty) -> Expr {
        let mut e = Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: Symbol::from(name) },
        );
        e.ty = Some(ty);
        e
    }

    fn int_array(name: &str) -> Expr {
        var_typed(name, Ty::Array { elem: std::sync::Arc::new(Ty::Int) })
    }

    #[test]
    fn array_and_array_is_intersect() {
        let (l, r) = (int_array("a"), int_array("b"));
        assert!(matches!(classify_set_op("&", &l, &r), SetOpCase::ArrayIntersect { elem: Ty::Int }));
    }

    #[test]
    fn array_or_array_is_union() {
        let (l, r) = (int_array("a"), int_array("b"));
        assert!(matches!(classify_set_op("|", &l, &r), SetOpCase::ArrayUnion { elem: Ty::Int }));
    }

    #[test]
    fn int_bitwise_is_unknown() {
        let (l, r) = (var_typed("a", Ty::Int), var_typed("b", Ty::Int));
        assert!(matches!(classify_set_op("&", &l, &r), SetOpCase::Unknown));
        assert!(matches!(classify_set_op("|", &l, &r), SetOpCase::Unknown));
    }

    #[test]
    fn array_and_array_different_elem_is_intersect() {
        let l = int_array("a");
        let r = var_typed("b", Ty::Array { elem: std::sync::Arc::new(Ty::Str) });
        assert!(matches!(
            classify_set_op("&", &l, &r),
            SetOpCase::ArrayIntersect { elem: Ty::Int }
        ));
    }

    #[test]
    fn array_or_array_different_elem_is_union() {
        let l = int_array("a");
        let r = var_typed("b", Ty::Array { elem: std::sync::Arc::new(Ty::Str) });
        assert!(matches!(
            classify_set_op("|", &l, &r),
            SetOpCase::ArrayUnion { elem: Ty::Int }
        ));
    }
}
