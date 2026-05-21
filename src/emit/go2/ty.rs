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
        _ => "interface{}".to_string(),
    }
}
