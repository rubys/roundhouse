//! Shared operand-type test for the binary-operator classifiers
//! (`add`, `sub`, `mul`, `div_pow`, `modulo`, `cmp`).
//!
//! Each classifier switches on the two operands' `.ty` annotations and,
//! for a concretely-typed pair Ruby has no operator for, returns
//! `Incompatible` — which both refuses emission (a target-side raise)
//! and surfaces an `incompatible_binop` diagnostic. That refusal is only
//! correct when we actually *know* both operand types. When an operand
//! is gradual/unknown we must fall through to native infix instead:
//! flagging it would mis-report valid code (`untyped <= Integer`,
//! `untyped + 1`) as an error and emit a runtime raise on a perfectly
//! good operation.
//!
//! "Gradual/unknown" is four cases:
//!   * `None` — the body-typer left no annotation.
//!   * `Ty::Var` — an unresolved inference variable.
//!   * `Ty::Untyped` — the explicit gradual escape hatch (RBS `untyped`,
//!     or a type the analyzer couldn't refine past it).
//!   * `Ty::Union` with any gradual arm — one gradual variant means we
//!     can't prove the whole value is incompatible (e.g.
//!     `(Float | untyped) * Float`). Checked recursively so nested
//!     unions collapse the same way.

use crate::ty::Ty;

/// Whether an operand type should suppress binop classification — i.e.
/// the classifier should treat it as `Unknown` and let the emitter fall
/// back to native infix rather than returning `Incompatible`. See the
/// module docs for the four gradual/unknown cases.
pub fn is_gradual_operand(t: Option<&Ty>) -> bool {
    match t {
        None | Some(Ty::Var { .. }) | Some(Ty::Untyped) => true,
        // Not `Incompatible`: a request-params value is whatever the request carried, so its operators are the native ones.
        Some(Ty::Class { id, .. }) if id.0.as_str() == crate::analyze::PARAM_VALUE => true,
        Some(Ty::Union { variants }) => {
            variants.iter().any(|v| is_gradual_operand(Some(v)))
        }
        _ => false,
    }
}

/// Admission comes from the operator's defining method, never the class label.
pub fn is_user_operator_receiver(receiver: &crate::expr::Expr) -> bool {
    receiver.decisions & crate::expr::RESOLVED_OPERATOR_RECEIVER != 0
}

/// An ordered number: `Int`, `Float`, `Numeric`/`BigDecimal`/`Rational`, or
/// a union whose every arm is one (`Float | Integer`, `Numeric | Integer`).
/// Ruby's arithmetic and comparison are defined between any two of them, so
/// for these the classifiers treat the pair like `Int` and `Float`.
pub fn is_number(ty: &Ty) -> bool {
    match ty {
        Ty::Int | Ty::Float => true,
        Ty::Class { id, .. } => matches!(id.0.as_str(), "Numeric" | "BigDecimal" | "Rational"),
        Ty::Union { variants } => !variants.is_empty() && variants.iter().all(is_number),
        _ => false,
    }
}

/// Only the lhs is checked: `Array - Set` raises `TypeError` in Ruby.
pub fn is_set_receiver(t: Option<&Ty>) -> bool {
    matches!(t, Some(Ty::Class { id, .. }) if id.0.as_str() == "Set")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_var_and_untyped_are_gradual() {
        assert!(is_gradual_operand(None));
        assert!(is_gradual_operand(Some(&Ty::Var {
            var: crate::ident::TyVar(0),
        })));
        assert!(is_gradual_operand(Some(&Ty::Untyped)));
    }

    #[test]
    fn concrete_types_are_not_gradual() {
        assert!(!is_gradual_operand(Some(&Ty::Int)));
        assert!(!is_gradual_operand(Some(&Ty::Str)));
    }

    #[test]
    fn union_is_gradual_iff_an_arm_is() {
        // A union of fully-concrete arms is NOT gradual — Ruby may still
        // raise on it (e.g. `Int | Nil`), so the classifier keeps
        // checking.
        let concrete = Ty::Union {
            variants: vec![Ty::Int, Ty::Nil].into(),
        };
        assert!(!is_gradual_operand(Some(&concrete)));

        // One gradual arm makes the whole union gradual.
        let with_untyped = Ty::Union {
            variants: vec![Ty::Float, Ty::Untyped].into(),
        };
        assert!(is_gradual_operand(Some(&with_untyped)));

        // Nesting collapses recursively.
        let nested = Ty::Union {
            variants: vec![
                Ty::Union {
                    variants: vec![Ty::Untyped, Ty::Untyped].into(),
                },
                Ty::Int,
            ].into(),
        };
        assert!(is_gradual_operand(Some(&nested)));
    }
}
