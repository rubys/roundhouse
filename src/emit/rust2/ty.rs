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
        // Rust has chrono/time, but the datetime seam isn't wired yet
        // (Stage 2) — a Time surface is an honest not-supported gap.
        Ty::Time => crate::emit::diagnostics::unsupported_time_ty("rust"),
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
            // Multi-variant non-Nilable unions (lowerer-synthesized
            // `set_index`/`get_index` value/return Tys are a
            // column-Ty union: `Union<i64, String, …>`) render as
            // `serde_json::Value`. The original `Box<dyn Any>`
            // required consumers to `.downcast::<T>()`, which doesn't
            // match the Ruby/Crystal `value.as(T)` shape the
            // lowerer's `Cast` emits — and
            // `coerce_arg_for_field_ty` already bridges Value →
            // primitive via `.as_X().unwrap()`.
            "serde_json::Value".to_string()
        }),
        Ty::Class { id, .. } => {
            let name = id.0.as_str();
            // Time → String for now; Rust's `chrono::DateTime<Utc>`
            // is the real target but the framework Ruby surface
            // serializes Times as ISO-8601 strings everywhere, so
            // String matches behavior without forcing the chrono
            // import on every consumer. Refine once the per-target
            // primitive runtime lands.
            if name == "Time" {
                return "String".to_string();
            }
            // Strip the namespace prefix — Rust uses file-as-module,
            // so `ActiveSupport::HashWithIndifferentAccess` ought to
            // render as the bare type name (the namespace becomes
            // the import path, not part of the identifier). Matches
            // the Const-emit decision in `expr.rs`.
            name.rsplit("::").next().unwrap_or(name).to_string()
        }
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
