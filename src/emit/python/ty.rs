//! Python type rendering helpers.

use crate::ty::Ty;

// Types ----------------------------------------------------------------

pub fn python_ty(ty: &Ty) -> String {
    match ty {
        Ty::Int => "int".to_string(),
        Ty::Float => "float".to_string(),
        Ty::Bool => "bool".to_string(),
        Ty::Str | Ty::Sym => "str".to_string(),
        Ty::Nil => "None".to_string(),
        Ty::Array { elem } => format!("list[{}]", python_ty(elem)),
        Ty::Hash { key, value } => format!("dict[{}, {}]", python_ty(key), python_ty(value)),
        Ty::Tuple { elems } => {
            let parts: Vec<String> = elems.iter().map(python_ty).collect();
            format!("tuple[{}]", parts.join(", "))
        }
        Ty::Record { .. } => "dict[str, object]".to_string(),
        Ty::Union { variants } => {
            // PEP 604 union syntax: `A | B | C`. Python 3.10+.
            let parts: Vec<String> = variants.iter().map(python_ty).collect();
            parts.join(" | ")
        }
        Ty::Class { id, .. } => match id.0.as_str() {
            "Time" => "str".to_string(),
            other => other.to_string(),
        },
        Ty::Fn { .. } => "object".to_string(),
        Ty::Var { .. } => "object".to_string(),
    }
}

pub(super) fn python_default(ty: &Ty) -> String {
    match ty {
        Ty::Int => "0".to_string(),
        Ty::Float => "0.0".to_string(),
        Ty::Bool => "False".to_string(),
        Ty::Str | Ty::Sym => "\"\"".to_string(),
        Ty::Nil => "None".to_string(),
        Ty::Array { .. } => "[]".to_string(),
        Ty::Hash { .. } => "{}".to_string(),
        Ty::Class { id, .. } if id.0.as_str() == "Time" => "\"\"".to_string(),
        _ => "None".to_string(),
    }
}

pub(super) fn py_literal_for(value: &str, ty: &Ty) -> String {
    match ty {
        Ty::Str | Ty::Sym => format!("{value:?}"),
        Ty::Int => {
            if value.parse::<i64>().is_ok() {
                value.to_string()
            } else {
                format!("0  # TODO: coerce {value:?}")
            }
        }
        Ty::Float => {
            if value.parse::<f64>().is_ok() {
                value.to_string()
            } else {
                format!("0.0  # TODO: coerce {value:?}")
            }
        }
        Ty::Bool => match value {
            "true" | "1" => "True".into(),
            "false" | "0" => "False".into(),
            _ => format!("False  # TODO: coerce {value:?}"),
        },
        Ty::Class { id, .. } if id.0.as_str() == "Time" => format!("{value:?}"),
        _ => format!("{value:?}"),
    }
}
