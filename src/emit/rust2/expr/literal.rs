//! Container and literal emit — Hash, Array, Lambda/closure, String
//! interpolation, primitive Literal nodes. Each function takes the IR
//! sub-shape it owns and produces a self-contained Rust expression
//! string. Tail-position return-type coercion (the
//! `in_return_tail() && current_return_ty() == Hash<...>` peephole)
//! lives here because it's a property of the literal in tail position,
//! not of the surrounding emit.

use crate::expr::{Expr, ExprNode, InterpPart, Literal};

use super::util::indent;
use super::{
    coerce_arg_for_param_ty, current_return_ty, emit_expr, in_return_tail,
};

/// Emit a Hash literal as `std::collections::HashMap::from([(k, v), ...])`.
/// Empty literals become `HashMap::new()`. Heterogeneous-value tuples
/// get `.to_string()` coercion to keep type unification happy when the
/// surrounding map is String-typed.
pub(super) fn emit_hash(entries: &[(Expr, Expr)]) -> String {
    if entries.is_empty() {
        return "std::collections::HashMap::new()".to_string();
    }
    // Tuple-type unification: HashMap::from([(k, v), ...]) infers from
    // the first tuple; later tuples must share that type. Coerce
    // string-literal values to String when any sibling value is a
    // non-literal String-typed expression.
    let has_non_literal_str_value = entries.iter().any(|(_, v)| {
        !matches!(&*v.node, ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } })
            && matches!(v.ty.as_ref(), Some(crate::ty::Ty::Str) | Some(crate::ty::Ty::Sym))
    });
    // Tail-position return-type coercion: when the literal is the
    // method body's tail AND the declared return is `Hash<String, V>`,
    // coerce keys to String and values to V's storage. Without this,
    // tuple inference picks the first value's type and trips E0308.
    let return_hash_kv: Option<(crate::ty::Ty, crate::ty::Ty)> = if in_return_tail() {
        match current_return_ty() {
            Some(crate::ty::Ty::Hash { key, value }) => Some((*key, *value)),
            _ => None,
        }
    } else {
        None
    };

    // Heterogeneous primitive-value detection. When entries mix a
    // string-typed value (Ty::Str / Sym) with a non-string primitive
    // (Ty::Int / Bool / Float), `HashMap::from([(k, v), ...])` infers
    // V from the first entry's type and rejects later entries — even
    // when callers will wrap the result in `.into_iter().map(...)
    // .collect()` to coerce K/V at the boundary, the inner array
    // literal must already type-unify. Render values as
    // `serde_json::Value::from(v)` and keys as `(k).to_string()` so
    // V uniforms to `Value` at the literal level and the produced
    // map is `HashMap<String, Value>` — the AR shape that
    // `Model::new(attrs)` / `Model::create(attrs)` callees expect.
    // Gated on all-string-typed keys (the typical Ruby hash literal
    // shape) so non-string-key maps aren't accidentally coerced.
    // Tail-position lit (return_hash_kv set) keeps its own coerce path.
    let any_str_value = entries
        .iter()
        .any(|(_, v)| matches!(v.ty.as_ref(), Some(crate::ty::Ty::Str | crate::ty::Ty::Sym)));
    let any_nonstr_primitive = entries.iter().any(|(_, v)| {
        matches!(
            v.ty.as_ref(),
            Some(crate::ty::Ty::Int | crate::ty::Ty::Bool | crate::ty::Ty::Float)
        )
    });
    let all_str_keys = entries
        .iter()
        .all(|(k, _)| matches!(k.ty.as_ref(), Some(crate::ty::Ty::Str | crate::ty::Ty::Sym)));
    let is_heterogeneous = any_str_value && any_nonstr_primitive && all_str_keys;
    if is_heterogeneous && return_hash_kv.is_none() {
        let pairs: Vec<String> = entries
            .iter()
            .map(|(k, v)| {
                let k_s = emit_expr(k);
                let v_s = emit_expr(v);
                format!("(({k_s}).to_string(), serde_json::Value::from({v_s}))")
            })
            .collect();
        return format!("std::collections::HashMap::from([{}])", pairs.join(", "));
    }
    let pairs: Vec<String> = entries
        .iter()
        .map(|(k, v)| {
            let str_color_handled = v.str_coercion.is_some();
            let v_raw = emit_expr(v);
            let v_s = if let Some((_, ref v_ty)) = return_hash_kv {
                // Return-tail Hash storage: keys/values land in the
                // declared HashMap<K, V>, not at a callee param. Family
                // 4's `Str→&str` Borrow is wrong here — V is `String`
                // (owned), not `&str`. For Str-storage of an Ivar/Var/
                // Send (owned-String producers), emit `.clone()` and
                // strip any prior Borrow str_coercion so the value is
                // owned String at the storage slot. Other v_ty shapes
                // fall through to the param-position coerce.
                if matches!(v_ty, crate::ty::Ty::Str | crate::ty::Ty::Sym)
                    && matches!(
                        &*v.node,
                        ExprNode::Ivar { .. } | ExprNode::Var { .. } | ExprNode::Send { .. }
                    )
                {
                    // Strip any leading `&(...)` Borrow coercion str_
                    // color applied (it expected an &str arg position),
                    // then clone for ownership at the storage slot.
                    let bare = match v.str_coercion {
                        Some(crate::expr::StrCoercion::Borrow) => {
                            // `&(raw)` ⇒ `raw`
                            v_raw
                                .strip_prefix("&(")
                                .and_then(|s| s.strip_suffix(")"))
                                .map(|s| s.to_string())
                                .unwrap_or(v_raw.clone())
                        }
                        _ => v_raw.clone(),
                    };
                    format!("{bare}.clone()")
                } else {
                    coerce_arg_for_param_ty(v, v_ty)
                }
            } else if !str_color_handled
                && has_non_literal_str_value
                && matches!(&*v.node, ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } })
            {
                format!("{v_raw}.to_string()")
            } else {
                v_raw
            };
            let k_raw = emit_expr(k);
            let k_s = if let Some((ref k_ty, _)) = return_hash_kv {
                match k_ty {
                    crate::ty::Ty::Str | crate::ty::Ty::Sym
                        if matches!(
                            &*k.node,
                            ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } }
                        ) && k.str_coercion.is_none() =>
                    {
                        format!("{k_raw}.to_string()")
                    }
                    _ => k_raw,
                }
            } else {
                k_raw
            };
            format!("({k_s}, {v_s})")
        })
        .collect();
    format!("std::collections::HashMap::from([{}])", pairs.join(", "))
}

