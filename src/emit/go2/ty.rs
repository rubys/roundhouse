//! Phase 1 Go type renderer for go2's library emit.
//!
//! `go_ty_stub` is the permissive variant — returns `interface{}` for
//! any Ty whose mapping isn't trivially obvious. The committed Go type
//! renderer (`go_ty`, below) works from a Rails-domain position and is
//! used by the runtime-extraction `emit_method` in `src/emit/go.rs`;
//! go2's stub emit needs a renderer that never crashes on an unknown
//! Ty, which `go_ty_stub` provides.
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
        // Anonymous record types — RBS `{ name: String, path: String }`.
        // Go has no NamedTuple analog, so collapse to a `map[string]T`.
        // When every field shares the same Ty (the common case: an
        // importmap-pin record with all-String fields), use that Ty
        // as the value type so use-site `p["name"]` indexes cleanly.
        // Heterogeneous records widen to `map[string]any` — callers
        // type-assert per field at the use site, matching Go's idiom
        // for dynamic-key maps.
        Some(Ty::Record { row }) => {
            let mut field_tys = row.fields.values();
            match field_tys.next() {
                Some(first) if field_tys.clone().all(|t| t == first) => {
                    format!("map[string]{}", go_ty_stub(Some(first)))
                }
                _ => "map[string]any".to_string(),
            }
        }
        // `Union { Nil, T }` collapses to `T`'s Go type IFF `T` maps
        // to a Go reference type (map, slice, pointer-to-struct) OR
        // is a string (where "" stands in for nil, the Go-idiomatic
        // empty-as-nil convention used by Flash's `@notice: String?`
        // shape). Integer/Float/Bool Unions stay `interface{}`
        // because their zero values (0, 0.0, false) are meaningful
        // — `0 == nil` would conflate "absent" with the actual
        // value zero. Strings get the convention because empty-as-
        // missing is so common in Ruby's framework code that the
        // alternative (per-field `*string`) is more invasive than
        // it's worth.
        Some(Ty::Union { variants }) => {
            let non_nil: Vec<&Ty> = variants
                .iter()
                .filter(|t| !matches!(t, Ty::Nil))
                .collect();
            if non_nil.len() == 1 {
                match non_nil[0] {
                    Ty::Hash { .. } | Ty::Array { .. } | Ty::Class { .. }
                    | Ty::Str | Ty::Sym => go_ty_stub(Some(non_nil[0])),
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

/// Committed `Ty` → Go type renderer (was `crate::emit::go::go_ty`).
/// Unlike `go_ty_stub`, this commits every Ty to a concrete Go type;
/// used by `src/emit/go.rs::emit_method` for the runtime-extraction
/// pipeline.
pub fn go_ty(ty: &Ty) -> String {
    match ty {
        Ty::Int => "int64".to_string(),
        Ty::Float => "float64".to_string(),
        Ty::Bool => "bool".to_string(),
        Ty::Str | Ty::Sym => "string".to_string(),
        // Go has time.Time, but the datetime seam isn't wired yet
        // (Stage 2) — a Time surface is an honest not-supported gap.
        // (The legacy `Ty::Class{"Time"} => "string"` stand-in below is
        // separate: it catches hand-written-rbs Time in the shared
        // runtime, not the first-class `Ty::Time` column type.)
        Ty::Time => crate::emit::diagnostics::unsupported_time_ty("go"),
        Ty::Nil => "struct{}".to_string(),
        Ty::Array { elem } => format!("[]{}", go_ty(elem)),
        Ty::Hash { key, value } => format!("map[{}]{}", go_ty(key), go_ty(value)),
        Ty::Tuple { elems } => {
            // Go has no tuple; collapse to interface{} for now.
            let _ = elems;
            "interface{}".to_string()
        }
        Ty::Record { .. } => "map[string]interface{}".to_string(),
        Ty::Union { variants } => option_shape(variants).unwrap_or_else(|| {
            // Arbitrary union -> empty interface; would be a sum type emit later.
            "interface{}".to_string()
        }),
        Ty::Class { id, .. } => match id.0.as_str() {
            // Schema DateTime/Date/Time columns carry Ty::Class(Time); Go
            // has a time package (`time.Time`) but wiring that import
            // into emit adds complexity. String is the same pragmatic
            // stand-in Rust uses; real timestamps arrive when a DB
            // adapter does.
            "Time" => "string".to_string(),
            other => other.to_string(),
        },
        Ty::Fn { .. } => "func()".to_string(),
        Ty::Var { .. } => "interface{}".to_string(),
        // Go has no syntactic distinction between "must narrow" and
        // "gradual" — both collapse to `interface{}`. The Var/Untyped
        // distinction survives in the IR and via diagnostics; Go-side
        // codegen renders both identically.
        Ty::Untyped => "interface{}".to_string(),
        // Go has no native bottom type. Functions that always
        // panic/exit return no value (or `interface{}` if the
        // surrounding context demands a value). Render as
        // `interface{}` — same shape as Untyped — and rely on Go's
        // unreachable-code analysis to catch missing returns.
        Ty::Bottom => "interface{}".to_string(),
    }
}

fn option_shape(variants: &[Ty]) -> Option<String> {
    if variants.len() != 2 {
        return None;
    }
    match (&variants[0], &variants[1]) {
        (Ty::Nil, other) | (other, Ty::Nil) => Some(format!("*{}", go_ty(other))),
        _ => None,
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
        // `Roundhouse::Modeler` is the Q1 back-pointer interface
        // declared in `runtime/go/v2/modeler.go`. Carried on
        // `ActiveRecordBase.Self` for polymorphic dispatch into the
        // outer subclass.
        | "Roundhouse::Modeler"
    )
}
