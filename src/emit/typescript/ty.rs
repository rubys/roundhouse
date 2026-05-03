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
        Ty::Hash { key, value } => format!("Record<{}, {}>", ts_ty(key), ts_ty(value)),
        Ty::Class { id, .. } => ts_class_ty(id),
        Ty::Untyped => "any".into(),
        Ty::Bottom => "never".into(),
        _ => "any".into(),
    }
}

/// Render a `Ty` for the return-type slot of a TS function/method.
/// Differs from `ts_ty` only at the OUTERMOST level: bare `Ty::Nil`
/// becomes `void` (the function returns nothing meaningful) instead
/// of `null` (a value type). Inner positions — including unions
/// containing Nil — recurse to `ts_ty` so `Ty::Union { Article, Nil }`
/// renders as `Article | null`, the right shape for a value the
/// caller might inspect.
pub fn ts_return_ty(ty: &Ty) -> String {
    match ty {
        Ty::Nil => "void".into(),
        _ => ts_ty(ty),
    }
}

fn ts_class_ty(id: &ClassId) -> String {
    let raw = id.0.as_str();
    if class_is_temporal(id) {
        return "string".into();
    }
    // Ruby builtins whose TS spelling differs:
    //   `Regexp` — JS calls it `RegExp` (capital E).
    //   `Hash`   — no TS class; `Record<string, any>` is the
    //              shape Ruby Hash flows through.
    //   `Symbol` — JS has `symbol` (lowercase) as a primitive
    //              type; method/field positions use `string` since
    //              Ruby symbols typically map to string keys.
    match raw {
        "Regexp" => return "RegExp".into(),
        "Hash" => return "Record<string, any>".into(),
        _ => {}
    }
    raw.into()
}

fn class_is_temporal(id: &ClassId) -> bool {
    matches!(
        id.0.as_str(),
        "Date" | "Time" | "DateTime" | "ActiveSupport::TimeWithZone"
    )
}
