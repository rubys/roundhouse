use crate::ident::ClassId;
use crate::ty::Ty;

/// Map a Roundhouse `Ty` to its TypeScript type expression.
/// Conservative — gradual escape hatch (`Untyped`) → `any`.
pub fn ts_ty(ty: &Ty) -> String {
    match ty {
        Ty::Int | Ty::Float => "number".into(),
        Ty::Bool => "boolean".into(),
        Ty::Str | Ty::Sym => "string".into(),
        Ty::Nil => "null".into(),
        Ty::Array { elem } => format!("{}[]", ts_ty(elem)),
        Ty::Class { id, .. } => ts_class_ty(id),
        Ty::Untyped => "any".into(),
        Ty::Bottom => "never".into(),
        _ => "any".into(),
    }
}

/// Default value literal for a TS-typed field — used in `field: T = <default>;`.
pub fn ts_default(ty: &Ty) -> String {
    match ty {
        Ty::Int | Ty::Float => "0".into(),
        Ty::Bool => "false".into(),
        Ty::Str | Ty::Sym => "\"\"".into(),
        Ty::Nil => "null".into(),
        Ty::Class { id, .. } if class_is_temporal(id) => "\"\"".into(),
        _ => "null as any".into(),
    }
}

fn ts_class_ty(id: &ClassId) -> String {
    if class_is_temporal(id) {
        "string".into()
    } else {
        id.0.as_str().into()
    }
}

fn class_is_temporal(id: &ClassId) -> bool {
    matches!(
        id.0.as_str(),
        "Date" | "Time" | "DateTime" | "ActiveSupport::TimeWithZone"
    )
}
