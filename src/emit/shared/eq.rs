//! Shared classifier for `==` / `!=` emission.
//!
//! Given two operands, returns an [`EqCase`] describing what kind of
//! equality this is — the same shape across every target, even though
//! each target emits it differently. The classifier consults
//! [`Expr::ty`] (populated by the body-typer); with no type info it
//! falls back to the `SameType` case and emits native infix, which
//! is always safe.
//!
//! Scope today: `SameType` (native infix) and `NilCheck` (one side
//! has `Ty::Nil`). Mixed-numeric promotion and cross-type literal
//! `false` emission can join when a runtime function surfaces them.

use crate::expr::Expr;
use crate::ty::Ty;

/// What kind of `==` / `!=` this is, in enough detail for a target
/// emitter to pick the right output form.
pub enum EqCase<'a> {
    /// Standard case — emit as native infix (`a == b` or `a === b`).
    /// Covers both "types match" and "types unknown" (fall back to
    /// whatever the target language would normally do).
    SameType,
    /// One side is exactly `Ty::Nil`; the other side (the "subject")
    /// is the thing being checked. The enclosing expression needs a
    /// nil-idiom call instead of a bare equality.
    NilCheck { subject: &'a Expr },
}

/// Classify a pair of operands for `==` / `!=` emission.
///
/// Returns `NilCheck { subject }` when exactly one side has `Ty::Nil`;
/// otherwise `SameType`. Equal-but-both-Nil falls into `SameType`
/// (the literal comparison `nil == nil` is true and `a == b` renders
/// it correctly in every target).
pub fn classify_eq<'a>(lhs: &'a Expr, rhs: &'a Expr) -> EqCase<'a> {
    let lhs_nil = matches!(lhs.ty.as_ref(), Some(Ty::Nil));
    let rhs_nil = matches!(rhs.ty.as_ref(), Some(Ty::Nil));
    match (lhs_nil, rhs_nil) {
        (true, true) => EqCase::SameType,
        (true, false) => EqCase::NilCheck { subject: rhs },
        (false, true) => EqCase::NilCheck { subject: lhs },
        (false, false) => EqCase::SameType,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{ExprNode, Literal};
    use crate::ident::Symbol;
    use crate::span::Span;

    fn int_var(name: &str) -> Expr {
        let mut e = Expr::new(
            Span::synthetic(),
            ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from(name),
            },
        );
        e.ty = Some(Ty::Int);
        e
    }

    fn str_var(name: &str) -> Expr {
        let mut e = Expr::new(
            Span::synthetic(),
            ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from(name),
            },
        );
        e.ty = Some(Ty::Str);
        e
    }

    fn nil_lit() -> Expr {
        let mut e = Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Nil },
        );
        e.ty = Some(Ty::Nil);
        e
    }

    fn unknown_var(name: &str) -> Expr {
        // No `.ty` set — simulates an expr the analyzer couldn't
        // resolve (falls back to "unknown" behavior).
        Expr::new(
            Span::synthetic(),
            ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from(name),
            },
        )
    }

    #[test]
    fn same_type_ints() {
        assert!(matches!(
            classify_eq(&int_var("a"), &int_var("b")),
            EqCase::SameType
        ));
    }

    #[test]
    fn same_type_strings() {
        assert!(matches!(
            classify_eq(&str_var("a"), &str_var("b")),
            EqCase::SameType
        ));
    }

    #[test]
    fn rhs_nil_is_nil_check_with_lhs_subject() {
        let lhs = str_var("x");
        let rhs = nil_lit();
        match classify_eq(&lhs, &rhs) {
            EqCase::NilCheck { subject } => {
                assert!(matches!(
                    &*subject.node,
                    ExprNode::Var { name, .. } if name.as_str() == "x"
                ));
            }
            _ => panic!("expected NilCheck"),
        }
    }

    #[test]
    fn lhs_nil_is_nil_check_with_rhs_subject() {
        let lhs = nil_lit();
        let rhs = str_var("x");
        match classify_eq(&lhs, &rhs) {
            EqCase::NilCheck { subject } => {
                assert!(matches!(
                    &*subject.node,
                    ExprNode::Var { name, .. } if name.as_str() == "x"
                ));
            }
            _ => panic!("expected NilCheck"),
        }
    }

    #[test]
    fn nil_equals_nil_is_same_type() {
        assert!(matches!(
            classify_eq(&nil_lit(), &nil_lit()),
            EqCase::SameType
        ));
    }

    #[test]
    fn missing_type_falls_back_to_same_type() {
        assert!(matches!(
            classify_eq(&unknown_var("a"), &unknown_var("b")),
            EqCase::SameType
        ));
    }

    #[test]
    fn one_typed_one_untyped_falls_back_to_same_type() {
        // Without type info on both sides we can't call it nil-check;
        // infix is always safe in that case.
        assert!(matches!(
            classify_eq(&int_var("a"), &unknown_var("b")),
            EqCase::SameType
        ));
    }
}
