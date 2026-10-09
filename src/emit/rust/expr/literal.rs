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
    coerce_arg_for_param_ty, current_return_ty, emit_expr, in_return_tail, with_closure_vars_scope,
    with_current_return_ty,
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
        !matches!(
            &*v.node,
            ExprNode::Lit {
                value: Literal::Str { .. } | Literal::Sym { .. }
            }
        ) && matches!(
            v.ty.as_ref(),
            Some(crate::ty::Ty::Str) | Some(crate::ty::Ty::Sym)
        )
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
            let str_color_handled = super::has_str_coercion(v);
            let v_raw = emit_expr(v);
            let v_s = if let Some((_, ref v_ty)) = return_hash_kv {
                if matches!(v_ty, crate::ty::Ty::Untyped) {
                    // HashMap<String, Value> return tails need every
                    // heterogeneous column value converted, including
                    // nullable fields and runtime calls that emit as
                    // borrowed strings. Value::from handles both
                    // Option<T> and already-Value expressions.
                    format!("serde_json::Value::from({v_raw})")
                } else if matches!(v_ty, crate::ty::Ty::Str | crate::ty::Ty::Sym)
                    && matches!(
                        &*v.node,
                        ExprNode::Ivar { .. } | ExprNode::Var { .. } | ExprNode::Send { .. }
                    )
                {
                    // Strip any leading `&(...)` Borrow coercion str_
                    // color applied (it expected an &str arg position),
                    // then clone for ownership at the storage slot.
                    let bare = if v.decisions & super::super::decide::bits::STR_BORROW != 0 {
                        // `&(raw)` ⇒ `raw`
                        v_raw
                            .strip_prefix("&(")
                            .and_then(|s| s.strip_suffix(")"))
                            .map(|s| s.to_string())
                            .unwrap_or(v_raw.clone())
                    } else {
                        v_raw.clone()
                    };
                    format!("{bare}.clone()")
                } else {
                    coerce_arg_for_param_ty(v, v_ty)
                }
            } else if !str_color_handled
                && has_non_literal_str_value
                && matches!(
                    &*v.node,
                    ExprNode::Lit {
                        value: Literal::Str { .. } | Literal::Sym { .. }
                    }
                )
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
                            ExprNode::Lit {
                                value: Literal::Str { .. } | Literal::Sym { .. }
                            }
                        ) && !super::has_str_coercion(k) =>
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
    // the rust emit. A Vec of either reaches the function tail wanting
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
                && !super::has_str_coercion(e)
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
///
/// The body is a fresh Rust block, so its `let` bindings are not the
/// enclosing method's. Snapshot the declared-var set the way `if` and
/// `while` do: an outer `let _cap` must not turn the nested capture
/// accumulator's first `_cap2 = …` into a rebind of a name this
/// closure never declared. Outer names stay in the snapshot, so a
/// genuine capture rebind (`_cap = _cap + …` inside the closure that
/// declared `_cap`) still emits without a second `let`.
pub(super) fn emit_closure(params: &[crate::ident::Symbol], body: &Expr) -> String {
    let ps: Vec<String> = params
        .iter()
        .map(|p| super::util::escape_rust_keyword(p.as_str()))
        .collect();
    // A closure has its own return type. Inheriting the enclosing
    // method's Option<T> return type makes tail-position conditionals
    // inside a block spuriously wrap their value in Some(...), and can
    // even produce invalid Rust at a nested block boundary.
    let body_s = with_current_return_ty(None, || with_closure_vars_scope(body, || emit_expr(body)));
    if body_s.contains('\n') {
        format!("|{}| {{\n{}\n}}", ps.join(", "), indent(&body_s, 1))
    } else {
        format!("|{}| {{ {body_s} }}", ps.join(", "))
    }
}

