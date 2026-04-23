//! Generic TypeScript body / expression / literal emission. Used by
//! the standalone `emit_method` (runtime extraction) and indirectly by
//! controller / view / model / spec emitters that fall back to
//! arbitrary `Expr` rendering.

use super::naming::{ts_field_name, ts_method_name};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ty::Ty;

// Body + expressions ---------------------------------------------------

pub(super) fn emit_body(body: &Expr, return_ty: &Ty) -> String {
    let is_void = matches!(return_ty, Ty::Nil);
    match &*body.node {
        ExprNode::Assign { target: LValue::Ivar { .. }, value } => {
            if is_void {
                format!("{};", emit_expr(value))
            } else {
                format!("return {};", emit_expr(value))
            }
        }
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = Vec::new();
            for (i, e) in exprs.iter().enumerate() {
                lines.push(emit_stmt(e, i == exprs.len() - 1, is_void));
            }
            lines.join("\n")
        }
        _ => {
            if is_void {
                format!("{};", emit_expr(body))
            } else {
                format!("return {};", emit_expr(body))
            }
        }
    }
}

fn emit_stmt(e: &Expr, is_last: bool, void_return: bool) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("const {} = {};", name, emit_expr(value))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("this.{} = {};", ts_field_name(name.as_str()), emit_expr(value))
        }
        _ => {
            if is_last && !void_return {
                format!("return {};", emit_expr(e))
            } else {
                format!("{};", emit_expr(e))
            }
        }
    }
}

pub(super) fn emit_expr(e: &Expr) -> String {
    // Analyzer-set diagnostic annotations short-circuit to a target
    // raise-equivalent (preserves Ruby's runtime-raise semantics).
    if e.diagnostic.is_some() {
        return r#"(() => { throw new Error("roundhouse: + with incompatible operand types"); })()"#.to_string();
    }
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => format!("this.{}", ts_field_name(name.as_str())),
        ExprNode::Send { recv, method, args, parenthesized, .. } => {
            emit_send_with_parens(recv.as_ref(), method.as_str(), args, *parenthesized)
        }
        ExprNode::Assign { target: _, value } => emit_expr(value),
        ExprNode::Seq { exprs } => {
            exprs.iter().map(emit_expr).collect::<Vec<_>>().join("; ")
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            // TS ternary `cond ? a : b`. `emit_expr` is always called in
            // an expression position; controller/view emitters have
            // their own statement-form If handlers.
            let cond_s = emit_expr(cond);
            let then_s = emit_expr(then_branch);
            let else_s = emit_expr(else_branch);
            format!("{cond_s} ? {then_s} : {else_s}")
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "||",
                BoolOpKind::And => "&&",
            };
            format!("{} {} {}", emit_expr(left), op_s, emit_expr(right))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}: {}", emit_expr(k), emit_expr(v)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
            let mut out = String::from("`");
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            if c == '`' || c == '\\' {
                                out.push('\\');
                                out.push(c);
                            } else if c == '$' {
                                out.push_str("\\$");
                            } else {
                                out.push(c);
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        out.push_str("${");
                        out.push_str(&emit_expr(expr));
                        out.push('}');
                    }
                }
            }
            out.push('`');
            out
        }
        other => format!("/* TODO: emit {:?} */", std::mem::discriminant(other)),
    }
}

/// Core send emission. `parenthesized` reflects whether the Ruby
/// source wrapped args in explicit parens — for 0-arg explicit-
/// receiver calls we use it to decide between `recv.name` (Ruby
/// reader convention, JS property access) and `recv.name()` (method
/// call). Always emits parens when args are present.
pub(super) fn emit_send_with_parens(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    parenthesized: bool,
) -> String {
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    if method == "[]" && recv.is_some() {
        return format!("{}[{}]", emit_expr(recv.unwrap()), args_s.join(", "));
    }
    // Ruby's binary operators ride the Send channel. TS needs infix;
    // `==` and `!=` map to strict `===` / `!==` so equality semantics
    // match Ruby (Ruby has no implicit type coercion).
    if let (Some(r), [arg]) = (recv, args) {
        // `+` dispatch: TS's native `+` handles numeric and string;
        // Array concat wants spread. Incompatible pairs refuse.
        if method == "+" {
            use crate::emit::shared::add::{classify_add, AddCase};
            match classify_add(r, arg) {
                AddCase::ArrayConcat { .. } => {
                    return format!("[...{}, ...{}]", emit_expr(r), emit_expr(arg));
                }
                AddCase::Incompatible => {
                    // Emit a runtime throw via IIFE; `throw` is a
                    // statement in JS/TS so wrapping is required to
                    // keep the form expression-valued.
                    return r#"(() => { throw new Error("roundhouse: + with incompatible operand types"); })()"#.to_string();
                }
                _ => {}
            }
        }
        if let Some(op) = ts_binop(method) {
            return format!("{} {op} {}", emit_expr(r), emit_expr(arg));
        }
    }
    // Ruby stdlib method → TS equivalent, when the Ruby name collides
    // with a nonexistent TS property. Keyed on name only today; a
    // receiver-typed dispatch would replace this when per-type
    // mappings diverge.
    let (mapped_name, force_parens) = match method {
        "strip" => ("trim", true),
        _ => (method, false),
    };
    let ts_m = ts_method_name(mapped_name);
    match recv {
        None => {
            if args_s.is_empty() {
                ts_m
            } else {
                format!("{}({})", ts_m, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_expr(r);
            if args_s.is_empty() && !parenthesized && !force_parens {
                // Ruby's `obj.name` without parens is typically a
                // reader; Juntos mirrors that with a property
                // accessor / getter, so emit without parens.
                format!("{recv_s}.{ts_m}")
            } else {
                format!("{recv_s}.{ts_m}({})", args_s.join(", "))
            }
        }
    }
}

pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "null".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        // Ruby symbols map to string literals — the typed analyzer may
        // refine this into a discriminated-union enum later, but for
        // the scaffold a string is unambiguous and round-trips through
        // comparison as expected.
        Literal::Sym { value } => format!("{:?}", value.as_str()),
    }
}

fn ts_binop(method: &str) -> Option<&'static str> {
    Some(match method {
        "==" => "===",
        "!=" => "!==",
        "<" => "<",
        "<=" => "<=",
        ">" => ">",
        ">=" => ">=",
        "+" => "+",
        "-" => "-",
        "*" => "*",
        "/" => "/",
        "%" => "%",
        "**" => "**",
        "<<" => "<<",
        ">>" => ">>",
        "|" => "|",
        "&" => "&",
        "^" => "^",
        _ => return None,
    })
}
