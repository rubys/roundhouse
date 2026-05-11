//! `rust2` expression emit — `Expr` IR → Rust source-text.
//!
//! Phase 2.1 scope: minimal handling for the inflector body shape
//! (Lit, Var, Send `==`, StringInterp, If). Extended file-by-file
//! through Phase 2 as each runtime file forces new IR shapes.

use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal};

pub(super) fn emit_expr(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => name.as_str().to_string(),
        ExprNode::Ivar { name } => format!("self.{name}"),
        ExprNode::SelfRef => "self".to_string(),
        ExprNode::Const { path } => path
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join("::"),
        ExprNode::StringInterp { parts } => emit_string_interp(parts),
        ExprNode::If { cond, then_branch, else_branch } => {
            // Ruby `cond ? a : b` and `if cond; a; else b; end` both
            // lower to `ExprNode::If`. Rust uses `if/else` as an
            // expression in both forms — same emit shape covers both.
            format!(
                "if {} {{ {} }} else {{ {} }}",
                emit_expr(cond),
                emit_expr(then_branch),
                emit_expr(else_branch),
            )
        }
        ExprNode::Send { recv, method, args, .. } => emit_send(recv.as_ref(), method.as_str(), args),
        ExprNode::Seq { exprs } => {
            // Rust statements are `;`-terminated; the last expression
            // is the block's value (no trailing `;`). Multi-statement
            // method bodies render natural Rust shape this way.
            let mut lines = Vec::with_capacity(exprs.len());
            let last = exprs.len().saturating_sub(1);
            for (i, e) in exprs.iter().enumerate() {
                let s = emit_expr(e);
                if i == last {
                    lines.push(s);
                } else {
                    lines.push(format!("{s};"));
                }
            }
            lines.join("\n")
        }
        ExprNode::Assign { target, value } => emit_assign(target, value),
        // Catch-all for IR shapes not yet implemented. Each new runtime
        // file in Phase 2 expands this until full coverage.
        other => format!("/* TODO rust2: ExprNode::{:?} */", std::mem::discriminant(other)),
    }
}

fn emit_assign(target: &LValue, value: &Expr) -> String {
    let rhs = emit_expr(value);
    match target {
        LValue::Var { name, .. } => format!("let {} = {rhs}", name.as_str()),
        LValue::Ivar { name } => format!("self.{name} = {rhs}"),
        LValue::Attr { recv, name } => format!("{}.{name} = {rhs}", emit_expr(recv)),
        LValue::Index { recv, index } => {
            format!("{}[{}] = {rhs}", emit_expr(recv), emit_expr(index))
        }
    }
}

fn emit_send(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    // Binary operators (==, !=, <, >, +, -, *, /) ingest as Send
    // with `method` as the operator name. Ruby `a == b` lowers to
    // `Send { recv: a, method: ==, args: [b] }`.
    if matches!(method, "==" | "!=" | "<" | ">" | "<=" | ">=" | "+" | "-" | "*" | "/")
        && recv.is_some()
        && args.len() == 1
    {
        return format!("{} {} {}", emit_expr(recv.unwrap()), method, emit_expr(&args[0]));
    }
    // Free functions / module functions (Inflector.pluralize → bare
    // pluralize() in the inflector module). Implicit-self bare calls
    // emit as bare function calls.
    if recv.is_none() {
        let args_s: Vec<String> = args.iter().map(emit_expr).collect();
        return format!("{}({})", method, args_s.join(", "));
    }
    // Method calls — `recv.method(args...)`. Phase 2.1 stub form;
    // later phases add Ruby→Rust stdlib bridges (start_with? →
    // starts_with, fetch → get, etc.).
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    format!("{}.{}({})", emit_expr(recv.unwrap()), method, args_s.join(", "))
}

fn emit_string_interp(parts: &[InterpPart]) -> String {
    // Rust `format!` macro is the natural interp target.
    // Lift literal text into the format string (escaping `{`/`}`),
    // each `#{expr}` becomes a `{}` placeholder + an arg.
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

pub(super) fn emit_literal(lit: &Literal) -> String {
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