/// Append a block-as-closure to a `recv.method(...)` call. The block's
/// IR shape determines what gets spliced as the last arg:
///   - `Lambda { params, body }`: emit a closure literal `|p1,..| { body }`.
///   - `Var { name }`: emit the bare identifier — `&block` forwarding
///     idiom (issue #25 stage 2). The slot context (`Send.block:`)
///     signals "this Var is a Proc forward", so no new IR variant
///     is needed. The forwarded name matches the def-site closure
///     param (see `render_block_param_placeholder` in method.rs).
pub(super) fn attach_block(base: &str, block: &Expr) -> String {
    let closure = match &*block.node {
        ExprNode::Lambda { params, body, .. } => emit_closure(params, body),
        ExprNode::Var { name, .. } => name.as_str().to_string(),
        _ => format!(
            "/* TODO rust2: non-Lambda/non-Var block: {:?} */",
            std::mem::discriminant(&*block.node)
        ),
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
        "NilClass" => Some("is_null"),
        _ => None,
    };
    // `TrueClass` and `FalseClass` are DISTINCT Ruby classes, and code
    // that tests them tests them separately — JsonBuilder's `encode_value`
    // returns "true" for one and "false" for the other. Both mapping to
    // `is_boolean()` made the first arm swallow the second, so
    // `encode_value(false)` answered "true". Compare the value itself.
    match class_name.as_str() {
        "TrueClass" => return format!("{recv_s} == true"),
        "FalseClass" => return format!("{recv_s} == false"),
        _ => {}
    }
    match predicate {
        Some(p) => format!("{recv_s}.{p}()"),
        None => format!("/* is_a?({class_name}): no Value variant */ false"),
    }
}

/// `#{x} is #{y}` → `format!("{} is {}", x, y)`. Literal text escapes
/// `{`/`}` as `{{`/`}}`; each interp `Expr` becomes a `{}` placeholder
/// + arg.
pub(super) fn emit_string_interp(parts: &[InterpPart]) -> String {
    let (fmt, args) = string_interp_fmt_and_args(parts);
    let mut out = format!("format!(\"{fmt}\"");
    if !args.is_empty() {
        out.push_str(", ");
        out.push_str(&args.join(", "));
    }
    out.push(')');
    out
}

/// Same lowering split into (format-string body, rendered args) so
/// `ops.rs::try_string_append` can splice the pieces into
/// `write!(io, "...", args)` — formatting directly into the
/// accumulator instead of allocating an intermediate `String` via
/// `format!` and copying it in with `push_str` (roundhouse#32).
pub(super) fn string_interp_fmt_and_args(parts: &[InterpPart]) -> (String, Vec<String>) {
    let mut fmt = String::new();
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
                // String interpolation in Ruby calls `to_s` on each
                // interp value (`"#{x}"` == `x.to_s`). Rust's `"{}"`
                // format spec uses `Display`, which for
                // `serde_json::Value` is the JSON serialization —
                // `Value::String("foo")` displays as `"\"foo\""`,
                // not `foo`. Route Value-shaped exprs through
                // `RubyToS::ruby_to_s` (defined in `runtime/rust/http.rs`)
                // so the interpolation matches Ruby's identity-on-String
                // semantics. The trait dispatches at compile time to the
                // right impl for `&str`/`String`/`&Value`, so emitting
                // `.ruby_to_s()` is safe even when the body-typer's
                // annotation imprecisely marks an actually-`&String`
                // closure param as Untyped.
                let arg = emit_string_interp_arg(expr);
                // Fire on Untyped / Record (method-style; those files
                // import `RubyToS`) and on other rust_value_shaped
                // types via UFCS so ActionController compiles without
                // the trait in scope. Missing-Ty Sends whose recv is
                // itself Untyped/Record still get method-style — the
                // body-typer doesn't always propagate the result Ty
                // through nested `value[key]` index Sends.
                match expr.ty.as_ref() {
                    Some(crate::ty::Ty::Untyped) | Some(crate::ty::Ty::Record { .. }) => {
                        args.push(format!("({arg}).ruby_to_s()"));
                    }
                    Some(ty)
                        if super::super::ty::rust_value_shaped(ty)
                            && matches!(
                                &*expr.node,
                                crate::expr::ExprNode::Var { .. }
                                    | crate::expr::ExprNode::Ivar { .. }
                            ) =>
                    {
                        args.push(format!(
                            "<serde_json::Value as crate::http::RubyToS>::ruby_to_s(&({arg}))"
                        ));
                    }
                    _ if expr_recv_is_value(expr) => {
                        args.push(format!("({arg}).ruby_to_s()"));
                    }
                    _ => args.push(arg),
                }
            }
        }
    }
    (fmt, args)
}

