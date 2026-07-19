//! `Ty` → C# type-string.
//!
//! C# is a *soft* strict target like Kotlin (see
//! `docs/csharp-migration-plan.md`): unlike Rust/Go — which elevate any
//! reachable `Ty::Untyped` to an emit-time error — C# maps `Untyped`/`Var`
//! to `object?`, so the gradual-typing escape hatch survives emission.
//! Modeled on `src/emit/kotlin/ty.rs`, with the C# spellings substituted:
//! `Any?` → `object?`, `MutableList`/`MutableMap` → `List`/`Dictionary`,
//! `Pair`/`Triple`/wider → C# value tuples `(T1, T2, …)`.
//!
//! Convention: `Int` → `long` (Rails IDs are 64-bit on sqlite),
//! `Float` → `double`, `Sym` → `string` (C# has no symbol), generics use
//! angle brackets (`List<T>`, `Dictionary<K, V>`), and a `T | Nil` union
//! prefers the nullable shorthand `T?`.
#![allow(dead_code)]

use crate::ty::Ty;

pub fn csharp_ty(t: &Ty) -> String {
    match t {
        Ty::Int => "long".to_string(),
        Ty::Float => "double".to_string(),
        Ty::Bool => "bool".to_string(),
        Ty::Str => "string".to_string(),
        // C# has a native `DateTimeOffset`. Temporal columns store ISO-8601
        // text (a `string` backing field) and read back as a real
        // `DateTimeOffset` via an explicit parsing getter — see the temporal
        // branch in csharp/library.rs and `Roundhouse.RhDateTime.Parse`.
        Ty::Time => "DateTimeOffset".to_string(),
        // No symbol type in C# — route symbols to string keys, as the
        // Kotlin/TS/Crystal renderers do.
        Ty::Sym => "string".to_string(),
        // Bare `Nil` defaults to the return-slot rendering `void`; a
        // value-slot nil is reached via unions (`T | Nil → T?`). A
        // `csharp_return_ty` helper will refine the outermost slot in
        // Phase 2.
        Ty::Nil => "void".to_string(),
        // Analysis-time relation type — erased by query specialization
        // before emit (see `Ty::Relation`). Reaching here is a
        // coverage gap: report, never degrade to `List<T>`.
        Ty::Relation { of } => {
            return crate::emit::diagnostics::unsupported_relation_ty("csharp", of);
        }
        // Divergence type — `raise`/`return`. C# has no bottom type (no
        // `Nothing`/`!`); a `throw` expression is convertible to any type,
        // so the slot rarely needs a concrete render. Fall back to the soft
        // top `object?` (refine in Phase 2 if a real bottom slot surfaces).
        Ty::Bottom => "object?".to_string(),

        // AR result sets and view accumulators mutate; `List`/`Dictionary`
        // are the mutable defaults (C#'s `IReadOnlyList`/`IReadOnlyDictionary`
        // tightening is a Phase 2+ refinement driven by `mutates_self`).
        Ty::Array { elem } => format!("List<{}>", csharp_ty(elem)),
        Ty::Hash { key, value } => {
            format!("Dictionary<{}, {}>", csharp_ty(key), csharp_ty(value))
        }

        // C# has native value tuples of arbitrary arity (`(T1, T2, …)`),
        // so — unlike Kotlin's Pair/Triple ceiling — every tuple renders
        // directly. An empty tuple has no analog; degrade to `object?`.
        Ty::Tuple { elems } => {
            if elems.is_empty() {
                "object?".to_string()
            } else {
                let parts: Vec<String> = elems.iter().map(csharp_ty).collect();
                format!("({})", parts.join(", "))
            }
        }

        Ty::Union { variants } => render_union(variants),

        Ty::Class { id, args } => render_class(id.0.as_str(), args),

        // RBS record literal (e.g. an importmap pin `{name:, path:}`). C#
        // has no anonymous record type — render it as a string-keyed
        // dictionary, which is what the lowerer emits for these and lets
        // field reads work via `record["name"]`. (Genuinely typed records
        // like `Router.match`'s result are modeled as named classes, not
        // `Ty::Record`.)
        Ty::Record { .. } => "Dictionary<string, object?>".to_string(),

        // Function type → C# delegate. `Action<…>` when the result is
        // `void`/`Nil` (C# splits void-returning delegates from value-
        // returning ones), `Func<…, R>` otherwise.
        Ty::Fn { params, ret, .. } => {
            let ps: Vec<String> = params.iter().map(|p| csharp_ty(&p.ty)).collect();
            let ret_s = csharp_ty(ret);
            if ret_s == "void" {
                if ps.is_empty() {
                    "Action".to_string()
                } else {
                    format!("Action<{}>", ps.join(", "))
                }
            } else {
                let mut all = ps;
                all.push(ret_s);
                format!("Func<{}>", all.join(", "))
            }
        }

        // The soft-strict escape: `object?`, with no emit diagnostic.
        Ty::Var { .. } | Ty::Untyped => "object?".to_string(),
    }
}

/// Render a `Ty::Class`. Last-segment naming (the flat `Roundhouse`
/// namespace resolves by name, not fully-qualified path), with the
/// well-known cross-target special cases: temporal classes stringify,
/// `Regexp` → `Regex`, `Hash` → `Dictionary`.
fn render_class(full: &str, args: &[Ty]) -> String {
    match full {
        "Date" | "Time" | "DateTime" | "ActiveSupport::TimeWithZone" => {
            return "string".to_string();
        }
        "Regexp" => return "Regex".to_string(),
        // The untyped-params value type. The from_raw lowering treats a
        // params value as a bare nested Hash / String; rendering ParamValue
        // as C#'s top `object?` (Dict → Dictionary, Str → string at the
        // value sites) makes the lowered `is`/`as` checks hold as emitted —
        // a typed wrapper would fail every check and silently drop create
        // params (see the same note in `kotlin/ty.rs`).
        "Roundhouse::ParamValue" | "ParamValue" => return "object?".to_string(),
        "Hash" => {
            return if args.len() == 2 {
                format!("Dictionary<{}, {}>", csharp_ty(&args[0]), csharp_ty(&args[1]))
            } else {
                "Dictionary<string, object?>".to_string()
            };
        }
        _ => {}
    }
    let base = super::naming::type_name(full);
    if args.is_empty() {
        base
    } else {
        let parts: Vec<String> = args.iter().map(csharp_ty).collect();
        format!("{base}<{}>", parts.join(", "))
    }
}

/// Render a union. Special-cases `T | Nil` as the nullable shorthand `T?`.
/// Heterogeneous (non-nullable) unions have no untagged C# analog — Phase 2
/// may generate a sealed/abstract record hierarchy; for now they degrade to
/// `object?` (nullable, so it also admits Nil).
fn render_union(variants: &[Ty]) -> String {
    let has_nil = variants.iter().any(|t| matches!(t, Ty::Nil));
    let non_nil: Vec<&Ty> = variants.iter().filter(|t| !matches!(t, Ty::Nil)).collect();
    match non_nil.as_slice() {
        [] => "object?".to_string(),
        [single] if has_nil => {
            // Don't double the `?` when the variant is already nullable
            // (a nested `Union{Str?, Nil}` → `string?`, not `string??`).
            let s = csharp_ty(single);
            if s.ends_with('?') { s } else { format!("{s}?") }
        }
        [single] => csharp_ty(single),
        _ => "object?".to_string(),
    }
}

/// True when the type is `Untyped` (or contains it). Decides whether a
/// method signature carries an annotation or leans on C# `var` inference.
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
