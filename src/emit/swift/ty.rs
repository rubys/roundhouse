//! `Ty` → Swift type-string.
//!
//! Swift is a *soft* strict target, exactly like Kotlin (see
//! `docs/swift-migration-plan.md`): unlike Rust/Go — which elevate any
//! reachable `Ty::Untyped` to an emit-time error — Swift maps
//! `Untyped`/`Var` to `Any?`, so the gradual-typing escape hatch survives
//! emission. Ported from `src/emit/kotlin/ty.rs` (the template).
//!
//! Convention vs Kotlin: `Int` → `Int` (Swift `Int` is 64-bit on 64-bit
//! platforms — no `Long`/`L`-suffix dance), collections are the value
//! types `[T]` / `[K: V]`, tuples are native at any arity, and a
//! `T | Nil` union prefers the nullable shorthand `T?`.
#![allow(dead_code)]

use crate::ty::Ty;

pub fn swift_ty(t: &Ty) -> String {
    match t {
        Ty::Int => "Int".to_string(),
        Ty::Float => "Double".to_string(),
        Ty::Bool => "Bool".to_string(),
        Ty::Str => "String".to_string(),
        // No symbol type in Swift — route symbols to string keys, as the
        // TS/Crystal/Kotlin renderers do.
        Ty::Sym => "String".to_string(),
        // Bare `Nil` defaults to the return-slot rendering `Void`; a
        // value-slot optional is reached via unions (`T | Nil → T?`).
        // A `swift_return_ty` helper will refine the outermost slot in
        // Phase 2.
        Ty::Nil => "Void".to_string(),
        // Divergence type — `raise`/`return`. Swift's `Never` (⊥) is the
        // direct analog of Kotlin `Nothing` / Rust `!`.
        Ty::Bottom => "Never".to_string(),

        // Swift arrays/dictionaries are value types declared with the
        // sugar forms. Mutability lives on the binding (`var` vs `let`),
        // not the type — no MutableList/List split to manage.
        Ty::Array { elem } => format!("[{}]", swift_ty(elem)),
        Ty::Hash { key, value } => {
            format!("[{}: {}]", swift_ty(key), swift_ty(value))
        }

        // Native tuples at any arity — no Pair/Triple/data-class
        // fallback ladder like Kotlin.
        Ty::Tuple { elems } => match elems.as_slice() {
            [] => "Void".to_string(),
            [single] => swift_ty(single),
            many => {
                let parts: Vec<String> = many.iter().map(swift_ty).collect();
                format!("({})", parts.join(", "))
            }
        },

        Ty::Union { variants } => render_union(variants),

        Ty::Class { id, args } => render_class(id.0.as_str(), args),

        // RBS record literal (e.g. Router.match's typed return). Swift
        // could generate a named `struct`; until a consumer forces that,
        // this is the same permissive placeholder Kotlin ships
        // (`MutableMap<String, Any?>` there).
        Ty::Record { .. } => "[String: Any?]".to_string(),

        // Function type → Swift function type `(P1, P2) -> R`.
        Ty::Fn { params, ret, .. } => {
            let ps: Vec<String> = params.iter().map(|p| swift_ty(&p.ty)).collect();
            format!("({}) -> {}", ps.join(", "), swift_ty(ret))
        }

        // The soft-strict escape: `Any?`, with no emit diagnostic.
        Ty::Var { .. } | Ty::Untyped => "Any?".to_string(),
    }
}

/// Render a `Ty::Class`. Named via `naming::type_name` (last segment +
/// the `*::Base` flat-module disambiguation), with the well-known
/// cross-target special cases: temporal classes stringify, `Regexp` →
/// `NSRegularExpression` (placeholder until a consumer decides between
/// it and native `Regex`), `Hash` → the dictionary sugar.
fn render_class(full: &str, args: &[Ty]) -> String {
    match full {
        "Date" | "Time" | "DateTime" | "ActiveSupport::TimeWithZone" => {
            return "String".to_string();
        }
        "Regexp" => return "NSRegularExpression".to_string(),
        // The params union renders as the top type — the Kotlin arc
        // proved the sealed-union shape doesn't survive the runtime's
        // untyped narrowing (`is_a?(Hash)` doesn't match a union wrapper
        // and silently drops every param); the recursive params value is
        // plain nested `[String: Any?]` maps end-to-end.
        "Roundhouse::ParamValue" | "ParamValue" => return "Any?".to_string(),
        "Hash" => {
            return if args.len() == 2 {
                format!("[{}: {}]", swift_ty(&args[0]), swift_ty(&args[1]))
            } else {
                "[String: Any?]".to_string()
            };
        }
        _ => {}
    }
    let base = super::naming::type_name(full);
    if args.is_empty() {
        base
    } else {
        let parts: Vec<String> = args.iter().map(swift_ty).collect();
        format!("{base}<{}>", parts.join(", "))
    }
}

/// Render a union. Special-cases `T | Nil` as the optional shorthand
/// `T?` (function types get the parens Swift requires: `((A) -> B)?`).
/// Nested unions flatten first — `(String | Nil) | Nil` is `String?`,
/// not `String??`. Heterogeneous (non-nullable) unions have no untagged
/// Swift analog — a generated `enum` with associated values is the
/// eventual shape (cf. `ParamValue` in `swift-reference/`); for now
/// they degrade to `Any?`.
fn render_union(variants: &[Ty]) -> String {
    fn flatten<'a>(variants: &'a [Ty], out: &mut Vec<&'a Ty>) {
        for v in variants {
            match v {
                Ty::Union { variants } => flatten(variants, out),
                _ => out.push(v),
            }
        }
    }
    let mut flat: Vec<&Ty> = Vec::new();
    flatten(variants, &mut flat);
    let has_nil = flat.iter().any(|t| matches!(t, Ty::Nil));
    let non_nil: Vec<&Ty> = flat.into_iter().filter(|t| !matches!(t, Ty::Nil)).collect();
    match non_nil.as_slice() {
        [] => "Void".to_string(),
        [single] if has_nil => {
            let inner = swift_ty(single);
            if matches!(single, Ty::Fn { .. }) {
                format!("({inner})?")
            } else {
                format!("{inner}?")
            }
        }
        [single] => swift_ty(single),
        _ => "Any?".to_string(),
    }
}

/// True when the type is `Untyped` (or contains it). Decides whether a
/// method signature carries an annotation or leans on Swift inference.
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