/// A sequence is emitted as Rust statements, which is valid in a method
/// body but not directly in a `format!`/`write!` argument position. Keep
/// its evaluation at the interpolation site and make it a block
/// expression so accumulator/capture setup runs exactly once and before
/// that value is formatted.
fn emit_string_interp_arg(expr: &Expr) -> String {
    let emitted = emit_expr(expr);
    if matches!(&*expr.node, ExprNode::Seq { .. }) {
        format!("{{\n{}\n}}", indent(&emitted, 1))
    } else {
        emitted
    }
}

/// Returns `true` when `expr` is an index/send into a recv whose
/// body-typer Ty is `Untyped`/`Record` — i.e. the result is `&Value`
/// at runtime even though the typing pass didn't propagate the
/// inner result Ty. Currently only catches `recv[key]` and
/// `recv.method()` shapes; deeper chains land here recursively
/// through the index recv. Unions / ParamValue are handled via
/// `rust_value_shaped` on `expr.ty` itself (UFCS), not this recv
/// walk — walking them here would wrap Hash/Session `.get()` Option
/// returns that rust-emit as `Option<T>`.
fn expr_recv_is_value(expr: &Expr) -> bool {
    use crate::expr::ExprNode;
    let recv_opt: Option<&Expr> = match &*expr.node {
        ExprNode::Send { recv: Some(r), .. } => Some(r),
        _ => None,
    };
    let Some(recv) = recv_opt else { return false };
    matches!(
        recv.ty.as_ref(),
        Some(crate::ty::Ty::Untyped) | Some(crate::ty::Ty::Record { .. })
    )
}

/// Primitive literal → Rust literal. `nil` → `None` so Option-typed
/// fields work; integer literals get the `_i64` suffix to commit to
/// the rust integer convention; floats get a `.0` to keep them
/// floating-typed when the value has no fractional part.
pub(crate) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => {
            // Tail `nil` in an untyped/Value-returning method is
            // `Value::Null`, not Option::None (`request_for_csrf`).
            if in_return_tail() {
                // Tail `nil` in a void method is the unit value; a bare
                // `None;` statement has no inferable type.
                if super::current_return_is_unit() {
                    return "()".to_string();
                }
                if let Some(ty) = current_return_ty() {
                    if super::super::ty::rust_value_shaped(&ty) {
                        return "serde_json::Value::Null".to_string();
                    }
                }
            }
            "None".to_string()
        }
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => format!("{value}_i64"),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        Literal::Sym { value } => format!("{:?}", value.as_str()),
        Literal::Regex { pattern, flags } => emit_regex_literal(pattern, flags),
    }
}

/// Expression-position `/pattern/flags` → `regex::Regex::new(...)`.
///
/// A comment placeholder is not an expression: Campfire's fresh emit
/// failed `cargo check` on `/* TODO rust2: Regex(...) */` sitting where
/// a value was required. The emitted crate already depends on `regex`
/// (see `CARGO_TOML_TEMPLATE`) and constant-position regexes already
/// lower to `regex::Regex::new` (`format_constant`). This is the same
/// lowering at expression position, so a literal used as a `gsub`
/// pattern is a real `Regex` rather than a hole.
///
/// Ruby's `i` and `x` match the crate flags. Ruby's `m` means dot-all,
/// so it maps to the crate's `s` (the crate's `m` changes anchor
/// behavior instead). Ruby's `o` is a compile-once hint with no pattern
/// meaning, and `e`/`s`/`n` encoding flags have no regex-crate equivalent;
/// those are reported and panic at the site rather than silently
/// changing the pattern. Rust regexes are Unicode by default, matching
/// Ruby's `u`; `(?u)` is retained for explicitness. An empty flag set
/// is a bare pattern: `(?)` is not a valid group.
fn emit_regex_literal(pattern: &str, flags: &str) -> String {
    let mut inline = String::new();
    for flag in flags.chars() {
        match flag {
            'i' | 'x' | 'u' => inline.push(flag),
            'm' => inline.push('s'),
            'o' => {}
            _ => {
                return crate::emit::diagnostics::report_unsupported(
                    crate::span::Span::synthetic(),
                    "rust",
                    "Regex",
                    format!(
                        "Ruby regex flag `{flag}` has no regex-crate equivalent; \
                         refusing to emit a pattern that would not match what Ruby matches"
                    ),
                );
            }
        }
    }
    let source = if inline.is_empty() {
        pattern.to_string()
    } else {
        format!("(?{inline}){pattern}")
    };
    format!("regex::Regex::new({source:?}).unwrap()")
}

