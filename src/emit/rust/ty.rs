//! `rust` type rendering — `Ty` → Rust source-text.
//!
//! Phase 2 port of `src/emit/rust/ty.rs`. Identical surface; lives
//! here so the legacy emitter and the rust emitter can diverge
//! independently as the migration progresses (e.g. rust may add
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
        // Rust has a native datetime via chrono (already a direct dep).
        // Temporal (Date/DateTime/Time) columns store ISO-8601 text
        // (String ivar) and read back as a real `chrono::DateTime<Utc>`
        // via an explicit parsing getter (`crate::rh_datetime::
        // parse_db_time`); `Union{Time, Nil}` renders `Option<...>`.
        Ty::Time => "chrono::DateTime<chrono::Utc>".to_string(),
        Ty::Date => crate::emit::diagnostics::unsupported_date_ty("rust"),
        Ty::Nil => "()".to_string(),
        // A self type the analyzer should have substituted with
        // the receiving class (see `Ty::SelfInstance`). Reaching
        // here is a defect: report, never guess a class.
        Ty::SelfInstance => {
            return crate::emit::diagnostics::unsupported_self_instance_ty("rust");
        }
        // Analysis-time relation type — erased by query specialization
        // before emit (see `Ty::Relation`). Reaching here is a
        // coverage gap: report, never degrade to `Vec<T>`.
        Ty::Relation { of } => {
            return crate::emit::diagnostics::unsupported_relation_ty("rust", of);
        }
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
        Ty::Union { .. } if ty.is_stringish() => "String".to_string(),
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
            if name == "Regexp" {
                return "regex::Regex".to_string();
            }
            if name == "StringIO" {
                return "crate::string_io::StringIO".to_string();
            }
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
        // (the gradual-typing escape hatch). rust commits this to
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

/// True when this IR type is emitted as `serde_json::Value` (or the
/// `Roundhouse::ParamValue` alias of it). Heterogeneous unions that
/// are neither stringish nor a 2-variant `T | nil` Option land here,
/// matching `rust_ty`. Callers that need Value APIs (`is_null`,
/// `Value::from`, `as_str`) key off this rather than listing
/// Untyped/Var by hand — RBS `String | Integer | Float | bool | nil`
/// and `String | Array[untyped]` are Unions, not Untyped.
pub(crate) fn rust_value_shaped(ty: &Ty) -> bool {
    match ty {
        Ty::Untyped | Ty::Var { .. } | Ty::Record { .. } => true,
        Ty::Union { variants } => !ty.is_stringish() && option_shape(variants).is_none(),
        Ty::Class { id, .. } => id.0.as_str() == "Roundhouse::ParamValue",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heterogeneous_nilable_union_is_value_shaped() {
        let ty = Ty::Union {
            variants: vec![Ty::Str, Ty::Int, Ty::Float, Ty::Bool, Ty::Nil],
        };
        assert!(rust_value_shaped(&ty));
        assert_eq!(rust_ty(&ty), "serde_json::Value");
    }

    #[test]
    fn string_or_array_union_is_value_shaped() {
        let ty = Ty::Union {
            variants: vec![Ty::Str, Ty::Array { elem: Box::new(Ty::Untyped) }],
        };
        assert!(rust_value_shaped(&ty));
        assert_eq!(rust_ty(&ty), "serde_json::Value");
    }

    #[test]
    fn string_symbol_union_is_not_value_shaped() {
        let ty = Ty::Union {
            variants: vec![Ty::Str, Ty::Sym],
        };
        assert!(!rust_value_shaped(&ty));
        assert!(ty.is_stringish());
    }

    #[test]
    fn option_string_is_not_value_shaped() {
        let ty = Ty::Union {
            variants: vec![Ty::Str, Ty::Nil],
        };
        assert!(!rust_value_shaped(&ty));
        assert!(rust_ty(&ty).starts_with("Option<"));
    }

    #[test]
    fn regexp_uses_the_rust_regex_type() {
        let ty = Ty::Class {
            id: crate::ident::ClassId(crate::ident::Symbol::from("Regexp")),
            args: Vec::new(),
        };

        assert_eq!(rust_ty(&ty), "regex::Regex");
    }
}
