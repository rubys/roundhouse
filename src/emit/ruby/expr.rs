//! Expression emission: the per-AST-node converter for Ruby.
//!
//! `emit_expr` is the entry; `emit_node` dispatches on `ExprNode`. Helpers
//! for arrays, hashes, sends, blocks, literals, lvalues, patterns, and
//! match arms live here too.

use crate::expr::{Arm, Expr, ExprNode, LValue, Literal, Pattern};
use crate::ident::Symbol;

use super::shared::indent_lines;

pub fn emit_expr(e: &Expr) -> String {
    emit_node(&e.node)
}

fn emit_node(n: &ExprNode) -> String {
    match n {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => format!("@{name}"),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::")
        }
        ExprNode::Hash { entries, braced } => emit_hash(entries, *braced),
        ExprNode::Array { elements, style } => emit_array(elements, style),
        ExprNode::StringInterp { parts } => emit_string_interp(parts),
        ExprNode::BoolOp { op, surface, left, right } => {
            emit_bool_op(*op, *surface, left, right)
        }
        ExprNode::Let { name, value, body, .. } => {
            format!("{name} = {}\n{}", emit_expr(value), emit_expr(body))
        }
        ExprNode::Lambda { params, block_param, body, .. } => {
            let mut ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
            if let Some(b) = block_param { ps.push(format!("&{b}")); }
            if ps.is_empty() {
                format!("-> {{ {} }}", emit_expr(body))
            } else {
                format!("->({}) {{ {} }}", ps.join(", "), emit_expr(body))
            }
        }
        ExprNode::Apply { fun, args, block } => {
            let args_s: Vec<String> = args.iter().map(emit_expr).collect();
            let base = format!("{}.call({})", emit_expr(fun), args_s.join(", "));
            if let Some(b) = block { format!("{base} {{ {} }}", emit_expr(b)) } else { base }
        }
        ExprNode::Send { recv, method, args, block, parenthesized } => {
            let base = emit_send_base(recv.as_ref(), method, args, *parenthesized);
            match block {
                None => base,
                Some(b) => emit_do_block(&base, b),
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            format!(
                "if {}\n{}\nelse\n{}\nend",
                emit_expr(cond),
                indent_lines(&emit_expr(then_branch), 1),
                indent_lines(&emit_expr(else_branch), 1),
            )
        }
        ExprNode::Case { scrutinee, arms } => {
            let mut s = format!("case {}\n", emit_expr(scrutinee));
            for arm in arms {
                s.push_str(&emit_arm(arm));
            }
            s.push_str("end");
            s
        }
        ExprNode::Seq { exprs } => {
            let mut out = String::new();
            for (i, e) in exprs.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                    if e.leading_blank_line {
                        out.push('\n');
                    }
                }
                out.push_str(&emit_expr(e));
            }
            out
        }
        ExprNode::Assign { target, value } => {
            format!("{} = {}", emit_lvalue(target), emit_expr(value))
        }
        ExprNode::Yield { args } => {
            let args_s: Vec<String> = args.iter().map(emit_expr).collect();
            if args_s.is_empty() { "yield".to_string() } else { format!("yield {}", args_s.join(", ")) }
        }
        ExprNode::Raise { value } => format!("raise {}", emit_expr(value)),
        ExprNode::RescueModifier { expr, fallback } => {
            format!("{} rescue {}", emit_expr(expr), emit_expr(fallback))
        }
    }
}

fn emit_bool_op(
    op: crate::expr::BoolOpKind,
    surface: crate::expr::BoolOpSurface,
    left: &Expr,
    right: &Expr,
) -> String {
    use crate::expr::{BoolOpKind, BoolOpSurface};
    let op_s = match (op, surface) {
        (BoolOpKind::Or, BoolOpSurface::Symbol) => "||",
        (BoolOpKind::Or, BoolOpSurface::Word) => "or",
        (BoolOpKind::And, BoolOpSurface::Symbol) => "&&",
        (BoolOpKind::And, BoolOpSurface::Word) => "and",
    };
    format!("{} {} {}", emit_expr(left), op_s, emit_expr(right))
}

fn emit_string_interp(parts: &[crate::expr::InterpPart]) -> String {
    use crate::expr::InterpPart;
    let mut out = String::with_capacity(2);
    out.push('"');
    for p in parts {
        match p {
            InterpPart::Text { value } => {
                for c in value.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
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

fn emit_array(elements: &[Expr], style: &crate::expr::ArrayStyle) -> String {
    use crate::expr::ArrayStyle;
    match style {
        ArrayStyle::Brackets => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[{}]", parts.join(", "))
        }
        ArrayStyle::BracketsSpaced => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            if parts.is_empty() {
                "[]".to_string()
            } else {
                format!("[ {} ]", parts.join(", "))
            }
        }
        ArrayStyle::PercentI => {
            // Symbol list: elements must be symbol literals. Emit bare names
            // without the leading `:` and space-separate.
            let parts: Vec<String> = elements
                .iter()
                .map(|e| match &*e.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => value.to_string(),
                    _ => emit_expr(e),
                })
                .collect();
            format!("%i[{}]", parts.join(" "))
        }
        ArrayStyle::PercentW => {
            // Word list: elements must be string literals. Emit without quotes.
            let parts: Vec<String> = elements
                .iter()
                .map(|e| match &*e.node {
                    ExprNode::Lit { value: Literal::Str { value } } => value.to_string(),
                    _ => emit_expr(e),
                })
                .collect();
            format!("%w[{}]", parts.join(" "))
        }
    }
}

fn emit_hash(entries: &[(Expr, Expr)], braced: bool) -> String {
    let parts: Vec<String> = entries
        .iter()
        .map(|(k, v)| {
            // Rails-idiomatic shorthand `key: value` when key is a symbol
            // literal. Bare shorthand requires a simple identifier; symbols
            // with special characters (e.g. `"turbo_confirm"`, `"text-sm"`)
            // use the quoted-key form `"name": value`. Rocket `k => v`
            // falls through for non-symbol keys.
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                let name = value.as_str();
                if is_simple_ident(name) {
                    format!("{name}: {}", emit_expr(v))
                } else {
                    format!("{:?}: {}", name, emit_expr(v))
                }
            } else {
                format!("{} => {}", emit_expr(k), emit_expr(v))
            }
        })
        .collect();
    if braced {
        format!("{{ {} }}", parts.join(", "))
    } else {
        parts.join(", ")
    }
}

