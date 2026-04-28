//! TypeScript type rendering — `Ty` → TS type expression.

use crate::ty::Ty;

// Types ----------------------------------------------------------------

pub fn ts_ty(ty: &Ty) -> String {
    match ty {
        Ty::Int | Ty::Float => "number".to_string(),
        Ty::Bool => "boolean".to_string(),
        // Symbols model as string for now. When a pass identifies a
        // closed set of symbols at a given position (enum detection),
        // emit it as a union-of-string-literals instead.
        Ty::Str | Ty::Sym => "string".to_string(),
        Ty::Nil => "null".to_string(),
        Ty::Array { elem } => format!("{}[]", ts_ty(elem)),
        Ty::Hash { key, value } => {
            format!("Record<{}, {}>", ts_ty(key), ts_ty(value))
        }
        Ty::Tuple { elems } => {
            let parts: Vec<String> = elems.iter().map(ts_ty).collect();
            format!("[{}]", parts.join(", "))
        }
        Ty::Record { .. } => "Record<string, unknown>".to_string(),
        Ty::Union { variants } => {
            let parts: Vec<String> = variants.iter().map(ts_ty).collect();
            parts.join(" | ")
        }
        Ty::Class { id, .. } => id.0.to_string(),
        Ty::Fn { .. } => "(...args: unknown[]) => unknown".to_string(),
        Ty::Var { .. } => "unknown".to_string(),
        // RBS-declared `untyped` — TS's explicit gradual escape is `any`.
        // Distinct from `Ty::Var` (rendered `unknown`, must-narrow): the
        // author signed this opt-out, so the call-site doesn't have to
        // narrow before use.
        Ty::Untyped => "any".to_string(),
        // Bottom type — TypeScript's `never`. Used for divergent
        // expressions (raise/return/next). TS's narrowing benefits
        // from `never` in exhaustiveness checks.
        Ty::Bottom => "never".to_string(),
    }
}
