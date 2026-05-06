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
    // Names whose first assignment lives inside a nested block (an
    // `if`/`else` arm, a `case` branch, …) need a hoisted `let`
    // declaration at the function-body level — TS `let` is block-
    // scoped, so a `let x = ...` inside the if-arm doesn't reach
    // sibling statements. Without hoisting, Ruby idioms like
    //   if cond
    //     x = "..."
    //     return x
    //   end
    //   x = "..."   # second assignment, outside the if
    // emit a TS file that references `x` outside of any visible
    // declaration. Always-hoist would lose the readable
    // `let x = init;` form for top-level-first reassignments;
    // restrict to vars whose top-level assignment count is strictly
    // less than their total count — i.e., at least one assignment
    // lives in a nested branch.
    let mut top_level_counts: std::collections::HashMap<crate::ident::Symbol, usize> =
        std::collections::HashMap::new();
    count_top_level_var_assignments(body, &mut top_level_counts);
    let mut hoisted: Vec<crate::ident::Symbol> = reassigned
        .iter()
        .filter(|name| {
            let total = count_var_for(body, name);
            let top = top_level_counts.get(*name).copied().unwrap_or(0);
            top < total
        })
        .cloned()
        .collect();
    hoisted.sort();
    let mut hoist_decls = String::new();
    for name in &hoisted {
        let escaped = escape_reserved_word(name.as_str());
        hoist_decls.push_str(&format!("let {escaped}: any;\n"));
        declared.insert(name.clone());
    }
    let body_s = emit_body_with_state(body, return_ty, &reassigned, &mut declared);
    if hoist_decls.is_empty() {
        body_s
    } else {
        format!("{hoist_decls}{body_s}")
    }
}

/// Count Var-assignment occurrences only at the top level of a
/// function body's `Seq` — siblings of the body's outermost
/// statement list. Nested branches (if-arms, case bodies) are NOT
/// counted; this lets the hoist-detection logic identify vars whose
/// reassignment splits across scopes.
fn count_top_level_var_assignments(
    body: &Expr,
    out: &mut std::collections::HashMap<crate::ident::Symbol, usize>,
) {
    match &*body.node {
        ExprNode::Seq { exprs } => {
            for e in exprs {
                if let ExprNode::Assign { target: LValue::Var { name, .. }, .. } = &*e.node {
                    *out.entry(name.clone()).or_insert(0) += 1;
                }
            }
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, .. } => {
            *out.entry(name.clone()).or_insert(0) += 1;
        }
        _ => {}
    }
}

