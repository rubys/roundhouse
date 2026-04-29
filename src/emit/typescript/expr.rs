//! Generic TypeScript body / expression / literal emission. Used by
//! the standalone `emit_method` (runtime extraction) and indirectly by
//! controller / view / model / spec emitters that fall back to
//! arbitrary `Expr` rendering.

use super::naming::{ts_field_name, ts_method_name};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ty::Ty;

// Body + expressions ---------------------------------------------------

pub(super) fn emit_body(body: &Expr, return_ty: &Ty) -> String {
    // Pre-walk: find local-var names assigned more than once in this
    // method body. They'll emit as `let` at first occurrence and bare
    // `name = value` thereafter. Names assigned exactly once still
    // emit as `const`. The `declared` set tracks which reassigned names
    // have already had their declaration emitted as we walk in source
    // order.
    let mut reassigned: std::collections::HashMap<crate::ident::Symbol, usize> =
        std::collections::HashMap::new();
    count_var_assignments(body, &mut reassigned);
    let reassigned: std::collections::HashSet<crate::ident::Symbol> = reassigned
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(s, _)| s)
        .collect();
    let mut declared: std::collections::HashSet<crate::ident::Symbol> =
        std::collections::HashSet::new();
    emit_body_with_state(body, return_ty, &reassigned, &mut declared)
}

fn emit_body_with_state(
    body: &Expr,
    return_ty: &Ty,
    reassigned: &std::collections::HashSet<crate::ident::Symbol>,
    declared: &mut std::collections::HashSet<crate::ident::Symbol>,
) -> String {
    let is_void = matches!(return_ty, Ty::Nil);
    match &*body.node {
        // Guard-clause: ingest rewrites `return if cond; rest...` to
        // `If { cond, then: nil, else: <rest> }` (see ingest/expr.rs's
        // "Guard-clause rewrite"). Reverse it on the way out so we
        // emit `if (cond) return; <rest>` instead of nesting the
        // whole method body inside the else branch. Only applies when
        // the then branch is the literal nil placeholder the rewrite
        // synthesizes.
        ExprNode::If { cond, then_branch, else_branch }
            if matches!(&*then_branch.node, ExprNode::Lit { value: Literal::Nil })
                && !is_nil_or_empty(else_branch) =>
        {
            let guard = format!(
                "if ({}) return{};",
                emit_expr(cond),
                if is_void { "" } else { " null" },
            );
            let rest = emit_body_with_state(else_branch, return_ty, reassigned, declared);
            format!("{guard}\n{rest}")
        }
        // `def initialize(owner); @owner = owner; end` — the assignment
        // is the whole body. Emit the assignment as a statement, then
        // return its value if non-void. Without this, the side-effect
        // of setting the ivar is lost (`{};` of the value alone reads
        // the local but doesn't write the ivar).
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            let field = ts_field_name(name.as_str());
            let value_s = emit_expr(value);
            if is_void {
                format!("this.{field} = {value_s};")
            } else {
                format!("return this.{field} = {value_s};")
            }
        }
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = Vec::new();
            for (i, e) in exprs.iter().enumerate() {
                lines.push(emit_stmt_with_state(
                    e,
                    i == exprs.len() - 1,
                    is_void,
                    reassigned,
                    declared,
                ));
            }
            lines.join("\n")
        }
        // Method-body-level begin/rescue emits as native try/catch rather
        // than IIFE-wrapped. Preserves control flow: early `return` inside
        // the body actually exits the method, `throw e` outside the match
        // arms rethrows cleanly, and no needless `(() => { ... })()` noise.
        ExprNode::BeginRescue { body: inner, rescues, else_branch, ensure, .. } => {
            emit_begin_rescue_stmt(inner, rescues, else_branch.as_ref(), ensure.as_ref(), return_ty)
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

/// Walk an expression tree counting `Assign { LValue::Var { name } }`
/// occurrences per name. Used to identify locals that need `let`
/// declarations (mutated more than once) versus locals that fit
/// `const` (single-assignment). The traversal visits all children so
/// reassignments inside nested if/while/case branches are counted.
fn count_var_assignments(
    e: &Expr,
    out: &mut std::collections::HashMap<crate::ident::Symbol, usize>,
) {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            *out.entry(name.clone()).or_insert(0) += 1;
            count_var_assignments(value, out);
        }
        ExprNode::Assign { value, .. } => count_var_assignments(value, out),
        ExprNode::Seq { exprs } => {
            for e in exprs {
                count_var_assignments(e, out);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            count_var_assignments(cond, out);
            count_var_assignments(then_branch, out);
            count_var_assignments(else_branch, out);
        }
        ExprNode::BoolOp { left, right, .. } => {
            count_var_assignments(left, out);
            count_var_assignments(right, out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                count_var_assignments(r, out);
            }
            for a in args {
                count_var_assignments(a, out);
            }
            if let Some(b) = block {
                count_var_assignments(b, out);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            count_var_assignments(fun, out);
            for a in args {
                count_var_assignments(a, out);
            }
            if let Some(b) = block {
                count_var_assignments(b, out);
            }
        }
        ExprNode::While { cond, body, .. } => {
            count_var_assignments(cond, out);
            count_var_assignments(body, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            count_var_assignments(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    count_var_assignments(g, out);
                }
                count_var_assignments(&arm.body, out);
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            count_var_assignments(body, out);
            for r in rescues {
                count_var_assignments(&r.body, out);
            }
            if let Some(e) = else_branch {
                count_var_assignments(e, out);
            }
            if let Some(e) = ensure {
                count_var_assignments(e, out);
            }
        }
        ExprNode::Lambda { body, .. } => count_var_assignments(body, out),
        ExprNode::Let { value, body, .. } => {
            count_var_assignments(value, out);
            count_var_assignments(body, out);
        }
        ExprNode::Return { value }
        | ExprNode::Raise { value }
        | ExprNode::RescueModifier { expr: value, .. } => count_var_assignments(value, out),
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    count_var_assignments(expr, out);
                }
            }
        }
        _ => {}
    }
}

