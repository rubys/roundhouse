//! `rust2` type rendering — `Ty` → Rust source-text.
//!
//! Phase 2 port of `src/emit/rust/ty.rs`. Identical surface; lives
//! here so the legacy emitter and the rust2 emitter can diverge
//! independently as the migration progresses (e.g. rust2 may add
//! `&str` vs `String` distinctions for borrowed parameters that
//! the legacy emitter can't introduce without breaking shipping
//! real-blog).

use crate::ty::Ty;

pub fn rust_ty(ty: &Ty) -> String {
    match ty {
        Ty::Int => "i64".to_string(),
        Ty::Float => "f64".to_string(),
        Ty::Bool => "bool".to_string(),
        Ty::Str => "String".to_string(),
        Ty::Sym => "String".to_string(),
        Ty::Nil => "()".to_string(),
        Ty::Array { elem } => format!("Vec<{}>", rust_ty(elem)),
        Ty::Hash { key, value } => format!(
            "std::collections::HashMap<{}, {}>",
            rust_ty(key),
            rust_ty(value)
        ),
        Ty::Tuple { elems } => {
            let parts: Vec<String> = elems.iter().map(rust_ty).collect();
            format!("({})", parts.join(", "))
        }
        Ty::Record { .. } => "serde_json::Value".to_string(),
        Ty::Union { variants } => option_shape(variants).unwrap_or_else(|| {
            "Box<dyn std::any::Any>".to_string()
        }),
        Ty::Class { id, .. } => match id.0.as_str() {
            "Time" => "String".to_string(),
            other => other.to_string(),
        },
        Ty::Fn { .. } => "Box<dyn Fn()>".to_string(),
        Ty::Var { .. } => "serde_json::Value".to_string(),
        // RBS `untyped` is pervasive in HWIA / Parameters / view_helpers
        // (the gradual-typing escape hatch). rust2 commits this to
        // `serde_json::Value` — heterogeneous, serializable, already a
        // dep, has nested object/array support that matches Ruby's
        // recursive normalize_value semantics. Crystal commits to
        // String fallback; TS to `any`. Rust gets the structured-but-
        // dynamic option.
        Ty::Untyped => "serde_json::Value".to_string(),
        Ty::Bottom => "!".to_string(),
    }
}

fn option_shape(variants: &[Ty]) -> Option<String> {
    if variants.len() != 2 {
        return None;
    }
    match (&variants[0], &variants[1]) {
        (Ty::Nil, other) | (other, Ty::Nil) => Some(format!("Option<{}>", rust_ty(other))),
        _ => None,
    }
}
