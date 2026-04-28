//! Go type rendering — `Ty` → Go type expression.

use crate::ty::Ty;

pub fn go_ty(ty: &Ty) -> String {
    match ty {
        Ty::Int => "int64".to_string(),
        Ty::Float => "float64".to_string(),
        Ty::Bool => "bool".to_string(),
        Ty::Str | Ty::Sym => "string".to_string(),
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