/// Render a begin/rescue at statement position — inside a method body
/// rather than as an expression. Preserves native TS control flow:
/// `try { ... } catch (e) { ... } finally { ... }` with early-return
/// and rethrow working as Ruby's semantics expect.
fn emit_begin_rescue_stmt(
    body: &Expr,
    rescues: &[crate::expr::RescueClause],
    else_branch: Option<&Expr>,
    ensure: Option<&Expr>,
    return_ty: &Ty,
) -> String {
    let mut out = String::new();
    out.push_str("try {\n");
    let body_s = emit_body(body, return_ty);
    for line in body_s.lines() {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    if let Some(eb) = else_branch {
        // Ruby's `else` runs iff the body raised nothing. Appending to
        // the try block preserves that ordering.
        let eb_s = emit_body(eb, return_ty);
        for line in eb_s.lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("} catch (e) {\n");

    // Chain rescue clauses as `if (e instanceof X) { ... } else if ...
    // else { throw e; }`. Bare rescue (no classes) is the catchall.
    let mut bare_catchall = false;
    for (i, rc) in rescues.iter().enumerate() {
        let body_s = emit_body(&rc.body, return_ty);
        let indented = indent_block(&body_s, 4);
        if rc.classes.is_empty() {
            out.push_str("  ");
            out.push_str(&indented.trim_start().to_string());
            out.push('\n');
            bare_catchall = true;
            break;
        }
        let instanceof_s: Vec<String> = rc
            .classes
            .iter()
            .map(|c| format!("e instanceof {}", emit_expr(c)))
            .collect();
        let keyword = if i == 0 { "if" } else { "} else if" };
        out.push_str("  ");
        out.push_str(&format!("{keyword} ({}) {{\n", instanceof_s.join(" || ")));
        for line in body_s.lines() {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
    }
    if !bare_catchall && !rescues.is_empty() {
        out.push_str("  } else {\n");
        out.push_str("    throw e;\n");
        out.push_str("  }\n");
    } else if rescues.is_empty() {
        // `begin; body; ensure; ...; end` with no rescue — must still
        // rethrow in the catch to preserve exception propagation.
        out.push_str("  throw e;\n");
    }
    out.push_str("}");

    if let Some(en) = ensure {
        out.push_str(" finally {\n");
        let en_s = emit_body(en, &Ty::Nil);
        for line in en_s.lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("}");
    }
    out
}

fn indent_block(s: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    s.lines()
        .map(|l| format!("{pad}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn emit_stmt(e: &Expr, is_last: bool, void_return: bool) -> String {
    let empty: std::collections::HashSet<crate::ident::Symbol> =
        std::collections::HashSet::new();
    let mut declared = empty.clone();
    emit_stmt_with_state(e, is_last, void_return, &empty, &mut declared)
}

fn emit_stmt_with_state(
    e: &Expr,
    is_last: bool,
    void_return: bool,
    reassigned: &std::collections::HashSet<crate::ident::Symbol>,
    declared: &mut std::collections::HashSet<crate::ident::Symbol>,
) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            // First occurrence of a name that we know will be reassigned
            // → `let`. First occurrence of a name assigned exactly once
            // → `const`. Subsequent occurrences (only possible for
            // reassigned names) → bare `name = value`.
            let escaped = escape_reserved_word(name.as_str());
            if reassigned.contains(name) {
                if declared.insert(name.clone()) {
                    format!("let {} = {};", escaped, emit_expr(value))
                } else {
                    format!("{} = {};", escaped, emit_expr(value))
                }
            } else {
                format!("const {} = {};", escaped, emit_expr(value))
            }
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("this.{} = {};", ts_field_name(name.as_str()), emit_expr(value))
        }
        // Return at statement position: emit as a native `return`
        // rather than wrapping in an IIFE. Ruby's `return nil` returns
        // nil, not undefined — emit `return null;` (not bare `return;`)
        // to preserve that semantic under TS's strict equality rules.
        ExprNode::Return { value } => {
            format!("return {};", emit_expr(value))
        }
        // Guard-return pattern: `if (cond) { return X; }` at statement
        // position, with no else branch (or an else that's nil). Rather
        // than emit a ternary, produce a native guard — preserves the
        // Ruby idiom `return nil if cond` as idiomatic TS.
        ExprNode::If { cond, then_branch, else_branch }
            if matches!(&*then_branch.node, ExprNode::Return { .. })
                && is_nil_or_empty(else_branch) =>
        {
            if let ExprNode::Return { value } = &*then_branch.node {
                format!("if ({}) return {};", emit_expr(cond), emit_expr(value))
            } else {
                unreachable!()
            }
        }
        // Postfix-`if` at statement position with no else branch.
        // Ruby's `x = [] if x.nil?` lowers to `If { cond, then=Assign,
        // else=nil }`. The default arm below would route through
        // `emit_expr` (a ternary), which drops the assignment's LHS
        // (`Assign` in expression position emits only the rhs). Emit
        // a native `if (cond) <stmt>;` instead — preserves the side
        // effect.
        ExprNode::If { cond, then_branch, else_branch } if is_nil_or_empty(else_branch) => {
            let cond_s = emit_expr(cond);
            let then_stmt = emit_stmt_with_state(then_branch, false, true, reassigned, declared);
            // emit_stmt with void_return=true gives a side-effect-only
            // form (no `return` wrapping). Already includes its own
            // trailing semicolon.
            format!("if ({cond_s}) {then_stmt}")
        }
        // Two-branch (or chained-elsif) `if` at statement position
        // when the value isn't being returned. The default arm would
        // emit a ternary (correct for value-position) but Ruby's
        // `if cond; @x = 1 elsif ...` is mutating local/ivar state —
        // a ternary discards the side effect. Block-form `if/else`
        // preserves it. When `is_last && !void_return`, fall through
        // to ternary so the value still flows out.
        ExprNode::If { cond, then_branch, else_branch }
            if !is_last || void_return =>
        {
            emit_if_block(cond, then_branch, else_branch, reassigned, declared)
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

/// Emit a multi-branch `if/else if/.../else` block at statement
/// position. Recurses on the else-branch when it's another `If`,
/// producing a flat `else if` chain instead of nested `else { if ... }`.
fn emit_if_block(
    cond: &Expr,
    then_branch: &Expr,
    else_branch: &Expr,
    reassigned: &std::collections::HashSet<crate::ident::Symbol>,
    declared: &mut std::collections::HashSet<crate::ident::Symbol>,
) -> String {
    let mut out = String::new();
    out.push_str("if (");
    out.push_str(&emit_expr(cond));
    out.push_str(") ");
    out.push_str(&emit_branch_block(then_branch, reassigned, declared));

    let mut current = else_branch;
    loop {
        if is_nil_or_empty(current) {
            return out;
        }
        match &*current.node {
            ExprNode::If { cond, then_branch, else_branch } => {
                out.push_str(" else if (");
                out.push_str(&emit_expr(cond));
                out.push_str(") ");
                out.push_str(&emit_branch_block(then_branch, reassigned, declared));
                current = else_branch;
            }
            _ => {
                out.push_str(" else ");
                out.push_str(&emit_branch_block(current, reassigned, declared));
                return out;
            }
        }
    }
}

/// Emit a single branch of an `if` block. Always-braced; multi-stmt
/// `Seq` indents naturally; single-stmt branches fit on one line
/// inside the braces. Branches inside `if/else` are statements (no
/// implicit return), so void_return = true.
fn emit_branch_block(
    e: &Expr,
    reassigned: &std::collections::HashSet<crate::ident::Symbol>,
    declared: &mut std::collections::HashSet<crate::ident::Symbol>,
) -> String {
    match &*e.node {
        ExprNode::Seq { exprs } if exprs.len() > 1 => {
            let mut s = String::from("{\n");
            for (i, sub) in exprs.iter().enumerate() {
                let stmt = emit_stmt_with_state(
                    sub,
                    i == exprs.len() - 1,
                    true,
                    reassigned,
                    declared,
                );
                for line in stmt.lines() {
                    s.push_str("  ");
                    s.push_str(line);
                    s.push('\n');
                }
            }
            s.push('}');
            s
        }
        _ => {
            let stmt = emit_stmt_with_state(e, true, true, reassigned, declared);
            format!("{{ {stmt} }}")
        }
    }
}

/// Suffix `_` to JS reserved words used as identifiers. Mirrors the
/// `escape_reserved` in the parent module that's applied to method
/// parameter names; here we apply it to local-variable references so
/// `params.fetch(:k, default)`'s body sees `default_` (matching the
/// param-name escape) instead of bare `default` (a JS keyword).
fn escape_reserved_word(name: &str) -> String {
    matches!(
        name,
        "default"
            | "with"
            | "function"
            | "class"
            | "for"
            | "let"
            | "const"
            | "var"
            | "return"
            | "switch"
            | "case"
            | "if"
            | "else"
            | "while"
            | "do"
            | "yield"
            | "delete"
            | "new"
            | "this"
            | "super"
            | "true"
            | "false"
            | "null"
            | "void"
            | "typeof"
            | "instanceof"
    )
    .then(|| format!("{name}_"))
    .unwrap_or_else(|| name.to_string())
}

/// `!x` parses tighter than `===`, `==`, `||`, `&&`, etc. — without
/// parentheses, `!x === y` reads as `(!x) === y`. Heuristic: if the
/// emitted operand contains a binary operator at top level, wrap it.
/// False positives (over-parenthesizing) are harmless; false negatives
/// invert the meaning. Skip parens on already-paren'd, identifier, or
/// member-access forms.
fn needs_parens_for_unary_not(s: &str) -> bool {
    if s.starts_with('(') && s.ends_with(')') {
        return false;
    }
    // Conservative: any space-separated infix operator triggers parens.
    s.contains(" === ")
        || s.contains(" !== ")
        || s.contains(" == ")
        || s.contains(" != ")
        || s.contains(" && ")
        || s.contains(" || ")
        || s.contains(" < ")
        || s.contains(" > ")
        || s.contains(" <= ")
        || s.contains(" >= ")
        || s.contains(" + ")
        || s.contains(" - ")
        || s.contains(" * ")
        || s.contains(" / ")
}

fn is_nil_or_empty(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Lit { value: Literal::Nil }
            | ExprNode::Seq { .. }  // empty Seq also falls here
    ) && matches!(
        &*e.node,
        ExprNode::Lit { value: Literal::Nil }
            | ExprNode::Seq { exprs: _ }
    ) && {
        if let ExprNode::Seq { exprs } = &*e.node {
            exprs.is_empty()
        } else {
            matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
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
        ExprNode::Var { name, .. } => escape_reserved_word(name.as_str()),
        ExprNode::Ivar { name } => format!("this.{}", ts_field_name(name.as_str())),
        ExprNode::Send { recv, method, args, block, parenthesized } => {
            emit_send_with_block(
                recv.as_ref(),
                method.as_str(),
                args,
                block.as_ref(),
                *parenthesized,
            )
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
        ExprNode::SelfRef => "this".to_string(),
        ExprNode::Lambda { params, body, .. } => {
            let params_s: Vec<String> = params.iter().map(|p| p.as_str().to_string()).collect();
            let body_s = emit_expr(body);
            match params.len() {
                0 => format!("() => {body_s}"),
                1 => format!("{} => {body_s}", params_s[0]),
                _ => format!("({}) => {body_s}", params_s.join(", ")),
            }
        }
        ExprNode::Return { value } => {
            // Expression-position return is rare — typically the
            // statement-level emitter handles Return cleanly. An IIFE
            // preserves semantics when Return appears inside a larger
            // expression (e.g., ternary guard `cond ? (return x) : y`).
            format!("(() => {{ return {}; }})()", emit_expr(value))
        }
        ExprNode::Super { args } => {
            // Ruby's `super` forwards to the parent class's same-named
            // method. TS requires `super.methodName(...)`, which needs
            // enclosing-method context that this emitter doesn't carry.
            // Emit syntactically-valid `super(...)` — class-level
            // emitters rewrite to `super.X(...)` where they know X.
            let args_s: Vec<String> = match args {
                None => vec![],
                Some(a) => a.iter().map(emit_expr).collect(),
            };
            format!("super({})", args_s.join(", "))
        }
        ExprNode::BeginRescue { body, rescues, ensure, .. } => {
            // Expression-position begin/rescue — wrap the try/catch in
            // an IIFE so the whole thing evaluates to a value. Single
            // bare `rescue` is common; multi-clause becomes an
            // instanceof chain in the catch body.
            let body_s = emit_expr(body);
            let catch_body = build_catch_body(rescues);
            let ensure_s = match ensure {
                Some(e) => format!(" finally {{ {}; }}", emit_expr(e)),
                None => String::new(),
            };
            format!(
                "(() => {{ try {{ return {body_s}; }} catch (e) {{ {catch_body} }}{ensure_s} }})()"
            )
        }
        ExprNode::RescueModifier { expr, fallback } => {
            format!(
                "(() => {{ try {{ return {}; }} catch {{ return {}; }} }})()",
                emit_expr(expr),
                emit_expr(fallback)
            )
        }
        ExprNode::Yield { args } => {
            // Ruby's `yield` invokes the enclosing method's implicit
            // block. Library-class emit gives every yield-using method
            // an injected `__block` parameter (see emit_plain_method);
            // here we just call it. Yield always targets the enclosing
            // *method*, not surrounding lambdas, so a naive substitution
            // is safe — the caller arranges __block to be in scope.
            let args_s: Vec<String> = args.iter().map(emit_expr).collect();
            format!("__block({})", args_s.join(", "))
        }
        other => format!("/* TODO: emit {:?} */", std::mem::discriminant(other)),
    }
}

fn build_catch_body(rescues: &[crate::expr::RescueClause]) -> String {
    if rescues.is_empty() {
        return "throw e;".to_string();
    }
    // Bare rescue (no explicit classes) catches everything.
    if rescues.len() == 1 && rescues[0].classes.is_empty() {
        return format!("return {};", emit_expr(&rescues[0].body));
    }
    let mut out = String::new();
    let mut has_bare_catchall = false;
    for (i, rc) in rescues.iter().enumerate() {
        if rc.classes.is_empty() {
            out.push_str(&format!(" else {{ return {}; }}", emit_expr(&rc.body)));
            has_bare_catchall = true;
            break;
        }
        let keyword = if i == 0 { "if" } else { "else if" };
        let instanceof_s: Vec<String> = rc
            .classes
            .iter()
            .map(|c| format!("e instanceof {}", emit_expr(c)))
            .collect();
        out.push_str(&format!(
            "{keyword} ({}) {{ return {}; }}",
            instanceof_s.join(" || "),
            emit_expr(&rc.body)
        ));
    }
    if !has_bare_catchall {
        out.push_str(" else { throw e; }");
    }
    out
}

/// Core send emission. `parenthesized` reflects whether the Ruby
/// source wrapped args in explicit parens — for 0-arg explicit-
/// receiver calls we use it to decide between `recv.name` (Ruby
/// reader convention, JS property access) and `recv.name()` (method
/// call). Always emits parens when args are present.
/// Send emission that folds a trailing block (Ruby `do ... end` / `&:sym`)
/// in as an arrow-function argument — TS's closest equivalent. The
/// block-less path delegates directly to `emit_send_with_parens`.
pub(super) fn emit_send_with_block(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    parenthesized: bool,
) -> String {
    let Some(blk) = block else {
        return emit_send_with_parens(recv, method, args, parenthesized);
    };
    let mut all_args: Vec<Expr> = args.to_vec();
    all_args.push(blk.clone());
    emit_send_with_parens(recv, method, &all_args, true)
}

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
    // `recv.[]=(k, v)` — indexed assignment lowered to a Send. TS needs
    // the LHS form `recv[k] = v`.
    if method == "[]=" && recv.is_some() && args.len() == 2 {
        return format!(
            "{}[{}] = {}",
            emit_expr(recv.unwrap()),
            args_s[0],
            args_s[1]
        );
    }
    // `Target.new(args)` → `new Target(args)`. Ruby's standard constructor
    // call convention; Juntos-side classes use the JS `new` keyword.
    if method == "new" && recv.is_some() {
        let recv_s = emit_expr(recv.unwrap());
        if args_s.is_empty() {
            return format!("new {recv_s}()");
        }
        return format!("new {recv_s}({})", args_s.join(", "));
    }
    // `x.nil?` → `x === null`. Ruby's `nil?` only matches nil (not
    // `false`, and TS equivalent must distinguish from `undefined`).
    // Strict equality preserves semantics.
    if method == "nil?" && recv.is_some() && args.is_empty() {
        return format!("{} === null", emit_expr(recv.unwrap()));
    }
    // `x.is_a?(ClassRef)` → `x instanceof ClassRef`. Ruby's predicate
    // form vs TS's binary operator. Cross-target classes that don't
    // exist in JS (`String`, `Numeric`, `Integer`, `Symbol`) need a
    // different mapping but produce TS that's at least syntactically
    // valid; per-class lowering is a follow-on.
    if method == "is_a?" && recv.is_some() && args.len() == 1 {
        return format!(
            "{} instanceof {}",
            emit_expr(recv.unwrap()),
            args_s[0],
        );
    }
    // Kernel `raise` — the runtime_src self-rewrite leaves it as
    // Send-no-recv. Two source surfaces:
    //   `raise X, "msg"`  → `throw new X("msg")`
    //   `raise X.new("msg")` → `throw new X("msg")` (already a Send)
    //   `raise "msg"`     → `throw new Error("msg")`
    // The bare-error form (`raise "msg"`) hasn't been observed in the
    // framework runtime yet; add a case if it appears.
    if method == "raise" && recv.is_none() {
        match args.len() {
            2 => {
                return format!(
                    "(() => {{ throw new {}({}); }})()",
                    args_s[0], args_s[1],
                );
            }
            1 => {
                return format!("(() => {{ throw {}; }})()", args_s[0]);
            }
            _ => {}
        }
    }
    // `x.!` — the Send-channel form of unary `!` (e.g., `!cond` lowered
    // to `cond.!`). Emit TS's prefix `!`. Parenthesize the operand so
    // `!x.nil?` (which lowers `nil?` to `x === null`) emits as
    // `!(x === null)` not `!x === null` — the latter parses as
    // `(!x) === null` and inverts the meaning.
    if method == "!" && recv.is_some() && args.is_empty() {
        let inner = emit_expr(recv.unwrap());
        return if needs_parens_for_unary_not(&inner) {
            format!("!({inner})")
        } else {
            format!("!{inner}")
        };
    }
    // Type-aware Ruby Enumerable → JS Array method rename. JS arrays
    // don't have `.each` (they have `.forEach`) and use `.length` not
    // `.size`. When the analyzer has typed the receiver as Array, emit
    // the JS-native form.
    if let Some(r) = recv {
        if let Some(Ty::Array { .. }) = &r.ty {
            match method {
                "each" => {
                    let recv_s = emit_expr(r);
                    return if args_s.is_empty() {
                        format!("{recv_s}.forEach")
                    } else {
                        format!("{recv_s}.forEach({})", args_s.join(", "))
                    };
                }
                "size" | "length" if args.is_empty() => {
                    return format!("{}.length", emit_expr(r));
                }
                "empty?" if args.is_empty() => {
                    return format!("{}.length === 0", emit_expr(r));
                }
                "any?" if args.is_empty() => {
                    return format!("{}.length > 0", emit_expr(r));
                }
                "first" if args.is_empty() => {
                    return format!("{}[0]", emit_expr(r));
                }
                "last" if args.is_empty() => {
                    let recv_s = emit_expr(r);
                    return format!("{recv_s}[{recv_s}.length - 1]");
                }
                _ => {}
            }
        }
        // String-typed receiver: the same `.empty?` predicate has the
        // same JS spelling (length-zero), and `.length` carries through.
        // Keep the arms parallel with Array so further per-type
        // additions land in obvious neighborhoods.
        if let Some(Ty::Str) = &r.ty {
            match method {
                "empty?" if args.is_empty() => {
                    return format!("{}.length === 0", emit_expr(r));
                }
                "size" if args.is_empty() => {
                    return format!("{}.length", emit_expr(r));
                }
                _ => {}
            }
        }
    }

    // Ruby's `<<` is polymorphic: Int bit-shift, Array/String append,
    // or a method call on classes that define it (like
    // ActiveModel::Errors.add). Dispatch on receiver type. TS has no
    // `<<` operator overloading, so the Class case has to emit as a
    // method call; the method name is `add` by convention (matches
    // Juntos's ActiveModel::Errors and similar collection APIs).
    if method == "<<" && recv.is_some() && args.len() == 1 {
        let r = recv.unwrap();
        if let Some(recv_ty) = &r.ty {
            match recv_ty {
                Ty::Class { .. } => {
                    return format!("{}.add({})", emit_expr(r), args_s[0]);
                }
                Ty::Array { .. } => {
                    return format!("{}.push({})", emit_expr(r), args_s[0]);
                }
                _ => {}
            }
        }
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
        // `-` dispatch: TS's native `-` handles numerics. Array set-
        // difference uses filter + includes. Incompatible pairs refuse.
        if method == "-" {
            use crate::emit::shared::sub::{classify_sub, SubCase};
            match classify_sub(r, arg) {
                SubCase::ArrayDifference { .. } => {
                    return format!(
                        "{}.filter(x => !{}.includes(x))",
                        emit_expr(r),
                        emit_expr(arg)
                    );
                }
                SubCase::Incompatible => {
                    return r#"(() => { throw new Error("roundhouse: - with incompatible operand types"); })()"#.to_string();
                }
                _ => {}
            }
        }
        // `*` dispatch: TS's native `*` handles numerics. String repeat
        // uses `.repeat(n)`; array repeat has no built-in (flat
        // map-ish trick); array join uses `.join(sep)`.
        if method == "*" {
            use crate::emit::shared::mul::{classify_mul, MulCase};
            match classify_mul(r, arg) {
                MulCase::StringRepeat => {
                    return format!("{}.repeat({})", emit_expr(r), emit_expr(arg));
                }
                MulCase::ArrayRepeat { .. } => {
                    // Array(n).fill(lhs).flat() repeats the array n times.
                    return format!(
                        "Array({}).fill({}).flat()",
                        emit_expr(arg),
                        emit_expr(r)
                    );
                }
                MulCase::ArrayJoin { .. } => {
                    return format!("{}.join({})", emit_expr(r), emit_expr(arg));
                }
                MulCase::Incompatible => {
                    return r#"(() => { throw new Error("roundhouse: * with incompatible operand types"); })()"#.to_string();
                }
                _ => {}
            }
        }
        // `/` and `**` dispatch: TS has both as native operators. Only
        // Incompatible pairs need special handling.
        if method == "/" || method == "**" {
            use crate::emit::shared::div_pow::{classify_div_pow, DivPowCase};
            if matches!(classify_div_pow(r, arg), DivPowCase::Incompatible) {
                return format!(
                    r#"(() => {{ throw new Error("roundhouse: `{method}` with incompatible operand types"); }})()"#
                );
            }
        }
        // `%` dispatch: TS has native `%` for numerics; Str % args
        // (Ruby sprintf) has no JS/TS equivalent — emit a throw.
        if method == "%" {
            use crate::emit::shared::modulo::{classify_modulo, ModuloCase};
            match classify_modulo(r, arg) {
                ModuloCase::StringFormat => {
                    return r#"(() => { throw new Error("roundhouse: String % (sprintf) not yet supported for TypeScript target"); })()"#.to_string();
                }
                ModuloCase::Incompatible => {
                    return r#"(() => { throw new Error("roundhouse: % with incompatible operand types"); })()"#.to_string();
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
        Literal::Regex { pattern, flags } => format!("/{pattern}/{flags}"),
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