/// Can `s` appear as a bareword hash key (`s: value`)? The bareword form
/// requires a `[A-Za-z_][A-Za-z0-9_]*` identifier, optionally ending in
/// `?`, `!`, or `=`. Anything else (hyphens, spaces, colons, digits-first)
/// must be quoted: `"s": value`.
fn is_simple_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    let mut saw_suffix = false;
    for c in chars {
        if saw_suffix {
            return false;
        }
        if c.is_ascii_alphanumeric() || c == '_' {
            continue;
        }
        if matches!(c, '?' | '!' | '=') {
            saw_suffix = true;
            continue;
        }
        return false;
    }
    true
}

/// Emit the receiver/method/args portion of a Send without its block.
/// Used by normal Ruby emission and by ERB template reconstruction.
pub(super) fn emit_send_base(
    recv: Option<&Expr>,
    method: &Symbol,
    args: &[Expr],
    parenthesized: bool,
) -> String {
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    match (recv, method.as_str()) {
        (Some(r), "[]") => format!("{}[{}]", emit_expr(r), args_s.join(", ")),
        (None, _) => {
            if args_s.is_empty() {
                method.to_string()
            } else if parenthesized {
                format!("{method}({})", args_s.join(", "))
            } else {
                format!("{method} {}", args_s.join(", "))
            }
        }
        (Some(r), _) => {
            let recv_s = emit_expr(r);
            if args_s.is_empty() {
                format!("{recv_s}.{method}")
            } else if parenthesized {
                format!("{recv_s}.{method}({})", args_s.join(", "))
            } else {
                format!("{recv_s}.{method} {}", args_s.join(", "))
            }
        }
    }
}

/// Emit a `Send + block` in plain Ruby form. Honors the Lambda's
/// `block_style` to pick `{ … }` vs `do … end`. `{ }` emits a single-line
/// body; `do … end` spans multiple lines when the body has newlines.
pub(super) fn emit_do_block(base: &str, block: &Expr) -> String {
    use crate::expr::BlockStyle;
    let ExprNode::Lambda { params, body, block_style, .. } = &*block.node else {
        return format!("{base} {{ {} }}", emit_expr(block));
    };
    let body_str = emit_expr(body);
    let params_str = if params.is_empty() {
        String::new()
    } else {
        let ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
        format!(" |{}|", ps.join(", "))
    };
    match block_style {
        BlockStyle::Brace => {
            // Single-line brace form — the common use for one-liner
            // callbacks and small block args.
            format!("{base} {{{params_str} {body_str} }}")
        }
        BlockStyle::Do => emit_do_form(base, &params_str, &body_str),
    }
}

fn emit_do_form(base: &str, params_str: &str, body_str: &str) -> String {
    let params_clause = if params_str.is_empty() {
        "do".to_string()
    } else {
        format!("do{params_str}")
    };
    if body_str.contains('\n') {
        format!(
            "{base} {}\n{}\nend",
            params_clause,
            indent_lines(&body_str, 1),
        )
    } else {
        format!("{base} {} {} end", params_clause, body_str)
    }
}

pub(super) fn emit_literal(l: &Literal) -> String {
    match l {
        Literal::Nil => "nil".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        Literal::Sym { value } => format!(":{value}"),
    }
}

fn emit_lvalue(lv: &LValue) -> String {
    match lv {
        LValue::Var { name, .. } => name.to_string(),
        LValue::Ivar { name } => format!("@{name}"),
        LValue::Attr { recv, name } => format!("{}.{name}", emit_expr(recv)),
        LValue::Index { recv, index } => format!("{}[{}]", emit_expr(recv), emit_expr(index)),
    }
}

fn emit_arm(arm: &Arm) -> String {
    let mut s = format!("when {}", emit_pattern(&arm.pattern));
    if let Some(g) = &arm.guard { s.push_str(&format!(" if {}", emit_expr(g))); }
    s.push('\n');
    s.push_str(&indent_lines(&emit_expr(&arm.body), 1));
    s.push('\n');
    s
}

fn emit_pattern(p: &Pattern) -> String {
    match p {
        Pattern::Wildcard => "_".to_string(),
        Pattern::Bind { name } => name.to_string(),
        Pattern::Lit { value } => emit_literal(value),
        Pattern::Array { elems, rest } => {
            let mut parts: Vec<String> = elems.iter().map(emit_pattern).collect();
            if let Some(r) = rest { parts.push(format!("*{r}")); }
            format!("[{}]", parts.join(", "))
        }
        Pattern::Record { fields, rest } => {
            let mut parts: Vec<String> = fields.iter()
                .map(|(k, v)| format!("{k}: {}", emit_pattern(v))).collect();
            if *rest { parts.push("**".into()); }
            format!("{{ {} }}", parts.join(", "))
        }
    }
}
