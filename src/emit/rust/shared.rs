//! Cross-cutting helpers used by multiple Rust emit modules.

use crate::expr::Literal;

pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "None".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => format!("{value}_i64"),
        Literal::Float { value } => format!("{value}_f64"),
        Literal::Str { value } => format!("{value:?}.to_string()"),
        // Ruby symbols map to `String` in our Rust shape (see
        // `rust_ty` for `Ty::Sym`). Emit with the `.to_string()` coercion
        // so Hash entries mixing `"x"` (strings) and `:y` (symbols) stay
        // a uniform `HashMap<&str, String>`.
        Literal::Sym { value } => format!("{:?}.to_string()", value.as_str()),
    }
}
