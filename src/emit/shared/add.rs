//! Shared classifier for `+` emission.
//!
//! Ruby's `+` is receiver-overloaded (Int/Float/String/Array each
//! defines it differently; most other types don't define it at all).
//! Targets split that single operator into multiple emission forms —
//! native `+` for the cases they support, per-target idioms for the
//! ones they don't, and a hard refusal for cases Ruby itself would
//! raise on (`1 + "hello"` → TypeError).
//!
//! Callers consult [`classify_add`] on the two operands' `.ty`
//! annotations (populated by the body-typer) and switch on the
//! returned [`AddCase`]. Without type information on both sides we
//! return [`AddCase::Unknown`] so emitters fall back to native
//! infix — always safe.

use crate::expr::Expr;
use crate::ty::Ty;

/// What kind of `+` this is, in enough detail for a target emitter
/// to pick the right output form.
pub enum AddCase<'a> {
    /// Int + Int or Float + Float — emit as native `+`.
    Numeric,
    /// Int + Float or Float + Int — most targets auto-coerce; Rust
    /// and Go need explicit casts on the Int side.
    NumericPromote,
    /// Str + Str — concatenation per target idiom (Elixir: `<>`,
    /// Rust: `format!`, others: native `+`).
    StringConcat,
    /// Array[T] + Array[T] where the element types match — concat
    /// per target idiom (Elixir: `++`, Go: `append(a, b...)`, Rust:
    /// `.concat()`, TS: spread, others: native `+`).
    ArrayConcat {
        /// The shared element type. Emitters that need to declare
        /// intermediate storage types (Rust, Go) read it here.
        elem: &'a Ty,
    },
    /// Both sides typed concretely but `+` isn't defined in Ruby
    /// for that pair (`Int + Str`, `Hash + Hash`, etc.). Ruby would
    /// raise at runtime; callers refuse at emit.
    Incompatible,
    /// Type info missing on either side — fall back to native infix.
    /// Matches the conservative policy: emit what we'd emit without
    /// this classifier.
    Unknown,
}

/// Classify a pair of operands for `+` emission.
pub fn classify_add<'a>(lhs: &'a Expr, rhs: &'a Expr) -> AddCase<'a> {
    let lhs_ty = lhs.ty.as_ref();
    let rhs_ty = rhs.ty.as_ref();

    let is_unknown = |t: Option<&Ty>| {
        matches!(t, None | Some(Ty::Var { .. }))
    };
    if is_unknown(lhs_ty) || is_unknown(rhs_ty) {
        return AddCase::Unknown;
    }

    let lhs_ty = lhs_ty.unwrap();
    let rhs_ty = rhs_ty.unwrap();

    match (lhs_ty, rhs_ty) {
        (Ty::Int, Ty::Int) | (Ty::Float, Ty::Float) => AddCase::Numeric,
        (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int) => AddCase::NumericPromote,
        (Ty::Str, Ty::Str) => AddCase::StringConcat,
        (Ty::Array { elem: l }, Ty::Array { elem: r }) if l == r => {
            AddCase::ArrayConcat { elem: l.as_ref() }
        }
        _ => AddCase::Incompatible,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{ExprNode, Literal};
    use crate::ident::{Symbol, VarId};
    use crate::span::Span;

    fn with_ty(node: ExprNode, ty: Ty) -> Expr {
        let mut e = Expr::new(Span::synthetic(), node);
        e.ty = Some(ty);
        e
    }

    fn var_with(name: &str, ty: Ty) -> Expr {
        with_ty(
            ExprNode::Var {
                id: VarId(0),
                name: Symbol::from(name),
            },
            ty,
        )
    }

    fn int_lit(value: i64) -> Expr {
        with_ty(ExprNode::Lit { value: Literal::Int { value } }, Ty::Int)
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
    fn int_plus_int_is_numeric() {
        assert!(matches!(
            classify_add(&int_lit(1), &int_lit(2)),
            AddCase::Numeric
        ));
    }

    #[test]
    fn float_plus_float_is_numeric() {
        let l = var_with("a", Ty::Float);
        let r = var_with("b", Ty::Float);
        assert!(matches!(classify_add(&l, &r), AddCase::Numeric));
    }

    #[test]
    fn int_plus_float_is_numeric_promote() {
        let l = var_with("a", Ty::Int);
        let r = var_with("b", Ty::Float);
        assert!(matches!(classify_add(&l, &r), AddCase::NumericPromote));
    }

    #[test]
    fn float_plus_int_is_numeric_promote() {
        let l = var_with("a", Ty::Float);
        let r = var_with("b", Ty::Int);
        assert!(matches!(classify_add(&l, &r), AddCase::NumericPromote));
    }

    #[test]
    fn str_plus_str_is_string_concat() {
        let l = var_with("a", Ty::Str);
        let r = var_with("b", Ty::Str);
        assert!(matches!(classify_add(&l, &r), AddCase::StringConcat));
    }

    #[test]
    fn array_plus_array_matching_elem_is_array_concat() {
        let a_ty = Ty::Array { elem: Box::new(Ty::Int) };
        let l = var_with("a", a_ty.clone());
        let r = var_with("b", a_ty);
        let case = classify_add(&l, &r);
        let AddCase::ArrayConcat { elem } = case else {
            panic!("expected ArrayConcat");
        };
        assert_eq!(*elem, Ty::Int);
    }

    #[test]
    fn array_plus_array_different_elem_is_incompatible() {
        let l = var_with(
            "a",
            Ty::Array { elem: Box::new(Ty::Int) },
        );
        let r = var_with(
            "b",
            Ty::Array { elem: Box::new(Ty::Str) },
        );
        // Mismatched element types — Ruby would allow this (producing
        // Array<Int|Str>) but our dispatch doesn't handle it yet, so
        // treat as Incompatible to keep emission honest.
        assert!(matches!(classify_add(&l, &r), AddCase::Incompatible));
    }

    #[test]
    fn int_plus_str_is_incompatible() {
        let l = var_with("a", Ty::Int);
        let r = var_with("b", Ty::Str);
        // Ruby raises TypeError. We refuse at emit.
        assert!(matches!(classify_add(&l, &r), AddCase::Incompatible));
    }

    #[test]
    fn hash_plus_hash_is_incompatible() {
        let h = Ty::Hash {
            key: Box::new(Ty::Sym),
            value: Box::new(Ty::Int),
        };
        let l = var_with("a", h.clone());
        let r = var_with("b", h);
        // Ruby raises NoMethodError (Hash doesn't define `+`). Refuse.
        assert!(matches!(classify_add(&l, &r), AddCase::Incompatible));
    }

    #[test]
    fn unknown_lhs_is_unknown() {
        let l = untyped_var("a");
        let r = int_lit(2);
        assert!(matches!(classify_add(&l, &r), AddCase::Unknown));
    }

    #[test]
    fn unknown_rhs_is_unknown() {
        let l = int_lit(1);
        let r = untyped_var("b");
        assert!(matches!(classify_add(&l, &r), AddCase::Unknown));
    }

    #[test]
    fn ty_var_counts_as_unknown() {
        let l = var_with(
            "a",
            Ty::Var {
                var: crate::ident::TyVar(0),
            },
        );
        let r = int_lit(2);
        assert!(matches!(classify_add(&l, &r), AddCase::Unknown));
    }
}
