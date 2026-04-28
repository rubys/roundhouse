//! Crystal type rendering — `Ty` → Crystal type expression, plus
//! defaults and fixture-literal coercion.

use crate::ty::Ty;

// Types ----------------------------------------------------------------

pub fn crystal_ty(ty: &Ty) -> String {
    match ty {
        // Crystal's default integer is Int32; Rails schemas typically
        // use BigInt for IDs, so Int64 is the safer default for the
        // scaffold.
        Ty::Int => "Int64".to_string(),
        Ty::Float => "Float64".to_string(),
        Ty::Bool => "Bool".to_string(),
        Ty::Str => "String".to_string(),
        // Crystal has native Symbol.
        Ty::Sym => "Symbol".to_string(),
        Ty::Nil => "Nil".to_string(),
        Ty::Array { elem } => format!("Array({})", crystal_ty(elem)),
        Ty::Hash { key, value } => format!("Hash({}, {})", crystal_ty(key), crystal_ty(value)),
        Ty::Tuple { elems } => {
            let parts: Vec<String> = elems.iter().map(crystal_ty).collect();
            format!("Tuple({})", parts.join(", "))
        }
        Ty::Record { .. } => "Hash(String, String)".to_string(),
        Ty::Union { variants } => {
            // Crystal union: `A | B | C`.
            let parts: Vec<String> = variants.iter().map(crystal_ty).collect();
            parts.join(" | ")
        }
        Ty::Class { id, .. } => id.0.to_string(),
        Ty::Fn { .. } => "Proc(Nil)".to_string(),
        Ty::Var { .. } => "_".to_string(),
        // RBS-declared `untyped`. Crystal's gradual escape is the
        // wildcard `_`; same rendering as Var keeps the distinction
        // in the IR but invisible at emission. Future refinement:
        // emit-time elevation to error for strict-target pipelines.
        Ty::Untyped => "_".to_string(),
        // Bottom type — Crystal's native `NoReturn`. The IR's
        // Bottom maps directly to Crystal's NoReturnType (which is
        // exactly the type-theoretic concept this variant models).
        Ty::Bottom => "NoReturn".to_string(),
    }
}

/// A Crystal literal expression for the given type's zero value.
/// Crystal's `property` declarations must be initialized — unlike
/// Rust's `#[derive(Default)]` we have to write the value inline.
/// Keep aligned with `crystal_ty`: whatever a field renders as, its
/// default must be a valid expression of that type.
pub(super) fn crystal_default(ty: &Ty) -> String {
    match ty {
        Ty::Int => "0_i64".to_string(),
        Ty::Float => "0.0_f64".to_string(),
        Ty::Bool => "false".to_string(),
        Ty::Str | Ty::Sym => "\"\"".to_string(),
        Ty::Nil => "nil".to_string(),
        Ty::Array { .. } => "[] of typeof({})".to_string(),
        Ty::Hash { .. } => "{} of String => String".to_string(),
        // Class types we emit ourselves get a .new; stdlib Time gets
        // Time.utc; anything else falls back to .new and trusts the
        // class defines a zero-arg initializer.
        Ty::Class { id, .. } => match id.0.as_str() {
            "Time" => "Time.utc".to_string(),
            other => format!("{other}.new"),
        },
        _ => "nil".to_string(),
    }
}

pub(super) fn crystal_literal_for(value: &str, ty: &Ty) -> String {
    match ty {
        Ty::Str | Ty::Sym => format!("{value:?}"),
        Ty::Int => {
            if value.parse::<i64>().is_ok() {
                format!("{value}_i64")
            } else {
                format!("0_i64 # TODO: coerce {value:?}")
            }
        }
        Ty::Float => {
            if value.parse::<f64>().is_ok() {
                format!("{value}_f64")
            } else {
                format!("0.0_f64 # TODO: coerce {value:?}")
            }
        }
        Ty::Bool => match value {
            "true" | "1" => "true".into(),
            "false" | "0" => "false".into(),
            _ => format!("false # TODO: coerce {value:?}"),
        },
        Ty::Class { id, .. } if id.0.as_str() == "Time" => format!("{value:?}"),
        _ => format!("{value:?}"),
    }
}
