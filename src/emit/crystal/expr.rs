//! Expression emission: per-AST-node converter for Crystal.
//!
//! Mirrors `src/emit/ruby/expr.rs` because Crystal's surface syntax
//! is Ruby-flavored (def/end, do |x|, string interp, blocks). The
//! divergences are localized:
//!   - `Lambda` needs typed params (Crystal Procs are typed); emit
//!     a closure form or fall back to a stub when types are missing.
//!   - Hash literals: emit the same shorthand (`key: val`) Spinel
//!     emits — Crystal accepts that as NamedTuple syntax, which works
//!     when helpers take `**opts`.
//!   - `Pattern` / `Case in` Ruby-3-style pattern matching maps to
//!     Crystal's narrower `case when` semantics; we reuse `case when`
//!     for the simple shapes the lowerer produces today.

use crate::expr::{Arm, Expr, ExprNode, InterpPart, LValue, Literal, Pattern};
use crate::ident::Symbol;

use super::shared::{escape_ident, indent_lines};

pub fn emit_expr(e: &Expr) -> String {
    emit_node(&e.node)
}

/// Public entry point used by `runtime_loader::crystal_units` for
/// module-level constant initializers (`HTML_ESCAPES = { ... }.freeze`
/// in view_helpers.rb, etc.). Same renderer; the function-typed
/// alias is a stable hook the loader plugs into `TargetEmit`.
pub fn emit_expr_for_runtime(e: &Expr) -> String {
    emit_expr(e)
}

fn is_empty_branch(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Seq { exprs } if exprs.is_empty())
        || matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
}

fn emit_node(n: &ExprNode) -> String {
    match n {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => escape_ident(name.as_str()),
        ExprNode::Ivar { name } => format!("@{name}"),
        ExprNode::SelfRef => "self".to_string(),
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
            // Crystal procs require typed params. The lowered IR
            // doesn't always carry param types here, so fall back to
            // a block-param closure form (`->{ body }`) when params
            // are empty. With params, emit untyped `->(p) { body }`
            // and rely on Crystal's inference / context.
            let mut ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
            if let Some(b) = block_param {
                ps.push(format!("&{b}"));
            }
            if ps.is_empty() {
                format!("-> {{ {} }}", emit_expr(body))
            } else {
                format!("->({}) {{ {} }}", ps.join(", "), emit_expr(body))
            }
        }
        ExprNode::Apply { fun, args, block } => {
            let args_s: Vec<String> = args.iter().map(emit_expr).collect();
            let base = format!("{}.call({})", emit_expr(fun), args_s.join(", "));
            if let Some(b) = block {
                format!("{base} {{ {} }}", emit_expr(b))
            } else {
                base
            }
        }
        ExprNode::Send { recv, method, args, block, parenthesized } => {
            // `require "x"` calls in Ruby method bodies are loadlate
            // imports — Ruby allows them anywhere, Crystal rejects
            // them outside file scope. Skip the call entirely; the
            // emitted Crystal file's top-level `require` statements
            // (or stdlib auto-load for Base64/JSON) handle the
            // semantic. Emits a comment so the diff stays auditable.
            if recv.is_none()
                && method.as_str() == "require"
                && args.len() == 1
                && matches!(
                    &*args[0].node,
                    ExprNode::Lit { value: Literal::Str { .. } }
                )
            {
                return format!("# Crystal: {} (skipped — module load handled at file scope)", emit_send_base(recv.as_ref(), method, args, *parenthesized));
            }
            let base = emit_send_base(recv.as_ref(), method, args, *parenthesized);
            match block {
                None => base,
                Some(b) => emit_do_block(&base, b),
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = emit_expr(cond);
            let then_s = emit_expr(then_branch);
            let else_empty = is_empty_branch(else_branch);
            if else_empty
                && !matches!(&*then_branch.node, ExprNode::Seq { .. })
                && !then_s.contains('\n')
            {
                format!("{then_s} if {cond_s}")
            } else if else_empty {
                format!("if {cond_s}\n{}\nend", indent_lines(&then_s, 1))
            } else {
                format!(
                    "if {cond_s}\n{}\nelse\n{}\nend",
                    indent_lines(&then_s, 1),
                    indent_lines(&emit_expr(else_branch), 1),
                )
            }
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
            if args_s.is_empty() {
                "yield".to_string()
            } else {
                format!("yield {}", args_s.join(", "))
            }
        }
        ExprNode::Raise { value } => format!("raise {}", emit_expr(value)),
        ExprNode::RescueModifier { expr, fallback } => {
            format!("{} rescue {}", emit_expr(expr), emit_expr(fallback))
        }
        ExprNode::Return { value } => {
            if matches!(&*value.node, ExprNode::Lit { value: crate::expr::Literal::Nil }) {
                "return".to_string()
            } else {
                format!("return {}", emit_expr(value))
            }
        }
        ExprNode::Super { args } => match args {
            None => "super".to_string(),
            Some(args) => {
                let args_s: Vec<String> = args.iter().map(emit_expr).collect();
                format!("super({})", args_s.join(", "))
            }
        },
        ExprNode::Next { value } => match value {
            None => "next".to_string(),
            Some(v) => format!("next {}", emit_expr(v)),
        },
        ExprNode::MultiAssign { targets, value } => {
            let lhs: Vec<String> = targets.iter().map(emit_lvalue).collect();
            format!("{} = {}", lhs.join(", "), emit_expr(value))
        }
        ExprNode::While { cond, body, until_form } => {
            let kw = if *until_form { "until" } else { "while" };
            format!(
                "{kw} {}\n{}\nend",
                emit_expr(cond),
                indent_lines(&emit_expr(body), 1),
            )
        }
        ExprNode::Range { begin, end, exclusive } => {
            let op = if *exclusive { "..." } else { ".." };
            let b = begin.as_ref().map(emit_expr).unwrap_or_default();
            let e = end.as_ref().map(emit_expr).unwrap_or_default();
            format!("{b}{op}{e}")
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, implicit } => {
            let mut s = String::new();
            if !*implicit {
                s.push_str("begin\n");
            }
            s.push_str(&indent_lines(&emit_expr(body), 1));
            s.push('\n');
            for rc in rescues {
                s.push_str("rescue");
                if !rc.classes.is_empty() {
                    let cs: Vec<String> = rc.classes.iter().map(emit_expr).collect();
                    s.push(' ');
                    s.push_str(&cs.join(", "));
                }
                if let Some(name) = &rc.binding {
                    s.push_str(&format!(" : {}", cs_or_exception(rc)));
                    s.push_str(&format!(" => {name}"));
                }
                s.push('\n');
                s.push_str(&indent_lines(&emit_expr(&rc.body), 1));
                s.push('\n');
            }
            if let Some(eb) = else_branch {
                s.push_str("else\n");
                s.push_str(&indent_lines(&emit_expr(eb), 1));
                s.push('\n');
            }
            if let Some(en) = ensure {
                s.push_str("ensure\n");
                s.push_str(&indent_lines(&emit_expr(en), 1));
                s.push('\n');
            }
            if !*implicit {
                s.push_str("end");
            }
            s
        }
    }
}