#[cfg(test)]
mod tests {
    use super::{emit_closure, emit_literal, emit_string_interp};
    use crate::emit::rust::EmitCtx;
    use crate::emit::rust::expr::{declare_var, emit_expr, with_emit_ctx};
    use crate::expr::{BlockStyle, Expr, ExprNode, InterpPart, LValue, Literal};
    use crate::ident::{Symbol, VarId};
    use crate::span::Span;

    fn int_lit(n: i64) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: Literal::Int { value: n },
            },
        )
    }

    fn var(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Var {
                id: VarId(0),
                name: Symbol::from(name),
            },
        )
    }

    fn assign(name: &str, value: Expr) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Var {
                    id: VarId(0),
                    name: Symbol::from(name),
                },
                value,
            },
        )
    }

    fn lambda(params: &[&str], body: Expr) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lambda {
                extra_params: Vec::new(),
                rest_param: None,
                params: params.iter().copied().map(Symbol::from).collect(),
                block_param: None,
                body,
                block_style: BlockStyle::Brace,
            },
        )
    }

    fn seq(exprs: Vec<Expr>) -> Expr {
        Expr::new(Span::synthetic(), ExprNode::Seq { exprs })
    }

    fn bare_rebinds(emitted: &str, name: &str) -> bool {
        emitted
            .replace(&format!("let mut {name}"), "")
            .replace(&format!("let {name}"), "")
            .contains(&format!("{name} ="))
    }

    /// Nested view captures (`message_area_tag` wrapping `messages_tag`)
    /// assign `_cap` outside the closure and `_cap2` inside it. The
    /// closure body must declare `_cap2`; inheriting the outer
    /// declared-var set emitted `_cap2 = …` with no `let`.
    #[test]
    fn nested_capture_assigns_declare_in_the_closure() {
        let inner = lambda(
            &[],
            seq(vec![
                assign("_cap2", int_lit(2)),
                assign("_cap2", int_lit(3)),
            ]),
        );
        let body = seq(vec![assign("_cap", int_lit(1)), inner]);
        let emitted = with_emit_ctx(EmitCtx::default(), || emit_expr(&body));
        assert!(
            emitted.contains("let _cap = 1_i64"),
            "outer capture declares:\n{emitted}"
        );
        assert!(
            emitted.contains("let mut _cap2 = 2_i64"),
            "nested capture declares inside the closure:\n{emitted}"
        );
        assert!(
            emitted.contains("_cap2 = 3_i64"),
            "second accumulator write rebinds the closure-local:\n{emitted}"
        );
    }

    /// Two levels of nesting (`_cap` / `_cap2` / `_cap3`) each get their
    /// own `let`. The middle closure's declaration must not be visible
    /// to the inner one, and neither must leak to the method body.
    #[test]
    fn doubly_nested_captures_each_declare() {
        let inner = lambda(&[], assign("_cap3", int_lit(3)));
        let middle = lambda(&[], seq(vec![assign("_cap2", int_lit(2)), inner]));
        let body = seq(vec![assign("_cap", int_lit(1)), middle]);
        let emitted = with_emit_ctx(EmitCtx::default(), || emit_expr(&body));
        for name in ["_cap", "_cap2", "_cap3"] {
            assert!(
                emitted.contains(&format!("let {name} =")),
                "{name} must be declared:\n{emitted}"
            );
            assert!(
                !bare_rebinds(&emitted, name),
                "{name} must not also appear as a bare rebind:\n{emitted}"
            );
        }
    }

    /// A capture that the closure itself declared still rebinds. The
    /// snapshot must keep outer declarations, not wipe the set.
    #[test]
    fn closure_rebinds_a_capture_it_declared() {
        let body = seq(vec![
            assign("_cap", int_lit(1)),
            assign("_cap", var("_cap")),
            assign("_cap", int_lit(2)),
        ]);
        let emitted = with_emit_ctx(EmitCtx::default(), || emit_closure(&[], &body));
        assert!(
            emitted.contains("let mut _cap = 1_i64"),
            "first assign declares:\n{emitted}"
        );
        assert!(
            emitted.contains("_cap = _cap"),
            "second assign rebinds:\n{emitted}"
        );
        assert_eq!(
            emitted.matches("let mut _cap").count(),
            1,
            "must not redeclare the same closure-local:\n{emitted}"
        );
    }

    #[test]
    fn closure_parameters_escape_rust_keywords() {
        let emitted = with_emit_ctx(EmitCtx::default(), || {
            emit_closure(&[Symbol::from("match")], &int_lit(1))
        });
        assert!(emitted.starts_with("|r#match|"), "{emitted}");
    }

    /// An outer `let` must not leak a declaration *into* the closure,
    /// and a closure-local `let` must not leak back out.
    #[test]
    fn closure_declared_set_does_not_leak_either_way() {
        let inner = lambda(&[], assign("inner_only", int_lit(1)));
        let body = seq(vec![
            assign("outer_only", int_lit(0)),
            inner,
            assign("outer_only", int_lit(2)),
            assign("inner_only", int_lit(3)),
        ]);
        let emitted = with_emit_ctx(EmitCtx::default(), || {
            declare_var("unrelated".to_string());
            emit_expr(&body)
        });
        assert!(
            emitted.contains("let outer_only = 0_i64"),
            "outer first assign declares:\n{emitted}"
        );
        assert!(
            emitted.contains("outer_only = 2_i64") && !emitted.contains("let outer_only = 2_i64"),
            "outer rebind stays a rebind:\n{emitted}"
        );
        assert!(
            emitted.contains("let inner_only = 1_i64"),
            "closure declares its own local:\n{emitted}"
        );
        assert!(
            emitted.contains("let inner_only = 3_i64"),
            "same name after the closure is a fresh binding:\n{emitted}"
        );
    }

    #[test]
    fn regex_literal_is_a_regex_new_expression() {
        let bare = emit_literal(&Literal::Regex {
            pattern: r"\d+".to_string(),
            flags: String::new(),
        });
        assert_eq!(bare, r#"regex::Regex::new("\\d+").unwrap()"#);
        assert!(
            !bare.contains("TODO"),
            "must not emit a comment placeholder: {bare}"
        );

        let flagged = emit_literal(&Literal::Regex {
            pattern: "foo".to_string(),
            flags: "im".to_string(),
        });
        assert_eq!(flagged, r#"regex::Regex::new("(?is)foo").unwrap()"#);

        // `o` is compile-once, not a match flag. Dropping it keeps the
        // pattern Ruby would match; prefixing `(?o)` would not parse.
        let once = emit_literal(&Literal::Regex {
            pattern: "foo".to_string(),
            flags: "io".to_string(),
        });
        assert_eq!(once, r#"regex::Regex::new("(?i)foo").unwrap()"#);
    }

    #[test]
    fn regex_encoding_flag_is_an_explicit_unsupported_diagnostic() {
        for flag in ["e", "s", "n"] {
            let (emitted, diags) = crate::emit::diagnostics::scope(|| {
                emit_literal(&Literal::Regex {
                    pattern: "foo".to_string(),
                    flags: flag.to_string(),
                })
            });
            assert!(
                emitted.starts_with("panic!"),
                "encoding flag `{flag}` must not become a Regex::new of a wrong pattern: {emitted}"
            );
            assert!(
                !emitted.contains("foo"),
                "must not substitute or keep a silently wrong pattern: {emitted}"
            );
            assert!(
                diags.iter().any(|d| d.message.contains("not supported")),
                "expected unsupported diagnostic for `{flag}`, got {diags:?}"
            );
        }
    }

    #[test]
    fn string_interpolation_wraps_capture_sequence_as_block_expression() {
        let capture = seq(vec![assign("_cap", int_lit(1)), var("_cap")]);
        let emitted = with_emit_ctx(EmitCtx::default(), || {
            emit_string_interp(&[InterpPart::Expr { expr: capture }])
        });

        assert!(
            emitted.contains("format!(\"{}\", {\n    let _cap = 1_i64;\n    _cap\n})"),
            "sequence is a single valid format argument:\n{emitted}"
        );
        assert!(!emitted.contains(", let _cap"), "{emitted}");
    }
}
