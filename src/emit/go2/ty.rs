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
        // `Union { Nil, T }` collapses to `T`'s Go type IFF `T` maps
        // to a Go reference type (map, slice, pointer-to-struct) —
        // those carry nil at the type level. Value types (string,
        // int, float, bool) can't be nil in Go, so the Union stays
        // `interface{}` and gets narrowed at runtime via the
        // emit_return_at::Seq early-nil-return walker.
        Some(Ty::Union { variants }) => {
            let non_nil: Vec<&Ty> = variants
                .iter()
                .filter(|t| !matches!(t, Ty::Nil))
                .collect();
            if non_nil.len() == 1 {
                match non_nil[0] {
                    Ty::Hash { .. } | Ty::Array { .. } | Ty::Class { .. } => {
                        go_ty_stub(Some(non_nil[0]))
                    }
                    _ => "interface{}".to_string(),
                }
            } else {
                "interface{}".to_string()
            }
        }
        Some(Ty::Class { id, .. }) => {
            // Promote to a Go pointer-to-struct using the same
            // `::` → identifier sanitization as `emit_library_class`.
            // Exception: known-interface classes (hand-written Go
            // interfaces in `runtime/go/v2/`, mirroring RBS-declared
            // phantom classes) emit as the bare type. Pointer-to-
            // interface has an empty method set in Go, so method
            // calls through the slot would fail to resolve; the
            // interface value itself already has reference semantics.
            let name = id.0.as_str().replace("::", "");
            if is_go_interface_class(id.0.as_str()) {
                name
            } else {
                format!("*{name}")
            }
        }
        _ => "interface{}".to_string(),
    }
}

/// Class IDs whose Go counterpart is an `interface` declaration
/// (hand-written in `runtime/go/v2/`) rather than a struct. Slot
/// types reference these as bare names — `var x Foo` not `var x *Foo`
/// — because pointer-to-interface has an empty method set in Go.
fn is_go_interface_class(id: &str) -> bool {
    matches!(
        id,
        "ActiveRecord::AdapterInterface"
        // `Roundhouse::ParamValue` is a hand-written `type
        // RoundhouseParamValue = any` alias (recursive sum type
        // can't be expressed as a Go struct). Like `AdapterInterface`,
        // it must emit bare — `*any` is pointer-to-interface with an
        // empty method set.
        | "Roundhouse::ParamValue"
    )
}
