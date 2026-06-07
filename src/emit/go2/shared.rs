//! Cross-cutting helpers used by multiple go2 emit modules.
//!
//! Naming conversions live here because they're called from every
//! per-output-kind emitter (library, expr, test, …). `emit_literal`
//! is the literal renderer shared by the test emitters (`spec`,
//! `controller_test`).

use crate::expr::Literal;

pub(super) fn pascalize_word(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

pub(super) fn go_field_name(ruby_name: &str) -> String {
    // Convert snake_case to PascalCase, special-casing common initialisms.
    ruby_name
        .split('_')
        .map(|part| match part {
            "id" => "ID".to_string(),
            "url" => "URL".to_string(),
            "http" => "HTTP".to_string(),
            "json" => "JSON".to_string(),
            "html" => "HTML".to_string(),
            "api" => "API".to_string(),
            other => pascalize_word(other),
        })
        .collect::<String>()
}

pub(super) fn go_method_name(ruby_name: &str) -> String {
    ruby_name
        .split('_')
        .map(|part| match part {
            "id" => "ID".to_string(),
            other => pascalize_word(other),
        })
        .collect::<String>()
}

/// Render a Ruby literal as a Go literal. Used by the test emitters
/// (`spec`, `controller_test`) which build assertion bodies from
/// parsed test source.
pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "nil".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        Literal::Sym { value } => format!("{:?}", value.as_str()),
        Literal::Regex { pattern, flags } => {
            format!("regexp.MustCompile({:?})", format!("(?{flags}){pattern}"))
        }
    }
}
