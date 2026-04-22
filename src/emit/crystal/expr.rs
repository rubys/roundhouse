//! Generic Crystal body/expression emission — used by the model
//! method emitter and by other modules that need a fallback for
//! arbitrary `Expr` rendering.

use crate::expr::{Expr, ExprNode, LValue, Literal};

// Bodies + expressions -------------------------------------------------

pub(super) fn emit_body(body: &Expr) -> String {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = Vec::new();
            for (i, e) in exprs.iter().enumerate() {
                if i > 0 && e.leading_blank_line {
                    lines.push(String::new());
                }
                lines.push(emit_stmt(e));
            }
            lines.join("\n")
        }
        _ => emit_stmt(body),
    }
}

pub(super) fn emit_stmt(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("{} = {}", name, emit_expr(value))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("@{} = {}", name, emit_expr(value))
        }
        _ => emit_expr(e),
    }
}

pub(super) fn emit_expr(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::")
        }
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => format!("@{name}"),
        ExprNode::Send { recv, method, args, .. } => {
            emit_send(recv.as_ref(), method.as_str(), args)
        }
        ExprNode::Assign { target: _, value } => emit_expr(value),
        ExprNode::Seq { exprs } => {
            exprs.iter().map(emit_expr).collect::<Vec<_>>().join("; ")
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            // Crystal ternary `cond ? a : b` (same syntax as Ruby).
            // emit_expr is always in expression position; the spec and
            // view emitters own statement-form If handling.
            let cond_s = emit_expr(cond);
            let then_s = emit_expr(then_branch);
            let else_s = emit_expr(else_branch);
            format!("{cond_s} ? {then_s} : {else_s}")
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::{BoolOpKind, BoolOpSurface};
            // Crystal supports both `&&` / `||` and `and` / `or` — we
            // preserve the surface form from the IR the same way the
            // Ruby emitter does.
            let op_s = match (op, &e.node) {
                (BoolOpKind::Or, _) => {
                    if let ExprNode::BoolOp { surface: BoolOpSurface::Word, .. } = &*e.node {
                        "or"
                    } else {
                        "||"
                    }
                }
                (BoolOpKind::And, _) => {
                    if let ExprNode::BoolOp { surface: BoolOpSurface::Word, .. } = &*e.node {
                        "and"
                    } else {
                        "&&"
                    }
                }
            };
            format!("{} {op_s} {}", emit_expr(left), emit_expr(right))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                        format!("{value}: {}", emit_expr(v))
                    } else {
                        format!("{} => {}", emit_expr(k), emit_expr(v))
                    }
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        ExprNode::StringInterp { parts } => {
            // Crystal interpolation is identical to Ruby's.
            use crate::expr::InterpPart;
            let mut out = String::from("\"");
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            match c {
                                '"' => out.push_str("\\\""),
                                '\\' => out.push_str("\\\\"),
                                '\n' => out.push_str("\\n"),
                                '#' => out.push_str("\\#"),
                                other => out.push(other),
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        out.push_str("#{");
                        out.push_str(&emit_expr(expr));
                        out.push('}');
                    }
                }
            }
            out.push('"');
            out
        }
        ExprNode::Yield { args } => {
            let parts: Vec<String> = args.iter().map(emit_expr).collect();
            if parts.is_empty() {
                "yield".to_string()
            } else {
                format!("yield {}", parts.join(", "))
            }
        }
        other => format!("# TODO: emit {:?}", std::mem::discriminant(other)),
    }
}

pub(super) fn emit_send(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    if method == "[]" && recv.is_some() {
        return format!("{}[{}]", emit_expr(recv.unwrap()), args_s.join(", "));
    }
    // Ruby's binary operators ride the Send channel. Crystal's syntax
    // matches Ruby's for these, so emit infix directly.
    if let (Some(r), [arg]) = (recv, args) {
        if is_cr_binop(method) {
            return format!("{} {method} {}", emit_expr(r), emit_expr(arg));
        }
    }
    match recv {
        None => {
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{}({})", method, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_expr(r);
            if args_s.is_empty() {
                format!("{recv_s}.{method}")
            } else {
                format!("{recv_s}.{method}({})", args_s.join(", "))
            }
        }
    }
}

fn is_cr_binop(method: &str) -> bool {
    matches!(
        method,
        "==" | "!="
            | "<"
            | "<="
            | ">"
            | ">="
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "**"
            | "<<"
            | ">>"
            | "|"
            | "&"
            | "^"
    )
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
        // Crystal has first-class symbols just like Ruby.
        Literal::Sym { value } => format!(":{}", value.as_str()),
    }
}