/// In Crystal, `rescue ex` requires an exception class type when a
/// binding name is used (`rescue ex : Exception`). Helper to render
/// the type clause; falls back to `Exception` when none was named.
fn cs_or_exception(_rc: &crate::expr::RescueClause) -> String {
    "Exception".to_string()
}

fn emit_bool_op(
    op: crate::expr::BoolOpKind,
    _surface: crate::expr::BoolOpSurface,
    left: &Expr,
    right: &Expr,
) -> String {
    use crate::expr::BoolOpKind;
    // Crystal supports both `||`/`&&` and `or`/`and` keywords; the
    // symbol form is the unambiguous choice (Crystal's `or`/`and`
    // have different precedence than Ruby's, so symbol-form keeps
    // semantics unchanged).
    let op_s = match op {
        BoolOpKind::Or => "||",
        BoolOpKind::And => "&&",
    };
    format!("{} {} {}", emit_expr(left), op_s, emit_expr(right))
}

fn emit_string_interp(parts: &[InterpPart]) -> String {
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

fn emit_array(elements: &[Expr], _style: &crate::expr::ArrayStyle) -> String {
    // Crystal doesn't have %i / %w shorthand — render bracket form
    // unconditionally. `style` is preserved for round-trip fidelity
    // in Ruby/Spinel emit but doesn't affect Crystal output.
    if elements.is_empty() {
        // Crystal rejects bare `[]` for the same reason as `{}`
        // (untyped). `[] of String?` is a permissive default that
        // matches the lowered IR's typical usage (errors arrays,
        // accumulator strings). Type-mismatched call sites surface a
        // Crystal error and we fix the source there.
        return "[] of String?".to_string();
    }
    let parts: Vec<String> = elements.iter().map(emit_expr).collect();
    format!("[{}]", parts.join(", "))
}

fn emit_hash(entries: &[(Expr, Expr)], braced: bool) -> String {
    let parts: Vec<String> = entries
        .iter()
        .map(|(k, v)| {
            // Symbol-keyed entries use the bareword shorthand
            // `key: value` when the key is a simple identifier — same
            // as Ruby/Spinel. In Crystal this produces a NamedTuple
            // entry, which interoperates with `**opts` parameters.
            // Hyphenated/special-character keys quote: `"data-x": v`.
            // Non-symbol keys fall through to the rocket form
            // (`expr => value`), producing a Hash literal.
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
        if parts.is_empty() {
            // Crystal rejects bare `{}` because it can't infer Hash vs
            // NamedTuple types. The transpiled framework runtime uses
            // empty hashes as default args (`def initialize(hash = {})`)
            // and as accumulators. `{} of Symbol => String?` is a
            // permissive default that matches the lowered IR's typical
            // usage (Rails Hash with symbol keys, nilable string values);
            // call sites that need different element types will surface
            // a Crystal type error and we'll fix the source there.
            "{} of Symbol => String?".to_string()
        } else {
            format!("{{ {} }}", parts.join(", "))
        }
    } else {
        parts.join(", ")
    }
}

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

pub(super) fn emit_send_base(
    recv: Option<&Expr>,
    method: &Symbol,
    args: &[Expr],
    parenthesized: bool,
) -> String {
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    let m = method.as_str();
    // `recv[idx]` and `recv[idx] = value` rendering. Always emits
    // index-syntax even when receiver is `self` — Ruby's parser shapes
    // `self[k]` as `Send { recv: SelfRef, method: "[]", args: [k] }`,
    // which the SelfRef-collapse below would render as the bare token
    // `[](k)` and Crystal would parse as a malformed empty-array
    // literal. Same reasoning for `self[k] = v` → `Send { method:
    // "[]=", args: [k, v] }`. Drop into index syntax explicitly.
    if (m == "[]" || m == "[]=") && !args_s.is_empty() {
        let recv_s = match recv {
            Some(r) if matches!(&*r.node, ExprNode::SelfRef) => "self".to_string(),
            Some(r) => emit_expr(r),
            None => "self".to_string(),
        };
        if m == "[]=" && args_s.len() == 2 {
            return format!("{recv_s}[{}] = {}", args_s[0], args_s[1]);
        }
        return format!("{recv_s}[{}]", args_s.join(", "));
    }

    if matches!(recv, Some(r) if matches!(&*r.node, ExprNode::SelfRef))
        && !is_setter_method(m)
        && !super::shared::is_crystal_reserved(m)
    {
        if args_s.is_empty() {
            return method.to_string();
        }
        if parenthesized {
            return format!("{method}({})", args_s.join(", "));
        }
        return format!("{method} {}", args_s.join(", "));
    }
    match (recv, m) {
        (Some(r), "[]") => format!("{}[{}]", emit_expr(r), args_s.join(", ")),
        (Some(r), op) if is_binary_operator(op) && args_s.len() == 1 => {
            format!("{} {op} {}", emit_expr(r), args_s[0])
        }
        (Some(r), name) if is_setter_method(name) && args_s.len() == 1 => {
            let attr = &name[..name.len() - 1];
            format!("{}.{attr} = {}", emit_expr(r), args_s[0])
        }
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

fn is_binary_operator(m: &str) -> bool {
    matches!(
        m,
        "==" | "!="
            | "<"
            | "<="
            | ">"
            | ">="
            | "<=>"
            | "==="
            | "=~"
            | "!~"
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "**"
            | "<<"
            | ">>"
            | "&"
            | "|"
            | "^"
    )
}

fn is_setter_method(m: &str) -> bool {
    if !m.ends_with('=') || m.len() < 2 {
        return false;
    }
    if matches!(m, "==" | "!=" | "<=" | ">=" | "<=>" | "===" | "=~") {
        return false;
    }
    if m == "[]=" {
        return false;
    }
    true
}

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
            indent_lines(body_str, 1),
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
            if s.contains('.') {
                s
            } else {
                format!("{s}.0")
            }
        }
        Literal::Str { value } => format!("{value:?}"),
        Literal::Sym { value } => format!(":{value}"),
        Literal::Regex { pattern, flags } => format!("/{pattern}/{flags}"),
    }
}

fn emit_lvalue(lv: &LValue) -> String {
    match lv {
        LValue::Var { name, .. } => escape_ident(name.as_str()),
        LValue::Ivar { name } => format!("@{name}"),
        LValue::Attr { recv, name } => format!("{}.{name}", emit_expr(recv)),
        LValue::Index { recv, index } => format!("{}[{}]", emit_expr(recv), emit_expr(index)),
    }
}

fn emit_arm(arm: &Arm) -> String {
    let mut s = format!("when {}", emit_pattern(&arm.pattern));
    if let Some(g) = &arm.guard {
        s.push_str(&format!(" if {}", emit_expr(g)));
    }
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
            if let Some(r) = rest {
                parts.push(format!("*{r}"));
            }
            format!("[{}]", parts.join(", "))
        }
        Pattern::Record { fields, rest } => {
            let mut parts: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!("{k}: {}", emit_pattern(v)))
                .collect();
            if *rest {
                parts.push("**".into());
            }
            format!("{{ {} }}", parts.join(", "))
        }
    }
}
