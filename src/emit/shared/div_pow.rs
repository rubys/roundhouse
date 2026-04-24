//! Shared classifier for `/` and `**` emission.
//!
//! Both operators are pure-numeric in Ruby: they work on Int/Float
//! and nothing else. The classifier enum is identical for both; each
//! emitter knows which operator it's emitting and picks the right
//! output form (`/` is native in every target; `**` needs `.pow()`,
//! `math.Pow`, or `:math.pow` in Rust/Go/Elixir respectively).
//!
//! Callers consult [`classify_div_pow`] on the two operands' `.ty`
//! annotations (populated by the body-typer). Missing type info
//! falls back to [`DivPowCase::Unknown`] — emitters then render as
//! native infix.

use crate::expr::Expr;
use crate::ty::Ty;

pub enum DivPowCase {
    /// Int/Int or Float/Float — emit natively per target.
    Numeric,
    /// Int/Float or Float/Int — Rust and Go need explicit casts;
    /// other targets auto-coerce.
    NumericPromote,
    /// Non-numeric operands — Ruby raises. Callers emit a target-
    /// language raise-equivalent.
    Incompatible,
    /// Type info missing on either side — fall back to native infix.
    Unknown,
}

pub fn classify_div_pow(lhs: &Expr, rhs: &Expr) -> DivPowCase {
    let lhs_ty = lhs.ty.as_ref();
    let rhs_ty = rhs.ty.as_ref();

    let is_unknown = |t: Option<&Ty>| matches!(t, None | Some(Ty::Var { .. }));
    if is_unknown(lhs_ty) || is_unknown(rhs_ty) {
        return DivPowCase::Unknown;
    }

    let lhs_ty = lhs_ty.unwrap();
    let rhs_ty = rhs_ty.unwrap();

    match (lhs_ty, rhs_ty) {
        (Ty::Int, Ty::Int) | (Ty::Float, Ty::Float) => DivPowCase::Numeric,
        (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int) => DivPowCase::NumericPromote,
        _ => DivPowCase::Incompatible,
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
    fn int_over_int_is_numeric() {
        assert!(matches!(
            classify_div_pow(&int_lit(10), &int_lit(2)),
            DivPowCase::Numeric
        ));
    }

    #[test]
    fn float_over_float_is_numeric() {
        let l = var_typed("a", Ty::Float);
        let r = var_typed("b", Ty::Float);
        assert!(matches!(classify_div_pow(&l, &r), DivPowCase::Numeric));
    }

    #[test]
    fn int_over_float_is_numeric_promote() {
        let l = var_typed("a", Ty::Int);
        let r = var_typed("b", Ty::Float);
        assert!(matches!(classify_div_pow(&l, &r), DivPowCase::NumericPromote));
    }

    #[test]
    fn float_over_int_is_numeric_promote() {
        let l = var_typed("a", Ty::Float);
        let r = var_typed("b", Ty::Int);
        assert!(matches!(classify_div_pow(&l, &r), DivPowCase::NumericPromote));
    }

    #[test]
    fn str_over_str_is_incompatible() {
        let l = var_typed("a", Ty::Str);
        let r = var_typed("b", Ty::Str);
        assert!(matches!(classify_div_pow(&l, &r), DivPowCase::Incompatible));
    }

    #[test]
    fn int_over_str_is_incompatible() {
        let l = var_typed("a", Ty::Int);
        let r = var_typed("b", Ty::Str);
        assert!(matches!(classify_div_pow(&l, &r), DivPowCase::Incompatible));
    }

    #[test]
    fn array_over_array_is_incompatible() {
        let arr = Ty::Array { elem: Box::new(Ty::Int) };
        let l = var_typed("a", arr.clone());
        let r = var_typed("b", arr);
        assert!(matches!(classify_div_pow(&l, &r), DivPowCase::Incompatible));
    }

    #[test]
    fn unknown_lhs_is_unknown() {
        let l = untyped_var("a");
        let r = int_lit(2);
        assert!(matches!(classify_div_pow(&l, &r), DivPowCase::Unknown));
    }

    #[test]
    fn unknown_rhs_is_unknown() {
        let l = int_lit(1);
        let r = untyped_var("b");
        assert!(matches!(classify_div_pow(&l, &r), DivPowCase::Unknown));
    }
}