/// Total count of Var-assignments to `name` in `body` (recursive).
fn count_var_for(body: &Expr, name: &crate::ident::Symbol) -> usize {
    let mut all: std::collections::HashMap<crate::ident::Symbol, usize> =
        std::collections::HashMap::new();
    count_var_assignments(body, &mut all);
    all.get(name).copied().unwrap_or(0)
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
        // Single-Case-as-whole-body (e.g., `process_action`'s synthesized
        // dispatcher): route through emit_stmt so it emits as a `switch`
        // rather than falling to the default arm that wraps the whole
        // node in `emit_expr` (which has no Case handler).
        ExprNode::Case { .. } => {
            emit_stmt_with_state(body, true, is_void, reassigned, declared)
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
        // Buffer-accumulate `var << X` is rewritten by `emit_stmt` to
        // `var += X;` — i.e., an assignment for declaration purposes.
        // Count it so the var gets `let` (mutable) instead of `const`.
        ExprNode::Send { recv: Some(recv), method, args, block, .. }
            if method.as_str() == "<<" && args.len() == 1 =>
        {
            if let ExprNode::Var { name, .. } = &*recv.node {
                *out.entry(name.clone()).or_insert(0) += 1;
            }
            count_var_assignments(recv, out);
            for a in args {
                count_var_assignments(a, out);
            }
            if let Some(b) = block {
                count_var_assignments(b, out);
            }
        }
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

/// Pre-walk a body to identify reassigned local-variable names.
/// Public for `view_thin.rs`'s use; the result feeds
/// `emit_stmt_with_state` so multi-statement bodies emit `let`
/// (mutable) for names assigned more than once and `const` for
/// names assigned exactly once.
pub(super) fn collect_reassigned(body: &Expr) -> std::collections::HashSet<crate::ident::Symbol> {
    let mut counts: std::collections::HashMap<crate::ident::Symbol, usize> =
        std::collections::HashMap::new();
    count_var_assignments(body, &mut counts);
    counts.into_iter().filter(|(_, n)| *n > 1).map(|(s, _)| s).collect()
}

pub(super) fn emit_stmt_with_state(
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
        // Buffer-accumulate idiom at statement position:
        // `buf << X` (Ruby) → `buf += X;` (TS), where `buf` is any
        // local-variable receiver. The lowered view body uses this
        // shape (`io << ViewHelpers.x(...)`); form_with's inner
        // capture uses `body << ...` with a different name. Same
        // rewrite applies — the receiver just needs to be an
        // `ExprNode::Var`. At expression position the type-aware
        // dispatch in `emit_send_with_parens` still handles
        // typed-Array `.push()` etc.
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
            if method.as_str() == "<<" && args.len() == 1 =>
        {
            // Buffer-accumulate idiom only applies to a String-typed
            // local variable (the synthesized view `io` buffer or
            // similar). Arrays go through `emit_expr` so the
            // type-aware `<<` dispatch produces `.push(...)`. When
            // the recv has no type (Untyped), fall back to `+=` to
            // preserve the prior behavior on view bodies that
            // synthesize a buffer without explicit init.
            if let ExprNode::Var { name, .. } = &*recv.node {
                let is_string_buf = matches!(
                    recv.ty,
                    Some(Ty::Str) | None,
                );
                if is_string_buf {
                    let val_s = emit_expr(&args[0]);
                    return format!("{} += {val_s};", escape_reserved_word(name.as_str()));
                }
            }
            // Receiver isn't a String-typed bare local — fall through
            // to the default arm, which routes through `emit_expr`
            // (and its type-aware `<<` dispatch for arrays /
            // class-with-add).
            if is_last && !void_return {
                format!("return {};", emit_expr(e))
            } else {
                format!("{};", emit_expr(e))
            }
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
            // Use emit_branch_block so multi-statement (or single-elem
            // Seq) then-branches recurse through the proper stmt path
            // — without this, a Seq then-branch falls to emit_stmt's
            // default arm which routes through emit_expr (losing the
            // `<<` → `+=` rewrite, the `let`/`const` declaration, etc.).
            // Single non-Seq stmts emit as `if (cond) { stmt }` —
            // technically braced where Ruby's postfix-if is brace-less,
            // but the brace form is universally valid TS.
            let then_block = emit_branch_block(then_branch, reassigned, declared);
            format!("if ({cond_s}) {then_block}")
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
        // `while cond; body; end` and `until cond; body; end` at
        // statement position emit as native loops. The until form
        // negates the condition (TS has no `until` keyword).
        ExprNode::While { cond, body, until_form } => {
            let cond_s = emit_expr(cond);
            let cond_s = if *until_form {
                format!("!({cond_s})")
            } else {
                cond_s
            };
            let body_stmt = emit_branch_block(body, reassigned, declared);
            format!("while ({cond_s}) {body_stmt}")
        }
        // `next` inside a Ruby block lowers to `return` from the JS
        // callback (since blocks become arrow functions). `next` with
        // a value (rare) returns that value; bare `next` returns
        // undefined. The synthesized lambda carries no value out, so
        // bare-return is fine.
        ExprNode::Next { value } => match value {
            Some(v) => format!("return {};", emit_expr(v)),
            None => "return;".to_string(),
        },
        // `case scrutinee; when X then body; ...; end` at statement
        // position. Emit as a TS `switch` when every arm pattern is a
        // single literal and the scrutinee is a simple value. Each arm
        // body is emitted recursively as a stmt (so bare method calls
        // become `this.method();`) followed by `break;`. Falls through
        // to the default-arm rendering (a TODO comment via emit_expr)
        // for non-literal patterns — the `process_action` dispatcher
        // (the only producer here today) always uses literal-symbol
        // arms.
        ExprNode::Case { scrutinee, arms }
            if arms.iter().all(|a| {
                a.guard.is_none()
                    && matches!(&a.pattern, crate::expr::Pattern::Lit { .. })
            }) =>
        {
            let scr_s = emit_expr(scrutinee);
            let mut out = format!("switch ({scr_s}) {{\n");
            for arm in arms {
                let pat_s = match &arm.pattern {
                    crate::expr::Pattern::Lit { value } => emit_literal(value),
                    _ => unreachable!(),
                };
                let body_stmt = emit_stmt_with_state(
                    &arm.body, false, true, reassigned, declared,
                );
                out.push_str(&format!("  case {pat_s}: {body_stmt} break;\n"));
            }
            out.push('}');
            out
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
        // Multi-stmt Seq → indented block. Single-stmt Seq is also
        // walked here (rather than falling through to the default,
        // which would route a Seq node through emit_expr) so its one
        // child stmt gets the proper emit_stmt treatment.
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            if exprs.len() == 1 {
                let stmt = emit_stmt_with_state(
                    &exprs[0],
                    true,
                    true,
                    reassigned,
                    declared,
                );
                return format!("{{ {stmt} }}");
            }
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

/// Unwrap a `Union<T, Nil>` to `T` for type-aware dispatch. The
/// flow-sensitive ivar typer wraps every ivar's type in
/// `Union<T, Nil>` because a first read can observe nil before any
/// assignment runs (see `parse_library_with_rbs`'s flow_ivars
/// reseed). The actual value is still `T` everywhere except the
/// possibly-nil first-read window, so dispatch on `T` is correct
/// for emit purposes. `Union<Nil>` and other shapes pass through
/// unchanged.
fn strip_nullable(ty: Option<&Ty>) -> Option<&Ty> {
    let ty = ty?;
    if let Ty::Union { variants } = ty {
        if variants.len() == 2 {
            let nil_idx = variants.iter().position(|v| matches!(v, Ty::Nil));
            if let Some(idx) = nil_idx {
                return Some(&variants[1 - idx]);
            }
        }
    }
    Some(ty)
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
            // Ruby `Foo::Bar` gets joined with `.` for TS access.
            // Framework-namespace paths (`ActionView::ViewHelpers`,
            // `ActionView::ViewHelpers::FormBuilder`) collapse to the
            // last segment — TS emits each runtime class flat at its
            // file's module scope and imports the bare name. The
            // import collector mirrors this collapse, so the call
            // site reaches the imported name directly. Other paths
            // (e.g. `Views::Articles` — a real nested object) pass
            // through joined.
            let segs: Vec<&str> = path.iter().map(|s| s.as_str()).collect();
            const FRAMEWORK_NAMESPACES: &[&str] = &[
                "ActionController",
                "ActiveRecord",
                "ActionView",
                "ActionDispatch",
                "ActiveSupport",
            ];
            if segs.len() >= 2 && FRAMEWORK_NAMESPACES.contains(&segs[0]) {
                segs.last().copied().unwrap_or("").to_string()
            } else {
                segs.join(".")
            }
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
            // TS ternary `cond ? a : b`, always parenthesized. JS's
            // `?:` has lower precedence than `||`/`&&`/comparisons, so
            // an unparenthesized ternary inside e.g. `x || (a ? b : c)`
            // miscompiles — the outer `||` binds first and the ternary
            // ends up testing `x || a` instead of `a`. Ruby's `||`
            // binds tighter than `?:` too, but Ruby callers express
            // the intended grouping with explicit parens; the emit
            // here can't see those parens, so wrap unconditionally.
            // `emit_expr` is always called in an expression position;
            // controller/view emitters have their own statement-form
            // If handlers.
            let cond_s = emit_expr(cond);
            let then_s = emit_expr(then_branch);
            let else_s = emit_expr(else_branch);
            format!("({cond_s} ? {then_s} : {else_s})")
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
            let header = match params.len() {
                0 => "()".to_string(),
                1 => params_s[0].clone(),
                _ => format!("({})", params_s.join(", ")),
            };
            // Multi-statement bodies need a block form so each
            // statement separates cleanly. Single-expression bodies
            // stay in the concise `args => expr` form. The body-
            // typer's return-flow tracking gives us the value of the
            // last statement; emit a `return` for that one.
            if let ExprNode::Seq { exprs } = &*body.node {
                if exprs.len() > 1 {
                    // Lambdas open a fresh scope. Pre-walk the body to
                    // identify reassigned locals (so e.g. an inner
                    // capture buffer `body = String.new` followed by
                    // `body << X` rewrites emits as `let body = ""`
                    // not `const body = ""`).
                    let reassigned = collect_reassigned(body);
                    let mut declared: std::collections::HashSet<crate::ident::Symbol> =
                        std::collections::HashSet::new();
                    let mut out = format!("{header} => {{ ");
                    for (i, e) in exprs.iter().enumerate() {
                        let stmt = emit_stmt_with_state(
                            e,
                            i == exprs.len() - 1,
                            false,
                            &reassigned,
                            &mut declared,
                        );
                        out.push_str(&stmt);
                        if i + 1 < exprs.len() {
                            out.push(' ');
                        }
                    }
                    out.push_str(" }");
                    return out;
                }
            }
            format!("{header} => {}", emit_expr(body))
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
        ExprNode::While { cond, body, until_form } => {
            // `while`/`until` at expression position is unusual —
            // wrap in IIFE so the syntactic position works. Statement-
            // position uses are handled in emit_stmt with a flat
            // form.
            let cond_s = emit_expr(cond);
            let cond_s = if *until_form {
                format!("!({cond_s})")
            } else {
                cond_s
            };
            let body_s = emit_expr(body);
            format!("(() => {{ while ({cond_s}) {{ {body_s}; }} }})()")
        }
        ExprNode::Next { value } => match value {
            Some(v) => format!("(() => {{ return {}; }})()", emit_expr(v)),
            None => "(() => { return; })()".to_string(),
        },
        ExprNode::Cast { value, target_ty } => {
            // TS `as T` is a compile-time assertion — no runtime
            // narrowing. The lowerer adds Cast at adapter-row sites
            // where the static value type is wider than the column;
            // TS's `any` lets the assignment through without `as`,
            // but emitting it documents intent and helps TS's narrowing.
            // Wrap value in parens for precedence safety.
            format!("({} as {})", emit_expr(value), super::ty::ts_ty(target_ty))
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
    if method == "[]" && recv.is_some() && args.len() == 1 {
        // Ruby's `x[i..j]` slice form — when the indexer's argument is
        // a Range, lower to `.slice(i, j+1)` (or `.slice(i)` for an
        // open-ended range, or `.slice(i, j)` for an exclusive range).
        // Works for Str AND Array receivers; both have `.slice` with
        // matching JS semantics.
        if let ExprNode::Range { begin, end, exclusive } = &*args[0].node {
            let begin_s = begin
                .as_ref()
                .map(|e| emit_expr(e))
                .unwrap_or_else(|| "0".to_string());
            let recv_s = emit_expr(recv.unwrap());
            return match end {
                None => format!("{recv_s}.slice({begin_s})"),
                Some(e) => {
                    let end_s = emit_expr(e);
                    if *exclusive {
                        format!("{recv_s}.slice({begin_s}, {end_s})")
                    } else {
                        format!("{recv_s}.slice({begin_s}, {end_s} + 1)")
                    }
                }
            };
        }
    }
    if method == "[]" && recv.is_some() && args.len() == 2 {
        // Ruby's two-arg `str[start, length]` / `arr[start, length]`
        // — substring/subarray of the given length. TS string and
        // array both expose `.slice(start, end)` with the same
        // start-inclusive/end-exclusive semantics, so the rewrite
        // is `recv.slice(start, start + length)`. Without this, the
        // generic `recv[a, b]` fallback produces JS `recv[(a, b)]`
        // (comma operator) — silently wrong.
        let recv_s = emit_expr(recv.unwrap());
        return format!("{recv_s}.slice({}, {} + {})", args_s[0], args_s[0], args_s[1]);
    }
    // Negative-int index on an Array (`arr[-1]` = last element,
    // `arr[-2]` = second to last, …). JS arrays don't support
    // negative indexing — `arr[-1]` reads the property "-1", which
    // is `undefined` for ordinary arrays. Rewrite to
    // `arr[arr.length + N]` (where N is negative). For dynamic
    // negative indices the same rewrite would need a runtime
    // check, but the literal case covers the framework patterns
    // we ship today (`records[-1]` in `Base.last`'s body).
    if method == "[]" && recv.is_some() && args.len() == 1 {
        if let (Some(r), ExprNode::Lit { value: Literal::Int { value } }) =
            (recv, &*args[0].node)
        {
            if *value < 0 && matches!(r.ty.as_ref(), Some(Ty::Array { .. }) | Some(Ty::Str)) {
                let recv_s = emit_expr(r);
                return format!("{recv_s}[{recv_s}.length{value}]");
            }
        }
    }
    // Framework class-instance receivers route bracket access to
    // method dispatch (`.get(k)` / `.set(k, v)`). JS bracket access
    // on a class instance returns `undefined` for runtime keys
    // (no index signature on the class shape); the framework runtime
    // classes (Parameters, HashWithIndifferentAccess, …) expose
    // explicit `get` / `set` methods as their cross-target API.
    //
    // Hash-typed and Array-typed receivers fall through to the
    // bracket-access form below — `Record<K, V>[k]` is correct for
    // Hash, and `T[][i]` is correct for Array. Same hardcoded class-
    // name list as the zero-arg-method fix; goes away once the typer
    // plumbs `AccessorKind` to Send.
    let is_framework_class_recv = |r: &Expr| -> bool {
        let recv_ty = strip_nullable(r.ty.as_ref());
        matches!(
            recv_ty,
            Some(Ty::Class { id, .. }) if {
                let name = id.0.as_str();
                let last = name.rsplit("::").next().unwrap_or(name);
                matches!(
                    last,
                    "HashWithIndifferentAccess"
                        | "Parameters"
                        | "ParameterMissing"
                        | "Router"
                )
            }
        )
    };
    if method == "[]" && args.len() == 1 {
        if let Some(r) = recv {
            if is_framework_class_recv(r) {
                return format!("{}.get({})", emit_expr(r), args_s[0]);
            }
            // Legacy hardcode for `@params[:k]` — @params's ty isn't
            // always recovered as Class (it can flow as Hash[Sym, Any]
            // through the analyzer), so the ivar-name shortcut still
            // pays for itself. Subsumed by the framework-class match
            // above when the type is recovered.
            if matches!(&*r.node, ExprNode::Ivar { name } if name.as_str() == "params") {
                return format!("{}.get({})", emit_expr(r), args_s[0]);
            }
        }
    }
    if method == "[]" && recv.is_some() {
        return format!("{}[{}]", emit_expr(recv.unwrap()), args_s.join(", "));
    }
    // `recv.[]=(k, v)` — indexed assignment lowered to a Send.
    if method == "[]=" && recv.is_some() && args.len() == 2 {
        let r = recv.unwrap();
        if is_framework_class_recv(r) {
            return format!("{}.set({}, {})", emit_expr(r), args_s[0], args_s[1]);
        }
        // Default: `recv[k] = v`.
        return format!(
            "{}[{}] = {}",
            emit_expr(r),
            args_s[0],
            args_s[1]
        );
    }
    // Attribute-writer Send: `obj.foo=(v)` → `obj.foo = v`. Ruby's
    // setter sugar dispatches as a method call on the `foo=` name;
    // TS uses property-assignment syntax. Only fires when the method
    // name ends in `=` (so `==` and `!=` aren't caught), excludes
    // operator names (`<=`, `>=`, etc. are binops handled elsewhere).
    if method.ends_with('=')
        && method.len() >= 2
        && method.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false)
        && recv.is_some()
        && args.len() == 1
    {
        let attr = &method[..method.len() - 1];
        return format!(
            "{}.{} = {}",
            emit_expr(recv.unwrap()),
            ts_field_name(attr),
            args_s[0]
        );
    }
    // `Target.new(args)` → `new Target(args)`. Ruby's standard constructor
    // call convention; Juntos-side classes use the JS `new` keyword.
    // Special cases for built-in types whose JS-side construction
    // syntax diverges:
    //   `String.new` → `""` (JS `new String()` produces a String
    //     OBJECT, not a primitive — different semantics for `+=`,
    //     equality, etc. Plain string literal is the correct mapping
    //     for buffer-accumulate idioms in lowered view bodies.)
    //   `Array.new` → `[]`
    //   `Hash.new` → `{}`
    if method == "new" && recv.is_some() {
        let recv_s = emit_expr(recv.unwrap());
        // Heuristic: only treat `.new(...)` as a constructor call when
        // the receiver is a bare class identifier (e.g. `Article`,
        // `Comment`). Member-access receivers like `Views.Articles`
        // refer to namespaced module-of-functions where `new` is just
        // a method name (the `new` action's view function); emitting
        // `new Views.Articles(...)` would invoke an object as a
        // constructor, which TS rejects at runtime. Fall through to
        // the regular member-call form for those.
        if !recv_s.contains('.') {
            if args_s.is_empty() {
                match recv_s.as_str() {
                    "String" => return "\"\"".to_string(),
                    "Array" => return "[]".to_string(),
                    "Hash" => return "{}".to_string(),
                    _ => {}
                }
                return format!("new {recv_s}()");
            }
            return format!("new {recv_s}({})", args_s.join(", "));
        }
    }
    // `x.nil?` → `x == null` (loose equality — matches both null
    // AND undefined). Ruby's `nil?` returns true for any unset ivar
    // (Ruby reads unset @vars as nil); the TS analog of "unset"
    // is `undefined`, not `null`. Strict `=== null` would miss
    // unset class fields and break the model constructor →
    // `fill_timestamps` path: when a field isn't supplied in
    // `new Article({...})`, `this.created_at` is undefined; the
    // `if self[:created_at].nil?` guard in fill_timestamps must
    // fire so the timestamp gets populated, otherwise the SQL
    // INSERT fails the NOT NULL constraint.
    //
    // Loose `== null` is safe against the false-vs-nil concern
    // earlier prose flagged: `false == null` is false in JS, so
    // `x.nil?` still distinguishes nil from false. Likewise
    // `0 == null` and `"" == null` are both false.
    if method == "nil?" && recv.is_some() && args.is_empty() {
        return format!("{} == null", emit_expr(recv.unwrap()));
    }
    // `x.class` (Ruby reflection — returns the receiver's class
    // object) → `x.constructor` in TS, which exposes the same
    // surface (static methods like `table_name`, `name`). Cast
    // through `any` so downstream property access on the
    // dynamically-typed constructor doesn't trip strict mode.
    if method == "class" && recv.is_some() && args.is_empty() {
        return format!("({}.constructor as any)", emit_expr(recv.unwrap()));
    }
    // `Time.now` → `new Date()`. Ruby's Time class has no JS
    // analog; JS Date covers the use cases the framework runtime
    // needs (`utc`, `iso8601`).
    if method == "now" && args.is_empty() {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*r.node {
                if path.len() == 1 && path[0].as_str() == "Time" {
                    return "new Date()".to_string();
                }
            }
        }
    }
    // Ruby stdlib `JSON.generate(x)` → JS `JSON.stringify(x)`. Same
    // semantics for the framework runtime's use cases (no
    // pretty-print options, no symbol-key handling). The companion
    // `JSON.parse` is identical in both languages — passes through
    // the generic Const-recv dispatch.
    if method == "generate" && args.len() == 1 {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*r.node {
                if path.len() == 1 && path[0].as_str() == "JSON" {
                    return format!("JSON.stringify({})", args_s[0]);
                }
            }
        }
    }
    // Ruby stdlib `Base64.strict_encode64(x)` → Node
    // `Buffer.from(x).toString("base64")`. `strict_encode64` differs
    // from `encode64` only in not inserting newlines; `Buffer`'s
    // base64 is already strict-shaped. The browser-side `btoa`
    // alternative would be ASCII-only — `Buffer.from` handles
    // arbitrary bytes correctly, matching Ruby's Base64 behavior.
    if method == "strict_encode64" && args.len() == 1 {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*r.node {
                if path.len() == 1 && path[0].as_str() == "Base64" {
                    return format!(
                        "Buffer.from({}).toString(\"base64\")",
                        args_s[0],
                    );
                }
            }
        }
    }
    // `<date>.utc` → no-op chained access; Date already represents
    // an absolute UTC instant. `.utc` returns `Date` itself.
    if method == "utc" && args.is_empty() && recv.is_some() {
        let inner = emit_expr(recv.unwrap());
        // Recognize `new Date()` form so the emit collapses
        // `Time.now.utc` → `new Date()` cleanly. Otherwise keep
        // the chain readable as-is.
        if inner == "new Date()" {
            return inner;
        }
    }
    // `<date>.iso8601` → `.toISOString()` — produces the Z-suffix
    // ISO-8601 string Ruby's Time#iso8601 does.
    if method == "iso8601" && args.is_empty() && recv.is_some() {
        return format!("{}.toISOString()", emit_expr(recv.unwrap()));
    }
    // `<regex>.match?(s)` → `<regex>.test(s)`. Ruby's `Regexp#match?`
    // returns boolean; JS RegExp has `.test()` for the same purpose.
    // Both `match?` (predicate) and `match` (returns MatchData) get
    // mapped: predicate → `.test()`, value form → `.exec()`.
    if method == "match?" && args.len() == 1 && recv.is_some() {
        return format!("{}.test({})", emit_expr(recv.unwrap()), args_s[0]);
    }
    if method == "match" && args.len() == 1 && recv.is_some() {
        if let Some(crate::ty::Ty::Class { id, .. }) = recv.unwrap().ty.as_ref() {
            if id.0.as_str() == "Regexp" || id.0.as_str() == "RegExp" {
                return format!("{}.exec({})", emit_expr(recv.unwrap()), args_s[0]);
            }
        }
    }
    // Ruby coercions: `.to_s` / `.to_i` / `.to_sym` map to JS
    // equivalents. `.to_sym` is a no-op in JS (use the string as
    // the hash key) — emit just the receiver. The nil case
    // diverges from Ruby (Ruby's nil.to_s is "" but JS String(null)
    // is "null"); call sites that care should narrow first.
    if recv.is_some() && args.is_empty() {
        match method {
            "to_s" => return format!("String({})", emit_expr(recv.unwrap())),
            "to_i" => return format!("Number({})", emit_expr(recv.unwrap())),
            "to_sym" => return emit_expr(recv.unwrap()),
            _ => {}
        }
    }
    // `x.is_a?(ClassRef)` → JS form. Most Ruby classes are
    // user-defined and translate to `x instanceof ClassRef`, but
    // primitives in Ruby (String, Integer, Float, Numeric, Symbol)
    // are JS primitives, not class instances — `"abc" instanceof
    // String` is false in JS. Map those to their `typeof` form.
    // Array gets `Array.isArray(x)` (cross-realm safe) instead of
    // `instanceof Array`.
    if method == "is_a?" && recv.is_some() && args.len() == 1 {
        let recv_s = emit_expr(recv.unwrap());
        let class_s = &args_s[0];
        return match class_s.as_str() {
            "String" => format!("typeof {recv_s} === \"string\""),
            "Integer" => format!("Number.isInteger({recv_s})"),
            "Float" => format!("typeof {recv_s} === \"number\" && !Number.isInteger({recv_s})"),
            "Numeric" => format!("typeof {recv_s} === \"number\""),
            // Ruby Symbol values render as TS strings (Lit::Sym
            // emits as a quoted string), so `is_a?(Symbol)` maps to
            // the same `typeof === "string"` check `is_a?(String)`
            // produces. Without the redirect, `typeof === "symbol"`
            // narrows TS's static type to `symbol`, which then
            // triggers TS2731 ("implicit Symbol-to-string coercion")
            // on subsequent template-literal interpolations.
            "Symbol" => format!("typeof {recv_s} === \"string\""),
            "Array" => format!("Array.isArray({recv_s})"),
            "TrueClass" | "FalseClass" => format!("typeof {recv_s} === \"boolean\""),
            // Ruby's `Hash` is a plain object in JS — no constructor
            // class to `instanceof` against. The plain-object check
            // is "typeof object && not null && not array".
            "Hash" => format!(
                "typeof {recv_s} === \"object\" && {recv_s} !== null && !Array.isArray({recv_s})"
            ),
            // `Regexp` is the Ruby builtin name; JS spells it
            // `RegExp` — same semantics, `instanceof` works once
            // the class name is corrected.
            "Regexp" => format!("{recv_s} instanceof RegExp"),
            _ => format!("{recv_s} instanceof {class_s}"),
        };
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
                // Ruby builtin error classes that have no TS analog
                // collapse to `Error`. Without this, the emitted code
                // references undeclared globals (`new NotImplementedError(...)`)
                // and tsc bails out with TS2304.
                let class_s = match args_s[0].as_str() {
                    "NotImplementedError" | "ArgumentError" | "RuntimeError"
                    | "TypeError" | "NameError" | "NoMethodError"
                    | "StandardError" | "KeyError" | "IndexError" => "Error",
                    other => other,
                };
                return format!(
                    "(() => {{ throw new {}({}); }})()",
                    class_s, args_s[1],
                );
            }
            1 => {
                return format!("(() => {{ throw {}; }})()", args_s[0]);
            }
            _ => {}
        }
    }
    // Kernel `puts` / `print` / `p` / `pp` — map to `console.log`.
    // The rewrite pass leaves these as Send-no-recv (alongside `raise`)
    // so they don't pick up an inappropriate `this.` prefix in static
    // method bodies. Ruby's variants differ in inspect-vs-to_s formatting
    // and trailing-newline handling; `console.log` is close enough for
    // the diagnostic purpose these calls serve in seeds / generators,
    // and avoids a per-variant runtime shim.
    if recv.is_none() && matches!(method, "puts" | "print" | "p" | "pp") {
        return format!("console.log({})", args_s.join(", "));
    }
    // Kernel `require` / `require_relative` / `load` / `autoload`
    // — Ruby's late-bound module loading. TS resolves modules at
    // import time via ES module syntax (handled separately in the
    // file header), so call-site `require "base64"` has no
    // analog and drops to a no-op. Emitting `null` keeps the
    // statement well-formed; treeshake / minifier elide it.
    if recv.is_none()
        && matches!(method, "require" | "require_relative" | "load" | "autoload")
    {
        return "null".to_string();
    }
    // `x.!` — the Send-channel form of unary `!` (e.g., `!cond` lowered
    // to `cond.!`). Emit TS's prefix `!`. Parenthesize the operand so
    // `!x.nil?` (which lowers `nil?` to `x === null`) emits as
    // `!(x === null)` not `!x === null` — the latter parses as
    // `(!x) === null` and inverts the meaning.
    //
    // Two surface forms reach here, both meaning "logical not":
    //   Send { recv: Some(x), method: "!", args: [] }   — Ruby's x.!()
    //   Send { recv: None,    method: "!", args: [x] }  — view_to_library's
    //                                                     `not_x = send(None, "!", [x])`
    // Handle both with the same prefix-`!` emission.
    if method == "!" {
        let inner_expr: Option<&Expr> = match (recv, args) {
            (Some(r), []) => Some(r),
            (None, [a]) => Some(a),
            _ => None,
        };
        if let Some(inner) = inner_expr {
            let inner_s = emit_expr(inner);
            return if needs_parens_for_unary_not(&inner_s) {
                format!("!({inner_s})")
            } else {
                format!("!{inner_s}")
            };
        }
    }
    // Type-aware per-receiver dispatch. The receiver type may be
    // nullable (an ivar's flow-sensitive type is `Union<T, Nil>` since
    // a first read can observe nil before any assignment); strip the
    // nullable wrapper so dispatch fires on the inner type.
    if let Some(r) = recv {
        let recv_ty = strip_nullable(r.ty.as_ref());
        match recv_ty {
            // Ruby Array → JS Array (with native method renames where
            // they diverge: `.each` → `.forEach`, `.size` → `.length`).
            Some(Ty::Array { .. }) => match method {
                "each" => {
                    let recv_s = emit_expr(r);
                    return if args_s.is_empty() {
                        format!("{recv_s}.forEach")
                    } else {
                        format!("{recv_s}.forEach({})", args_s.join(", "))
                    };
                }
                "size" | "length" | "count" if args.is_empty() => {
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
                // Ruby's `arr.reverse` returns a new array; JS Array
                // has the same name but mutates in place. Pair it with
                // a `[...arr]` spread so the receiver isn't clobbered.
                // Also covers the bare-call form (`arr.reverse` without
                // parens) — Ruby allows zero-arg method calls without
                // parens, but TS requires `()` so we always emit them.
                "reverse" if args.is_empty() => {
                    return format!("[...{}].reverse()", emit_expr(r));
                }
                // `arr.to_a` is a no-op on arrays; arr.to_h converts
                // a `[[k, v], ...]` array to an object via Object.fromEntries.
                "to_a" if args.is_empty() => return emit_expr(r),
                "to_h" if args.is_empty() => {
                    return format!("Object.fromEntries({})", emit_expr(r));
                }
                // `arr.sort_by { |x| key(x) }` returns a new array
                // sorted by the key. JS Array#sort takes a comparator
                // (returning -1/0/+1) not a key function — wrap via
                // an IIFE that captures the key lambda once and
                // applies the standard ka<kb / ka>kb comparator.
                // `[...arr]` makes a copy (Ruby sort_by is
                // non-mutating; JS sort mutates in place).
                "sort_by" if args.len() == 1 => {
                    let recv_s = emit_expr(r);
                    let key_fn = &args_s[0];
                    return format!(
                        "((__arr, __key) => [...__arr].sort((a, b) => {{ const ka = __key(a); const kb = __key(b); return ka < kb ? -1 : ka > kb ? 1 : 0; }}))({recv_s}, {key_fn})"
                    );
                }
                // `arr.sort` (no block) → JS Array#sort with default
                // comparator on a fresh copy. JS's default sort is
                // string-coerced (matches Ruby's sort for strings,
                // diverges for numbers but those need an explicit
                // comparator anyway).
                "sort" if args.is_empty() => {
                    return format!("[...{}].sort()", emit_expr(r));
                }
                // Ruby's `Array#join` with no args uses `$,` as the
                // separator (defaults to nil → "").  JS's
                // `Array.prototype.join()` defaults to "," — wrong
                // semantics. Always pass an explicit separator.
                "join" if args.is_empty() => {
                    return format!("{}.join(\"\")", emit_expr(r));
                }
                "join" if args.len() == 1 => {
                    return format!("{}.join({})", emit_expr(r), args_s[0]);
                }
                _ => {}
            },
            // String receiver: predicate forms parallel Array's; the
            // case-shift helpers map to JS String methods.
            Some(Ty::Str) => match method {
                "empty?" if args.is_empty() => {
                    return format!("{}.length === 0", emit_expr(r));
                }
                "size" if args.is_empty() => {
                    return format!("{}.length", emit_expr(r));
                }
                "upcase" if args.is_empty() => {
                    return format!("{}.toUpperCase()", emit_expr(r));
                }
                "downcase" if args.is_empty() => {
                    return format!("{}.toLowerCase()", emit_expr(r));
                }
                "capitalize" if args.is_empty() => {
                    // JS has no built-in capitalize. Match Ruby's
                    // semantics: uppercase the first char, lowercase
                    // the rest. Wrap in IIFE so the receiver expr is
                    // evaluated once even when we reference it twice.
                    let recv_s = emit_expr(r);
                    return format!(
                        "(__s => __s.charAt(0).toUpperCase() + __s.slice(1).toLowerCase())({recv_s})"
                    );
                }
                "strip" if args.is_empty() => {
                    return format!("{}.trim()", emit_expr(r));
                }
                "reverse" if args.is_empty() => {
                    return format!("{}.split(\"\").reverse().join(\"\")", emit_expr(r));
                }
                "chars" if args.is_empty() => {
                    return format!("{}.split(\"\")", emit_expr(r));
                }
                "start_with?" if args.len() == 1 => {
                    return format!("{}.startsWith({})", emit_expr(r), args_s[0]);
                }
                "end_with?" if args.len() == 1 => {
                    return format!("{}.endsWith({})", emit_expr(r), args_s[0]);
                }
                "include?" if args.len() == 1 => {
                    return format!("{}.includes({})", emit_expr(r), args_s[0]);
                }
                // `s.sub(pat, repl)` → `s.replace(pat, repl)` (first
                // match only — JS replace's default semantics match
                // Ruby sub).
                "sub" if args.len() == 2 => {
                    return format!(
                        "{}.replace({}, {})",
                        emit_expr(r),
                        args_s[0],
                        args_s[1],
                    );
                }
                // `s.gsub(pat, repl)` → `s.replace(pat_with_g, repl)`.
                // Ruby gsub replaces every match; JS replace defaults
                // to first only — the regex needs a `g` flag. Patch
                // it inline when the pattern is a regex literal;
                // otherwise wrap with a runtime g-flag enforcer.
                // Hash replacements (`s.gsub(re, MAP)`) wrap in a
                // lookup callback `m => MAP[m]`.
                "gsub" if args.len() == 2 => {
                    let pat_s = if let ExprNode::Lit {
                        value: Literal::Regex { pattern, flags },
                    } = &*args[0].node
                    {
                        let new_flags = if flags.contains('g') {
                            flags.clone()
                        } else {
                            format!("{flags}g")
                        };
                        format!("/{pattern}/{new_flags}")
                    } else {
                        // Runtime check — covers Const refs to
                        // regex constants (`HTML_ESCAPE_PATTERN`)
                        // whose type isn't visible at emit time.
                        let raw = &args_s[0];
                        format!(
                            "{raw} instanceof RegExp && !{raw}.flags.includes(\"g\") ? new RegExp({raw}.source, {raw}.flags + \"g\") : {raw}",
                        )
                    };
                    let repl_s = if matches!(args[1].ty.as_ref(), Some(Ty::Hash { .. })) {
                        format!("(__m: string) => ({})[__m]", args_s[1])
                    } else {
                        args_s[1].clone()
                    };
                    return format!("{}.replace({pat_s}, {repl_s})", emit_expr(r));
                }
                // `s.tr(from, to)` — character translation. Limited
                // to single-char from/to (covers framework Ruby's
                // `inner_k.to_s.tr("_", "-")` shape). Multi-char
                // and ranges aren't yet supported; the call falls
                // through to the generic dispatch (and tsc errors)
                // so the gap surfaces instead of silently miscompiling.
                "tr" if args.len() == 2 => {
                    if let (
                        ExprNode::Lit { value: Literal::Str { value: from } },
                        ExprNode::Lit { value: Literal::Str { value: to } },
                    ) = (&*args[0].node, &*args[1].node)
                    {
                        if from.chars().count() == 1 && to.chars().count() == 1 {
                            // Escape the `from` char for use inside a
                            // regex character class.
                            let c = from.chars().next().unwrap();
                            let escaped = match c {
                                '\\' | '/' | '^' | '$' | '.' | '|' | '?'
                                | '*' | '+' | '(' | ')' | '[' | ']' | '{'
                                | '}' => format!("\\{c}"),
                                _ => c.to_string(),
                            };
                            return format!(
                                "{}.replace(/{}/g, {:?})",
                                emit_expr(r),
                                escaped,
                                to,
                            );
                        }
                    }
                }
                _ => {}
            },
            // Hash → JS plain-object. `.merge` becomes object spread;
            // `.key?` becomes the `in` operator; `.empty?` counts keys;
            // `.each |k, v|` iterates entries.
            Some(Ty::Hash { .. }) => {
                let recv_s = emit_expr(r);
                match method {
                    "key?" | "has_key?" | "include?" if args.len() == 1 => {
                        return format!("{} in {recv_s}", args_s[0]);
                    }
                    "empty?" if args.is_empty() => {
                        return format!("Object.keys({recv_s}).length === 0");
                    }
                    "any?" if args.is_empty() => {
                        return format!("Object.keys({recv_s}).length > 0");
                    }
                    "size" | "length" if args.is_empty() => {
                        return format!("Object.keys({recv_s}).length");
                    }
                    "merge" if args.len() == 1 => {
                        return format!("{{ ...{recv_s}, ...{} }}", args_s[0]);
                    }
                    // `hash.delete(key)` — Ruby removes the key in
                    // place and returns the deleted value (or nil).
                    // JS plain objects don't have `.delete()`; the
                    // `delete` keyword is the statement form. Emit
                    // an IIFE so the Send expression is valid in
                    // both expression and statement position, and so
                    // the return value matches Ruby (the prior
                    // value at the key, or `undefined` if absent —
                    // close enough to nil for the framework's call
                    // sites).
                    "delete" if args.len() == 1 => {
                        return format!(
                            "((__h, __k) => {{ const __v = __h[__k]; delete __h[__k]; return __v; }})({recv_s}, {})",
                            args_s[0],
                        );
                    }
                    "keys" if args.is_empty() => {
                        return format!("Object.keys({recv_s})");
                    }
                    "values" if args.is_empty() => {
                        return format!("Object.values({recv_s})");
                    }
                    // `.to_h` on a Hash is a no-op in Ruby — emit the
                    // receiver verbatim. The strong-params chain
                    // (`params.require(:k).permit(:a, :b).to_h`) is
                    // the common producer.
                    "to_h" if args.is_empty() => return recv_s,
                    // `hash.fetch(key, default)` → `hash[key] ?? default`.
                    // Spec lowering's `<Resource>Params.from_raw` body
                    // emits `params.fetch("title", "")` for each
                    // permitted field; without this rewrite the Send
                    // emits literally and tsc rejects since
                    // `Record<string, any>` has no `.fetch`. The
                    // single-arg form falls through to a bracket
                    // index — Ruby's KeyError on missing key isn't
                    // modeled in TS.
                    "fetch" if args.len() == 2 => {
                        return format!("{recv_s}[{}] ?? {}", args_s[0], args_s[1]);
                    }
                    "fetch" if args.len() == 1 => {
                        return format!("{recv_s}[{}]", args_s[0]);
                    }
                    "dup" | "clone" if args.is_empty() => {
                        return format!("{{ ...{recv_s} }}");
                    }
                    "each" if args_s.len() <= 1 => {
                        // `hash.each |k, v| { ... }` lowers to a
                        // 2-arg block. JS's `Object.entries(o).forEach`
                        // passes a single `[k, v]` tuple; wrap the
                        // block in a forwarder that pulls the pair
                        // apart so the caller-supplied 2-arg lambda
                        // sees `(k, v)` as Ruby intended. Without the
                        // forwarder, a `(k, v) =>` block would receive
                        // `[k, v], index, _arr` instead.
                        return if args_s.is_empty() {
                            format!("Object.entries({recv_s})")
                        } else {
                            format!(
                                "Object.entries({recv_s}).forEach(__p => ({})(__p[0], __p[1]))",
                                args_s[0],
                            )
                        };
                    }
                    _ => {}
                }
            }
            _ => {}
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
                // Ruby's `str << x` appends in place. TS strings
                // are immutable, but the receiver is always a
                // local variable in our view-builder pattern (the
                // synthesized `io` buffer), so `+=` produces the
                // same effect at the call site. View bodies use
                // this as `io << "...html..."` to accumulate output.
                Ty::Str => {
                    return format!("{} += {}", emit_expr(r), args_s[0]);
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
    //
    // `include?` (no type info) → `.includes(...)` — the Array dispatch
    // covers known-Array receivers above; this catches the case
    // where receiver type is `any` (e.g. `(this.constructor as any).schema_columns.include?(...)`).
    // Hash receivers reach the type-aware branch and emit as `in`,
    // so they don't fall through here.
    let (mapped_name, force_parens) = match method {
        "strip" => ("trim", true),
        "include?" => ("includes", true),
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
            // Ruby's `obj.name` without parens is typically a reader;
            // Juntos mirrors that with a property accessor / getter,
            // so emit without parens for instance receivers.
            //
            // EXCEPTION: when the receiver is a `Const` (a namespace
            // import like `ViewHelpers`, `RouteHelpers`, `Inflector`,
            // `Array`, `String`, `Math`, …), zero-arg sends are
            // function CALLS, not property reads — those namespaces
            // expose callable functions, not getters. Always emit
            // parens for Const-receiver sends so `RouteHelpers.articles_path`
            // becomes `RouteHelpers.articles_path()` instead of leaking
            // the function reference.
            //
            // SUB-EXCEPTION: a small set of class-level attr_accessor
            // fields in the framework runtime (`ActiveRecord.adapter`,
            // …) emit as `static x: T;` not as a method, so callers
            // need property access not a call. Carry the list here
            // until the typer surfaces AccessorKind through Send.
            let is_const_recv = matches!(&*r.node, ExprNode::Const { .. });
            let const_field = is_const_recv && {
                let path = if let ExprNode::Const { path } = &*r.node {
                    path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::")
                } else {
                    String::new()
                };
                matches!(
                    (path.as_str(), method),
                    ("ActiveRecord", "adapter")
                )
            };
            let suppress_const_parens = is_const_recv && const_field;
            // Class-instance receivers whose class is one of the
            // transpiled framework runtime classes ALWAYS need `()`
            // for zero-arg method calls — these classes use explicit
            // `def` methods (Parameters, HashWithIndifferentAccess,
            // ActionDispatch::Router, etc.), not attr_reader-collapsed
            // TS fields. Without `()`, the emit produces a method
            // reference (`this.hash.is_empty`) instead of a call
            // (`this.hash.is_empty()`), which JS lets stand at parse
            // time and produces wrong values at runtime.
            //
            // App-level model classes (Article, Comment, …) have their
            // attr_readers collapsed to TS fields, so zero-arg
            // `article.title` SHOULD stay as property access — those
            // aren't on this list. The body-typer carries
            // `AccessorKind` per method but doesn't yet thread it
            // through to the Send-emit; this hardcoded class-name list
            // is the bridge until that plumbing lands.
            // The receiver type may be nullable (`Union<Class, Nil>`
            // — an ivar's flow-sensitive type is nullable until the
            // analyzer proves a write-before-read). Strip the Nil
            // variant so the class-name match fires on the real type.
            let recv_ty_inner = strip_nullable(r.ty.as_ref());
            let is_method_class_recv = matches!(
                recv_ty_inner,
                Some(Ty::Class { id, .. }) if {
                    let name = id.0.as_str();
                    let last = name.rsplit("::").next().unwrap_or(name);
                    matches!(
                        last,
                        "HashWithIndifferentAccess"
                            | "Parameters"
                            | "ParameterMissing"
                            | "Router"
                    )
                }
            );
            let raw = if args_s.is_empty()
                && !parenthesized
                && !force_parens
                && !is_method_class_recv
                && (!is_const_recv || suppress_const_parens)
            {
                format!("{recv_s}.{ts_m}")
            } else {
                format!("{recv_s}.{ts_m}({})", args_s.join(", "))
            };
            // Self-type narrowing: framework Base methods like
            // `find`/`all`/`where`/`last`/`create` declare a return
            // type of `Base` in RBS, but at the call site
            // `Article.find(id)` should yield `Article`. Roundhouse's
            // RBS parser doesn't support `instance` / `self` types,
            // so emit a TS cast here when the receiver is a Const
            // class and the method is one of the self-typed Base
            // class methods. Resolves TS2740 ("Base missing
            // properties from Article") on every model assignment
            // from a class-method result. Singular methods cast to
            // `<Class>`; collection-returning methods cast to
            // `<Class>[]`. The result is parenthesized so any
            // chained access reads from the casted value, not from
            // the un-parenthesized cast which TS parses as
            // `<expr> as <Class.member>`.
            if is_const_recv {
                // Method names match against the Ruby form (with the
                // `!`/`?` suffix the source uses) since the IR
                // preserves them — sanitize happens later, in
                // `ts_method_name`. So `Article.create!(...)` lands
                // here with `method == "create!"`.
                let stripped = method.trim_end_matches('!').trim_end_matches('?');
                let cast_target = match stripped {
                    "find" | "find_by" | "last" | "create" | "first" => Some(recv_s.clone()),
                    "all" | "where" => Some(format!("{recv_s}[]")),
                    _ => None,
                };
                if let Some(target) = cast_target {
                    return format!("({raw} as {target})");
                }
            }
            raw
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
        Literal::Regex { pattern, flags } => {
            // Ruby regex anchors don't have direct JS equivalents:
            //   `\A` / `\z` / `\Z` — Ruby string-boundary anchors,
            //                        absolute (don't match before/after \n).
            //   JS `^` / `$` — line-boundary anchors by default.
            //
            // Without translation, `/\A\d{5}\z/` emits literally and JS
            // treats `\A` / `\z` as escaped letters (matches the
            // characters A and z), silently breaking every Ruby pattern
            // that uses them. JS `^` / `$` without the `m` flag are
            // string-boundary in practice (no `m` → no per-line shift),
            // matching Ruby `\A` / `\z` for the framework's use cases
            // (validates_format_of, route param matching, etc.).
            //
            // Translate only when neither escaped already. `\\A`
            // (literal-backslash followed by A) keeps that meaning.
            // Cheap state machine: walk char-by-char, copy escaped
            // backslash sequences verbatim, replace bare `\A` / `\z` /
            // `\Z`. Strict-end `\Z` differs from `\z` only in trailing-
            // newline handling — close enough to `$` for these targets.
            let translated = translate_ruby_regex_anchors(pattern);
            format!("/{translated}/{flags}")
        }
    }
}

/// Walk a Ruby regex source, replacing string-boundary anchors with
/// JS line-boundary anchors. Handles `\\` escapes so a literal
/// backslash followed by `A`/`z`/`Z` doesn't get clobbered.
fn translate_ruby_regex_anchors(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('A') => { out.push('^'); chars.next(); }
                Some('z') | Some('Z') => { out.push('$'); chars.next(); }
                Some('\\') => { out.push('\\'); out.push('\\'); chars.next(); }
                _ => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
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
