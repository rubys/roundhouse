//! `Ty` → Kotlin type-string.
//!
//! Kotlin is a *soft* strict target (see `docs/kotlin-migration-plan.md`):
//! unlike Rust/Go — which elevate any reachable `Ty::Untyped` to an
//! emit-time error — Kotlin maps `Untyped`/`Var` to `Any?`, so the
//! gradual-typing escape hatch survives emission. Modeled on
//! `src/emit/crystal/ty.rs` (the closest strict analog) but softened
//! toward `Any?` like the TypeScript renderer.
//!
//! Convention: `Int` → `Long` (Rails IDs are 64-bit on sqlite),
//! `Float` → `Double`, `Sym` → `String` (Kotlin has no symbol), generics
//! use angle brackets (`MutableList<T>`, `MutableMap<K, V>`), and a
//! `T | Nil` union prefers the nullable shorthand `T?`.
#![allow(dead_code)]

use crate::ty::Ty;

pub fn kotlin_ty(t: &Ty) -> String {
    match t {
        Ty::Int => "Long".to_string(),
        Ty::Float => "Double".to_string(),
        Ty::Bool => "Boolean".to_string(),
        Ty::Str => "String".to_string(),
        // No symbol type in Kotlin — route symbols to string keys, as
        // the TS/Crystal renderers do.
        Ty::Sym => "String".to_string(),
        // Bare `Nil` defaults to the return-slot rendering `Unit`; a
        // value-slot `Nothing?` is reached via unions (`T | Nil → T?`).
        // A `kotlin_return_ty` helper will refine the outermost slot in
        // Phase 2.
        Ty::Nil => "Unit".to_string(),
        // Divergence type — `raise`/`return`. Kotlin's `Nothing` (≤ every
        // type) is the direct analog of Rust `!` / Crystal `NoReturn`.
        Ty::Bottom => "Nothing".to_string(),

        // AR result sets and view accumulators mutate, so default the
        // collection types to the mutable variants. A `mutates_self`-
        // driven tightening to read-only `List`/`Map` is a Phase 2+
        // refinement.
        Ty::Array { elem } => format!("MutableList<{}>", kotlin_ty(elem)),
        Ty::Hash { key, value } => {
            format!("MutableMap<{}, {}>", kotlin_ty(key), kotlin_ty(value))
        }

        // Kotlin lacks N-tuples beyond Pair/Triple. 2/3 map directly;
        // wider tuples need a generated `data class` (Phase 2) — fall
        // back to `List<Any?>` for now.
        Ty::Tuple { elems } => match elems.as_slice() {
            [a, b] => format!("Pair<{}, {}>", kotlin_ty(a), kotlin_ty(b)),
            [a, b, c] => format!("Triple<{}, {}, {}>", kotlin_ty(a), kotlin_ty(b), kotlin_ty(c)),
            _ => "List<Any?>".to_string(),
        },

        Ty::Union { variants } => render_union(variants),

        Ty::Class { id, args } => render_class(id.0.as_str(), args),

        // RBS record literal (e.g. an importmap pin `{name:, path:}`).
        // Kotlin has no anonymous record type — render it as a string-keyed
        // map, which is what the lowerer emits for these (`mutableMapOf(...)`)
        // and lets field reads work via `record["name"]`. (Genuinely typed
        // records like `Router.match`'s result are modeled as named classes,
        // not `Ty::Record`.)
        Ty::Record { .. } => "MutableMap<String, Any?>".to_string(),

        // Function type → Kotlin lambda type `(P1, P2) -> R`.
        Ty::Fn { params, ret, .. } => {
            let ps: Vec<String> = params.iter().map(|p| kotlin_ty(&p.ty)).collect();
            format!("({}) -> {}", ps.join(", "), kotlin_ty(ret))
        }

        // The soft-strict escape: `Any?`, with no emit diagnostic.
        Ty::Var { .. } | Ty::Untyped => "Any?".to_string(),
    }
}

/// Render a `Ty::Class`. Last-segment naming (Kotlin resolves by import,
/// not fully-qualified path), with the well-known cross-target special
/// cases: temporal classes stringify, `Regexp` → `Regex`, `Hash` →
/// `MutableMap`.
fn render_class(full: &str, args: &[Ty]) -> String {
    match full {
        "Date" | "Time" | "DateTime" | "ActiveSupport::TimeWithZone" => {
            return "String".to_string();
        }
        "Regexp" => return "Regex".to_string(),
        // The untyped-params value type. The from_raw lowering (and any
        // `@params[...]` access) treats a params value as a bare nested
        // Hash / String — `raw.is_a?(Hash) ? raw : {}`, `raw.is_a?(String)
        // ? raw : ""`. Rendering ParamValue as Kotlin's top type `Any?`
        // (Dict → MutableMap, Str → String at the value sites) makes those
        // `is Map<*,*>` / `is String` / `as MutableMap` checks hold as
        // emitted — a typed sealed wrapper would fail every check (a
        // ParamValue.Dict is not a Map), which silently dropped every
        // create's params (the e2e turbo/cable specs). Server.kt builds the
        // matching plain Map/String values.
        "Roundhouse::ParamValue" | "ParamValue" => return "Any?".to_string(),
        "Hash" => {
            return if args.len() == 2 {
                format!("MutableMap<{}, {}>", kotlin_ty(&args[0]), kotlin_ty(&args[1]))
            } else {
                "MutableMap<String, Any?>".to_string()
            };
        }
        _ => {}
    }
    let base = super::naming::type_name(full);
    if args.is_empty() {
        base
    } else {
        let parts: Vec<String> = args.iter().map(kotlin_ty).collect();
        format!("{base}<{}>", parts.join(", "))
    }
}

/// Render a union. Special-cases `T | Nil` as the nullable shorthand
/// `T?`. Heterogeneous (non-nullable) unions have no untagged Kotlin
/// analog — Phase 2 may generate a sealed type; for now they degrade to
/// `Any?` (nullable when the union admits Nil).
fn render_union(variants: &[Ty]) -> String {
    let has_nil = variants.iter().any(|t| matches!(t, Ty::Nil));
    let non_nil: Vec<&Ty> = variants.iter().filter(|t| !matches!(t, Ty::Nil)).collect();
    match non_nil.as_slice() {
        [] => "Nothing?".to_string(),
        [single] if has_nil => format!("{}?", kotlin_ty(single)),
        [single] => kotlin_ty(single),
        _ => "Any?".to_string(),
    }
}

/// True when the type is `Untyped` (or contains it). Decides whether a
/// method signature carries an annotation or leans on Kotlin inference.
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