/// Emit an Array literal as a `vec![...]` macro invocation. Tail-position
/// return-type coercion forces string-literal elements to `String` when
/// the function returns `Vec<String>` / `Vec<Sym>`.
pub(super) fn emit_array(elements: &[Expr]) -> String {
    let return_elem_ty: Option<crate::ty::Ty> = if in_return_tail() {
        match current_return_ty() {
            Some(crate::ty::Ty::Array { elem }) => Some(*elem),
            _ => None,
        }
    } else {
        None
    };
    let coerce_to_string_elem = matches!(
        return_elem_ty.as_ref(),
        Some(crate::ty::Ty::Str | crate::ty::Ty::Sym)
    );
    // Ty::Record and Ty::Untyped both render as `serde_json::Value` at
    // the rust2 emit. A Vec of either reaches the function tail wanting
    // Value-shaped elements; route through coerce_arg_for_param_ty so
    // the Hash-literal-to-Value transform fires per element.
    let coerce_via_param_ty = matches!(
        return_elem_ty.as_ref(),
        Some(crate::ty::Ty::Untyped) | Some(crate::ty::Ty::Record { .. })
    );
    let parts: Vec<String> = elements
        .iter()
        .map(|e| {
            if coerce_via_param_ty {
                // Vec<Value> return — route each element through the
                // shared Family 3 / Hash-literal-to-Value transform so
                // HashMap literals and primitive elements emit as
                // `serde_json::Value` for the storage slot.
                if let Some(ty) = return_elem_ty.as_ref() {
                    return coerce_arg_for_param_ty(e, ty);
                }
            }
            let raw = emit_expr(e);
            if coerce_to_string_elem
                && matches!(
                    &*e.node,
                    ExprNode::Lit {
                        value: Literal::Str { .. } | Literal::Sym { .. }
                    }
                )
                && e.str_coercion.is_none()
            {
                format!("{raw}.to_string()")
            } else {
                raw
            }
        })
        .collect();
    format!("vec![{}]", parts.join(", "))
}

