//! Phase 1 Go type renderer for go2's library emit.
//!
//! `go_ty_stub` is the permissive variant — returns `interface{}` for
//! any Ty whose mapping isn't trivially obvious. The real Go type
//! renderer (`crate::emit::go::go_ty`) works from a Rails-domain
//! position; go2's stub emit needs a renderer that never crashes on
//! an unknown Ty.
//!
//! When go2's per-class emit gets sharper, this file grows into the
//! analog of `src/emit/rust2/ty.rs`. For now it returns `interface{}`
//! almost universally, which keeps Phase 1 output syntactically valid.

use crate::ty::Ty;

pub fn go_ty_stub(ty: Option<&Ty>) -> String {
    match ty {
        Some(Ty::Str) => "string".to_string(),
        Some(Ty::Int) => "int64".to_string(),
        Some(Ty::Float) => "float64".to_string(),
        Some(Ty::Bool) => "bool".to_string(),
        Some(Ty::Sym) => "string".to_string(),
        Some(Ty::Hash { key, value }) => {
            format!(
                "map[{}]{}",
                go_ty_stub(Some(key)),
                go_ty_stub(Some(value))
            )
        }
        Some(Ty::Array { elem }) => format!("[]{}", go_ty_stub(Some(elem))),
        // Union types stay `interface{}` — collapsing `Union{T, Nil}`
        // to `T` would break `s == nil` checks for value types
        // (Go strings/ints/floats can't be nil). The narrowing
        // walker in emit_return_at::Seq handles the runtime
        // assertion when an early-nil-return shape appears.
        Some(Ty::Class { id, .. }) => {
            // Promote to a Go pointer-to-struct using the same
            // `::` → identifier sanitization as `emit_library_class`.
            let name = id.0.as_str().replace("::", "");
            format!("*{name}")
        }
        _ => "interface{}".to_string(),
    }
}
