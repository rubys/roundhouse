//! Rust type rendering. `rust_ty` lowers an analyzer `Ty` to its Rust
//! source-text spelling. Re-exported from `super` so external callers
//! (currently only inside this crate) can keep using `emit::rust::rust_ty`.

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
        Ty::Hash { key, value } => {
            format!("std::collections::HashMap<{}, {}>", rust_ty(key), rust_ty(value))
        }
        Ty::Tuple { elems } => {
            let parts: Vec<String> = elems.iter().map(rust_ty).collect();
            format!("({})", parts.join(", "))
        }
        Ty::Record { .. } => "serde_json::Value".to_string(),
        Ty::Union { variants } => option_shape(variants).unwrap_or_else(|| {
            // Non-nullable unions: fall back to a boxed trait object for now.
            // Real answer: emit an enum. Landing when a fixture demands it.
            "Box<dyn std::any::Any>".to_string()
        }),
        Ty::Class { id, .. } => match id.0.as_str() {
            // Schema Date/DateTime/Time columns carry Ty::Class(Time); map
            // to String for now so models emit compilable Rust. A future
            // step with a chrono/time dep can upgrade this to a real
            // DateTime type.
            "Time" => "String".to_string(),
            other => other.to_string(),
        },
        Ty::Fn { .. } => "Box<dyn Fn()>".to_string(),
        Ty::Var { .. } => "()".to_string(),
        // RBS-declared `untyped`. Rust has no native gradual escape;
        // any node carrying `Ty::Untyped` that reaches an emit-relevant
        // position is a fail-stop (the diagnostic pipeline elevates it
        // to Error before this renderer runs in a strict pipeline).
        // For surfaces that *can* tolerate it (debug renderings,
        // exploratory emits), fall back to `()` to mirror Var. Real
        // path forward is `Box<dyn _Adapter>` once the corpus declares
        // its interfaces — at which point this branch should be
        // unreachable.
        Ty::Untyped => "()".to_string(),
        // The bottom type — Rust's `!` (never). Has special
        // subtyping: coerces to any other type. `if cond { panic!() }
        // else { x }` types as typeof(x) cleanly because of this.
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