/// Build a Rust closure literal `|params| body` from a Lambda IR
/// node. Single-line bodies inline; multi-line bodies wrap in
/// `{ ... }`. No type annotations on params — call-site inference
/// handles the cases we hit; explicit types come later when generic
/// Lambda usage forces them.
pub(super) fn emit_closure(params: &[crate::ident::Symbol], body: &Expr) -> String {
    let ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
    let body_s = emit_expr(body);
    if body_s.contains('\n') {
        format!("|{}| {{\n{}\n}}", ps.join(", "), indent(&body_s, 1))
    } else {
        format!("|{}| {{ {body_s} }}", ps.join(", "))
    }
}

/// Append a block-as-closure to a `recv.method(...)` call. The block's
/// Lambda IR carries params + body; we emit a closure literal and
/// splice it as the last arg.
pub(super) fn attach_block(base: &str, block: &Expr) -> String {
    let closure = if let ExprNode::Lambda { params, body, .. } = &*block.node {
        emit_closure(params, body)
    } else {
        format!("/* TODO rust2: non-Lambda block: {:?} */", std::mem::discriminant(&*block.node))
    };
    if let Some(stripped) = base.strip_suffix("()") {
        format!("{stripped}({closure})")
    } else if let Some(stripped) = base.strip_suffix(')') {
        format!("{stripped}, {closure})")
    } else {
        format!("{base}({closure})")
    }
}

/// `recv.is_a?(Class)` → serde_json predicate where the class name
/// maps to a Value variant, else `false` with a marker comment.
pub(super) fn emit_is_a(recv: &Expr, class_arg: &Expr) -> String {
    let class_name = match &*class_arg.node {
        ExprNode::Const { path } => path.last().map(|s| s.to_string()).unwrap_or_default(),
        _ => return format!("/* is_a? unknown class: {} */ false", emit_expr(class_arg)),
    };
    let recv_s = emit_expr(recv);
    let predicate = match class_name.as_str() {
        "Hash" => Some("is_object"),
        "Array" => Some("is_array"),
        "String" => Some("is_string"),
        "Integer" => Some("is_i64"),
        "Float" => Some("is_f64"),
        "TrueClass" | "FalseClass" => Some("is_boolean"),
        "NilClass" => Some("is_null"),
        _ => None,
    };
    match predicate {
        Some(p) => format!("{recv_s}.{p}()"),
        None => format!("/* is_a?({class_name}): no Value variant */ false"),
    }
}

/// `#{x} is #{y}` → `format!("{} is {}", x, y)`. Literal text escapes
/// `{`/`}` as `{{`/`}}`; each interp `Expr` becomes a `{}` placeholder
/// + arg.
pub(super) fn emit_string_interp(parts: &[InterpPart]) -> String {
    let mut fmt = String::from("format!(\"");
    let mut args: Vec<String> = Vec::new();
    for p in parts {
        match p {
            InterpPart::Text { value } => {
                for c in value.chars() {
                    match c {
                        '"' => fmt.push_str("\\\""),
                        '\\' => fmt.push_str("\\\\"),
                        '\n' => fmt.push_str("\\n"),
                        '\r' => fmt.push_str("\\r"),
                        '\t' => fmt.push_str("\\t"),
                        '{' => fmt.push_str("{{"),
                        '}' => fmt.push_str("}}"),
                        other => fmt.push(other),
                    }
                }
            }
            InterpPart::Expr { expr } => {
                fmt.push_str("{}");
                args.push(emit_expr(expr));
            }
        }
    }
    fmt.push_str("\"");
    if !args.is_empty() {
        fmt.push_str(", ");
        fmt.push_str(&args.join(", "));
    }
    fmt.push(')');
    fmt
}

/// Primitive literal → Rust literal. `nil` → `None` so Option-typed
/// fields work; integer literals get the `_i64` suffix to commit to
/// the rust2 integer convention; floats get a `.0` to keep them
/// floating-typed when the value has no fractional part.
pub(crate) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "None".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => format!("{value}_i64"),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        Literal::Sym { value } => format!("{:?}", value.as_str()),
        Literal::Regex { pattern, .. } => format!("/* TODO rust2: Regex({pattern:?}) */"),
    }
}
