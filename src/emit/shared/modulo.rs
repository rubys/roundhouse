//! Shared classifier for `%` emission.
//!
//! Ruby's `%` has two meaningful shapes: numeric modulo (Int/Float)
//! and string formatting (`"%d %s" % [5, "foo"]`). Target support
//! varies — Python and Crystal inherit Ruby's `%` semantics
//! natively; Rust/Go/TS have only numeric `%` and need `format!` /
//! `fmt.Sprintf` / a helper for the string-format case.
//!
//! Callers consult [`classify_modulo`] on the two operands' `.ty`
//! annotations (populated by the body-typer). Missing type info
//! falls back to [`ModuloCase::Unknown`] — emitters then render as
//! native infix.
//!
//! NOTE: `%` on a numeric pair in Ruby follows the sign of the
//! divisor (`-7 % 3 = 2`); C/Rust/Go/JS `%` follows the sign of the
//! dividend (`-7 % 3 = -1`). Tolerated for now; rework when a
//! fixture forces the distinction.

use crate::expr::Expr;
use crate::ty::Ty;

pub enum ModuloCase {
    /// Int % Int or Float % Float — emit as native `%`.
    Numeric,
    /// Int % Float or Float % Int — most targets auto-coerce; Rust
    /// and Go need explicit casts on the Int side.
    NumericPromote,
    /// Str % args — Ruby's `String#%` does printf-style formatting.
    /// Target support is split: Python and Crystal inherit the
    /// semantics natively, everything else needs a helper or
    /// `format!` / `fmt.Sprintf` call.
    StringFormat,
    /// Non-numeric, non-string-format operands — Ruby raises. Callers
    /// emit a target-language raise-equivalent.
    Incompatible,
    /// Type info missing on either side — fall back to native infix.
    Unknown,
}

pub fn classify_modulo(lhs: &Expr, rhs: &Expr) -> ModuloCase {
    let lhs_ty = lhs.ty.as_ref();
    let rhs_ty = rhs.ty.as_ref();

    let is_unknown = |t: Option<&Ty>| matches!(t, None | Some(Ty::Var { .. }));
    if is_unknown(lhs_ty) || is_unknown(rhs_ty) {
        return ModuloCase::Unknown;
    }

    let lhs_ty = lhs_ty.unwrap();
    let rhs_ty = rhs_ty.unwrap();

    match (lhs_ty, rhs_ty) {
        (Ty::Int, Ty::Int) | (Ty::Float, Ty::Float) => ModuloCase::Numeric,
        (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int) => ModuloCase::NumericPromote,
        // Str % anything typed (Array of args, Hash, single value) is
        // string formatting in Ruby. Reject Str % Str as that's not
        // meaningful (but Ruby allows it — `"%" % "foo"` is a no-op).
        // For now, Str with any rhs that isn't obvious-Incompatible
        // gets StringFormat.
        (Ty::Str, _) => ModuloCase::StringFormat,
        _ => ModuloCase::Incompatible,
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
    fn int_mod_int_is_numeric() {
        assert!(matches!(
            classify_modulo(&int_lit(7), &int_lit(3)),
            ModuloCase::Numeric
        ));
    }

    #[test]
    fn float_mod_float_is_numeric() {
        let l = var_typed("a", Ty::Float);
        let r = var_typed("b", Ty::Float);
        assert!(matches!(classify_modulo(&l, &r), ModuloCase::Numeric));
    }

    #[test]
    fn int_mod_float_is_numeric_promote() {
        let l = var_typed("a", Ty::Int);
        let r = var_typed("b", Ty::Float);
        assert!(matches!(classify_modulo(&l, &r), ModuloCase::NumericPromote));
    }

    #[test]
    fn str_mod_array_is_string_format() {
        let l = var_typed("a", Ty::Str);
        let r = var_typed("b", Ty::Array { elem: Box::new(Ty::Int) });
        assert!(matches!(classify_modulo(&l, &r), ModuloCase::StringFormat));
    }

    #[test]
    fn str_mod_int_is_string_format() {
        // Ruby: `"%d" % 5` is valid — single-arg format.
        let l = var_typed("a", Ty::Str);
        let r = var_typed("b", Ty::Int);
        assert!(matches!(classify_modulo(&l, &r), ModuloCase::StringFormat));
    }

    #[test]
    fn array_mod_array_is_incompatible() {
        let arr = Ty::Array { elem: Box::new(Ty::Int) };
        let l = var_typed("a", arr.clone());
        let r = var_typed("b", arr);
        assert!(matches!(classify_modulo(&l, &r), ModuloCase::Incompatible));
    }

    #[test]
    fn int_mod_str_is_incompatible() {
        let l = var_typed("a", Ty::Int);
        let r = var_typed("b", Ty::Str);
        assert!(matches!(classify_modulo(&l, &r), ModuloCase::Incompatible));
    }

    #[test]
    fn hash_mod_hash_is_incompatible() {
        let h = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Int) };
        let l = var_typed("a", h.clone());
        let r = var_typed("b", h);
        assert!(matches!(classify_modulo(&l, &r), ModuloCase::Incompatible));
    }

    #[test]
    fn unknown_lhs_is_unknown() {
        let l = untyped_var("a");
        let r = int_lit(2);
        assert!(matches!(classify_modulo(&l, &r), ModuloCase::Unknown));
    }

    #[test]
    fn unknown_rhs_is_unknown() {
        let l = int_lit(1);
        let r = untyped_var("b");
        assert!(matches!(classify_modulo(&l, &r), ModuloCase::Unknown));
    }
}
