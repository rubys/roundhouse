//! Generic Go body/expression emission — used by the model method
//! emitter and by other modules that need a fallback for arbitrary
//! `Expr` rendering.
//!
//! Forked 2026-05-21 from `src/emit/go/expr.rs` so go2 can evolve
//! the walker independently (Phase 2+ type-aware emit, lowered-IR
//! coverage, transpiled-runtime call shapes) without dragging
//! legacy go regressions.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ty::Ty;

// Reused verbatim from legacy go until go2 needs its own dispatch.
use crate::emit::go::shared::go_method_name;

pub(super) fn emit_expr(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Send { recv, method, args, .. } => emit_send(recv.as_ref(), method.as_str(), args),
        ExprNode::Assign { target: _, value } => emit_expr(value),
        ExprNode::Seq { exprs } => {
            exprs.iter().map(emit_expr).collect::<Vec<_>>().join("; ")
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = emit_expr(cond);
            let then_s = emit_block_body(then_branch);
            let else_s = emit_block_body(else_branch);
            format!("if {cond_s} {{\n{then_s}\n}} else {{\n{else_s}\n}}")
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}: {}", emit_expr(k), emit_expr(v)))
                .collect();
            format!("map[string]interface{{}}{{{}}}", parts.join(", "))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[]interface{{}}{{{}}}", parts.join(", "))
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "||",
                BoolOpKind::And => "&&",
            };
            format!("{} {} {}", emit_expr(left), op_s, emit_expr(right))
        }
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
            let mut fmt = String::new();
            let mut args: Vec<String> = Vec::new();
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            if c == '%' {
                                fmt.push_str("%%");
                            } else {
                                fmt.push(c);
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        fmt.push_str("%v");
                        args.push(emit_expr(expr));
                    }
                }
            }
            if args.is_empty() {
                format!("{fmt:?}")
            } else {
                format!("fmt.Sprintf({fmt:?}, {})", args.join(", "))
            }
        }
        other => format!("/* TODO: emit {:?} */", std::mem::discriminant(other)),
    }
}

pub(super) fn emit_send(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();

    if method == "[]" && recv.is_some() {
        return format!("{}[{}]", emit_expr(recv.unwrap()), args_s.join(", "));
    }

    // Binary operators: Ruby parses `a == b`, `a + b`, etc. as
    // `Send { recv: a, method: "==", args: [b] }`. Emit them infix.
    if let (Some(r), Some(op)) = (recv, binary_op(method)) {
        if args.len() == 1 {
            return format!("{} {} {}", emit_expr(r), op, args_s[0]);
        }
    }

    // Ruby→Go method-name mapping for string operations that have no
    // 1:1 in Go's stdlib (`strip` is `strings.TrimSpace(…)`, not
    // `.Strip()`). Only kicks in for instance dispatch on Str-typed
    // receivers; class calls and unknown types pass through.
    if let Some(r) = recv {
        if args.is_empty() && matches!(r.ty, Some(Ty::Str)) {
            if let Some(wrapped) = map_go_str_method(method, &emit_expr(r)) {
                return wrapped;
            }
        }
    }

    let go_m = go_method_name(method);
    match recv {
        None => {
            if args_s.is_empty() {
                go_m
            } else {
                format!("{}({})", go_m, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_expr(r);
            // Struct field access vs method call: 0-arg Sends on a
            // non-Class receiver whose method isn't a known AR/stdlib
            // call render without parens (`p.Title`, not `p.Title()`).
            let is_class_call = matches!(&*r.node, ExprNode::Const { .. });
            if !is_class_call && args_s.is_empty() && !is_known_go_method(method) {
                return format!("{recv_s}.{go_m}");
            }
            format!("{}.{}({})", recv_s, go_m, args_s.join(", "))
        }
    }
}

/// Ruby method names that map to Go binary operators when called
/// with a receiver and one argument. `nil` semantics for `==` differ
/// (Go nil interface vs typed nil) but that's a downstream concern —
/// at this level we just rewrite the call shape.
fn binary_op(method: &str) -> Option<&'static str> {
    Some(match method {
        "==" => "==",
        "!=" => "!=",
        "<" => "<",
        "<=" => "<=",
        ">" => ">",
        ">=" => ">=",
        "+" => "+",
        "-" => "-",
        "*" => "*",
        "/" => "/",
        "%" => "%",
        _ => return None,
    })
}

/// AR/stdlib method names that should emit with parens on a model
/// struct receiver. Everything else on a non-Class receiver with no
/// args is treated as a field read. Grows alongside the runtime.
fn is_known_go_method(name: &str) -> bool {
    matches!(
        name,
        "save" | "save!" | "destroy" | "destroy!" | "update" | "update!"
            | "delete" | "touch" | "reload"
            | "validate" | "attributes" | "errors"
    )
}

/// Map Ruby String methods onto Go expressions that compile. `strip`
/// in Ruby is `strings.TrimSpace(s)` in Go — no method form exists.
/// Returns `Some(emit_text)` for a handled method. Unhandled methods
/// fall through to the default `.Method()` emit which may or may not
/// compile depending on the target receiver's actual methods.
fn map_go_str_method(method: &str, recv_text: &str) -> Option<String> {
    match method {
        "strip" => Some(format!("strings.TrimSpace({recv_text})")),
        "upcase" => Some(format!("strings.ToUpper({recv_text})")),
        "downcase" => Some(format!("strings.ToLower({recv_text})")),
        _ => None,
    }
}

pub(super) fn emit_block_body(e: &Expr) -> String {
    let raw = match &*e.node {
        ExprNode::Seq { exprs } => exprs
            .iter()
            .map(emit_expr)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => emit_expr(e),
    };
    raw.lines().map(|l| format!("\t{l}")).collect::<Vec<_>>().join("\n")
}

pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "nil".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        Literal::Sym { value } => format!("{:?}", value.as_str()),
        Literal::Regex { pattern, flags } => {
            format!("regexp.MustCompile({:?})", format!("(?{flags}){pattern}"))
        }
    }
}

/// Emit `expr` at body (return) position — Ruby's last-expression
/// semantics mapped to Go's explicit `return`. Recurses into `If`
/// and `Seq` so the return lands at the value-producing leaf. All
/// other variants emit as `return <value_expression>`.
///
/// Output is indented one tab in (caller wraps in `func ... { ... }`).
pub(super) fn emit_return_body(e: &Expr) -> String {
    let mut out = String::new();
    emit_return_at(e, &mut out, 1);
    out
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push('\t');
    }
}

fn emit_return_at(e: &Expr, out: &mut String, depth: usize) {
    match &*e.node {
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = emit_expr(cond);
            indent(out, depth);
            out.push_str(&format!("if {cond_s} {{\n"));
            emit_return_at(then_branch, out, depth + 1);
            indent(out, depth);
            out.push_str("} else {\n");
            emit_return_at(else_branch, out, depth + 1);
            indent(out, depth);
            out.push_str("}\n");
        }
        ExprNode::Seq { exprs } => {
            // All but the last are statements (effects-only); last is
            // the return-position expression.
            for (i, sub) in exprs.iter().enumerate() {
                if i + 1 == exprs.len() {
                    emit_return_at(sub, out, depth);
                } else {
                    indent(out, depth);
                    out.push_str(&emit_expr(sub));
                    out.push('\n');
                }
            }
        }
        _ => {
            let v = emit_expr(e);
            indent(out, depth);
            out.push_str(&format!("return {v}\n"));
        }
    }
}
