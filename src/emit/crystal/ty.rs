//! `Ty` → Crystal type-string. Crystal's type system has direct
//! analogs for most Roundhouse `Ty` variants; the ones that don't map
//! cleanly (Untyped, Record, Var) fall back to a target-appropriate
//! permissive form.
//!
//! Convention: `Int` → `Int64` (Rails IDs are 64-bit on sqlite/MySQL),
//! `Float` → `Float64`, `Sym` → `Symbol`, `Str` → `String`, `Bool` →
//! `Bool`, `Nil` → `Nil`. Generics use parens: `Array(T)`, `Hash(K, V)`.
//! Unions render as `A | B`; an `A | Nil` union prefers the `A?`
//! shorthand.

use crate::ty::Ty;

pub fn crystal_ty(t: &Ty) -> String {
    match t {
        Ty::Int => "Int64".to_string(),
        Ty::Float => "Float64".to_string(),
        Ty::Bool => "Bool".to_string(),
        Ty::Str => "String".to_string(),
        Ty::Sym => "Symbol".to_string(),
        Ty::Nil => "Nil".to_string(),
        Ty::Bottom => "NoReturn".to_string(),
        Ty::Array { elem } => format!("Array({})", crystal_ty(elem)),
        Ty::Hash { key, value } => format!("Hash({}, {})", crystal_ty(key), crystal_ty(value)),
        Ty::Tuple { elems } => {
            let parts: Vec<String> = elems.iter().map(crystal_ty).collect();
            format!("Tuple({})", parts.join(", "))
        }
        Ty::Union { variants } => render_union(variants),
        Ty::Class { id, args } => {
            // Class names from the runtime/ingest pipeline arrive bare
            // (last-segment only) — `parse_library_with_rbs`'s
            // `class_name_path` doesn't carry the enclosing module path
            // through to ClassId. For framework classes that live in a
            // module (`ActiveSupport::HashWithIndifferentAccess`,
            // `ActionController::Parameters`, etc.), bare references at
            // a use site ('@hash : HashWithIndifferentAccess') don't
            // resolve in Crystal's namespace lookup. Re-attach the
            // module prefix here for the known framework classes.
            let name = id.0.as_str();
            let qualified = qualify_framework_class(name);
            if args.is_empty() {
                qualified
            } else {
                let parts: Vec<String> = args.iter().map(crystal_ty).collect();
                format!("{qualified}({})", parts.join(", "))
            }
        }
        // `Untyped`, `Record`, `Var`, `Fn` don't have idiomatic Crystal
        // type-position renderings. `Untyped` falls back to the
        // permissive `JSON::Any`-ish stand-in `String` for now —
        // method-level Untyped annotations get stripped at the
        // `emit_method` boundary, so reaching this branch usually
        // indicates a deeper type that should have been resolved.
        Ty::Untyped | Ty::Record { .. } | Ty::Var { .. } | Ty::Fn { .. } => "String".to_string(),
    }
}

/// Render a union. Special-cases `T | Nil` as `T?` (Crystal's nilable
/// shorthand) when the union has exactly two variants and one is Nil.
/// Larger unions render as `A | B | C`.
fn render_union(variants: &[Ty]) -> String {
    if variants.len() == 2 {
        let (nil_idx, non_nil_idx) = match (
            variants.iter().position(|t| matches!(t, Ty::Nil)),
            variants.iter().position(|t| !matches!(t, Ty::Nil)),
        ) {
            (Some(n), Some(nn)) => (n, nn),
            _ => return variants.iter().map(crystal_ty).collect::<Vec<_>>().join(" | "),
        };
        let _ = nil_idx;
        return format!("{}?", crystal_ty(&variants[non_nil_idx]));
    }
    variants.iter().map(crystal_ty).collect::<Vec<_>>().join(" | ")
}

/// Re-attach the enclosing-module prefix for transpiled framework
/// classes that are referenced from OUTSIDE their home module — e.g.
/// `ActionController::Parameters` referencing
/// `ActiveSupport::HashWithIndifferentAccess`. Bare references to the
/// HWIA name don't resolve from within the ActionController namespace.
///
/// Conservative scope: only the cross-module cases land here. Classes
/// like `ActiveRecord::Base`, `RecordInvalid`, etc. are referenced
/// almost exclusively from WITHIN their home module (the active_record
/// runtime files are all in `ActiveRecord`); leaving them bare lets
/// Crystal's namespace-nesting lookup resolve them and avoids parse-
/// time forward-reference issues with `property record : Base?` style
/// declarations inside reopened modules.
fn qualify_framework_class(name: &str) -> String {
    match name {
        "HashWithIndifferentAccess" => "ActiveSupport::HashWithIndifferentAccess".to_string(),
        _ => name.to_string(),
    }
}

/// True when the type is `Untyped` (or a union containing Untyped).
/// Used to decide whether a method signature should be emitted with
/// type annotations or left bare for Crystal inference.
pub fn has_untyped(t: &Ty) -> bool {
    match t {
        Ty::Untyped => true,
        Ty::Array { elem } => has_untyped(elem),
        Ty::Hash { key, value } => has_untyped(key) || has_untyped(value),
        Ty::Tuple { elems } => elems.iter().any(has_untyped),
        Ty::Union { variants } => variants.iter().any(has_untyped),
        Ty::Class { args, .. } => args.iter().any(has_untyped),
        Ty::Fn { params, ret, .. } => {
            params.iter().any(|p| has_untyped(&p.ty)) || has_untyped(ret)
        }
        _ => false,
    }
}
