//! Rewrite `assert_*`/`refute_*` Sends inside test method bodies to
//! inline `raise` expressions. Spinel doesn't ship `Minitest::
//! Assertions`, so source `assert_equal a, b` would dispatch through
//! a vacuous body and silently pass — see
//! `project_spinel_assertions_vacuous.md`. After this pass, assertion
//! failures actually raise, the spinel binary exits nonzero, and
//! `make spinel-test` consumes it as a fail signal.
//!
//! Patterns rewritten at the call site:
//!   - `assert_equal a, b`              → `raise "…" if a != b`
//!   - `assert v`                       → `raise "…" if !v`
//!   - `assert_not v` / `refute v`      → `raise "…" if v`
//!   - `assert_nil v`                   → `raise "…" if !v.nil?`
//!   - `assert_not_nil v` / `refute_nil v` → `raise "…" if v.nil?`
//!   - `assert_empty c`                 → `raise "…" if !c.empty?`
//!   - `assert_not_empty c` / `refute_empty c` → `raise "…" if c.empty?`
//!   - `assert_includes c, x`           → `raise "…" if !c.include?(x)`
//!   - `refute_includes c, x`           → `raise "…" if c.include?(x)`
//!   - `assert_kind_of K, x`            → `raise "…" if !x.is_a?(K)`
//!   - `assert_instance_of K, x`        → `raise "…" if !x.instance_of?(K)`
//!   (`assert_match` and `assert_operator` deliberately not lowered —
//!   nilable-value handling and Class-subclass `<` checks aren't
//!   cross-target-safe. Each target's test_helper handles them; Ruby's
//!   TestBase provides both.)
//!   - `assert_predicate o, :sym`       → `raise "…" if !o.sym` (sym from Symbol literal)
//!   - `assert_difference("X.m"[, d]) { body }` → before/after capture
//!   - `assert_no_difference("X.m") { body }`   → same, delta 0
//!
//! Assertions that depend on shared dispatch state (`assert_response`,
//! `assert_select`, `assert_redirected_to`) stay as method calls;
//! their helper bodies in `runtime/spinel/test/test_helper.rb` raise
//! directly rather than delegating to a vacuous `assert`.

use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal, RescueClause};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

/// Top-level entry. Walk `body` bottom-up, rewriting recognized
/// assertion Sends to inline raise statements. Unrecognized Sends
/// pass through unchanged.
pub fn inline_assertions(body: &Expr) -> Expr {
    map_expr(body)
}

fn map_expr(e: &Expr) -> Expr {
    // Recurse first (bottom-up). After children are rewritten, give
    // the current node a chance to rewrite at this level. For Seq
    // nodes, flatten any inlined Seq replacements so downstream
    // emitters see a single flat statement list.
    let inner = match &*e.node {
        ExprNode::Seq { exprs } => {
            let mut out: Vec<Expr> = Vec::with_capacity(exprs.len());
            for child in exprs {
                let mapped = map_expr(child);
                if let ExprNode::Seq { exprs: nested } = &*mapped.node {
                    // Splice — a per-statement assertion rewrite that
                    // produces multiple statements (e.g. assert_difference)
                    // returns a Seq; flatten into the surrounding Seq so
                    // emit produces a clean statement list rather than a
                    // nested begin block.
                    out.extend(nested.iter().cloned());
                } else {
                    out.push(mapped);
                }
            }
            return Expr::new(e.span, ExprNode::Seq { exprs: out });
        }
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: map_expr(cond),
            then_branch: map_expr(then_branch),
            else_branch: map_expr(else_branch),
        },
        ExprNode::Case { scrutinee, arms } => ExprNode::Case {
            scrutinee: map_expr(scrutinee),
            arms: arms
                .iter()
                .map(|a| crate::expr::Arm {
                    pattern: a.pattern.clone(),
                    guard: a.guard.as_ref().map(map_expr),
                    body: map_expr(&a.body),
                })
                .collect(),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(map_expr),
            method: method.clone(),
            args: args.iter().map(map_expr).collect(),
            block: block.as_ref().map(map_expr),
            parenthesized: *parenthesized,
        },
        ExprNode::Apply { fun, args, block } => ExprNode::Apply {
            fun: map_expr(fun),
            args: args.iter().map(map_expr).collect(),
            block: block.as_ref().map(map_expr),
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: map_expr(body),
            block_style: *block_style,
        },
        ExprNode::Assign { target, value } => ExprNode::Assign {
            target: match target {
                LValue::Attr { recv, name } => LValue::Attr {
                    recv: map_expr(recv),
                    name: name.clone(),
                },
                LValue::Index { recv, index } => LValue::Index {
                    recv: map_expr(recv),
                    index: map_expr(index),
                },
                other => other.clone(),
            },
            value: map_expr(value),
        },
        ExprNode::Let { id, name, value, body } => ExprNode::Let {
            id: *id,
            name: name.clone(),
            value: map_expr(value),
            body: map_expr(body),
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: map_expr(left),
            right: map_expr(right),
        },
        ExprNode::Return { value } => ExprNode::Return { value: map_expr(value) },
        ExprNode::Raise { value } => ExprNode::Raise { value: map_expr(value) },
        ExprNode::Yield { args } => ExprNode::Yield {
            args: args.iter().map(map_expr).collect(),
        },
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, implicit } => {
            ExprNode::BeginRescue {
                body: map_expr(body),
                rescues: rescues
                    .iter()
                    .map(|rc| RescueClause {
                        classes: rc.classes.iter().map(map_expr).collect(),
                        binding: rc.binding.clone(),
                        body: map_expr(&rc.body),
                    })
                    .collect(),
                else_branch: else_branch.as_ref().map(map_expr),
                ensure: ensure.as_ref().map(map_expr),
                implicit: *implicit,
            }
        }
        ExprNode::Hash { entries, kwargs } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (map_expr(k), map_expr(v)))
                .collect(),
            kwargs: *kwargs,
        },
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(map_expr).collect(),
            style: *style,
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Text { value } => InterpPart::Text { value: value.clone() },
                    InterpPart::Expr { expr } => InterpPart::Expr { expr: map_expr(expr) },
                })
                .collect(),
        },
        _ => return rewrite_send(e).unwrap_or_else(|| e.clone()),
    };
    let new_e = Expr {
        span: e.span,
        node: Box::new(inner),
        ty: e.ty.clone(),
        effects: e.effects.clone(),
        leading_blank_line: e.leading_blank_line,
        diagnostic: e.diagnostic.clone(),
        str_coercion: e.str_coercion,
        hint: e.hint,
    };
    rewrite_send(&new_e).unwrap_or(new_e)
}

/// Rewrite a bare-receiver `assert_*`/`refute_*` Send into an inline
/// raise expression. Returns None for non-assertion Sends so the
/// caller passes them through unchanged.
fn rewrite_send(e: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv, method, args, block, .. } = &*e.node else {
        return None;
    };
    // Bare-method calls inside `def test_*` bodies arrive here in two
    // shapes: pre-typing `Send { recv: None }`, and post-typing
    // `Send { recv: Some(SelfRef) }` (the body-typer wraps bare names
    // with an explicit self receiver for dispatch resolution). Accept
    // both — we're rewriting at the call shape, not the dispatch.
    match recv {
        None => {}
        Some(r) if matches!(&*r.node, ExprNode::SelfRef) => {}
        _ => return None,
    }
    let span = e.span;
    match method.as_str() {
        "assert_equal" if args.len() >= 2 => {
            let expected = args[0].clone();
            let actual = args[1].clone();
            let msg = format!("assert_equal failed");
            Some(raise_if(
                span,
                send_method(span, expected, "!=", vec![actual]),
                msg,
            ))
        }
        "refute_equal" | "assert_not_equal" if args.len() >= 2 => {
            // Inverted assert_equal — raise *if* the values are equal.
            let a = args[0].clone();
            let b = args[1].clone();
            Some(raise_if(
                span,
                send_method(span, a, "==", vec![b]),
                "refute_equal failed".to_string(),
            ))
        }
        "assert" if args.len() == 1 => {
            let cond = args[0].clone();
            Some(raise_if(span, not_expr(span, cond), "assertion failed".to_string()))
        }
        "assert_not" | "refute" if args.len() == 1 => {
            let cond = args[0].clone();
            Some(raise_if(span, cond, "refute failed".to_string()))
        }
        "assert_nil" if args.len() == 1 => {
            let val = args[0].clone();
            Some(raise_if(
                span,
                not_expr(span, send_method(span, val, "nil?", vec![])),
                "assert_nil failed".to_string(),
            ))
        }
        "assert_not_nil" | "refute_nil" if args.len() == 1 => {
            let val = args[0].clone();
            Some(raise_if(
                span,
                send_method(span, val, "nil?", vec![]),
                "refute_nil failed".to_string(),
            ))
        }
        "assert_empty" if args.len() == 1 => {
            let coll = args[0].clone();
            Some(raise_if(
                span,
                not_expr(span, send_method(span, coll, "empty?", vec![])),
                "assert_empty failed".to_string(),
            ))
        }
        "assert_not_empty" | "refute_empty" if args.len() == 1 => {
            let coll = args[0].clone();
            Some(raise_if(
                span,
                send_method(span, coll, "empty?", vec![]),
                "refute_empty failed".to_string(),
            ))
        }
        "assert_includes" if args.len() >= 2 => {
            let coll = args[0].clone();
            let item = args[1].clone();
            Some(raise_if(
                span,
                not_expr(span, send_method(span, coll, "include?", vec![item])),
                "assert_includes failed".to_string(),
            ))
        }
        "refute_includes" if args.len() >= 2 => {
            let coll = args[0].clone();
            let item = args[1].clone();
            Some(raise_if(
                span,
                send_method(span, coll, "include?", vec![item]),
                "refute_includes failed".to_string(),
            ))
        }
        "assert_kind_of" if args.len() >= 2 => {
            // `assert_kind_of Klass, x` — order matches Minitest (class first).
            let klass = args[0].clone();
            let val = args[1].clone();
            Some(raise_if(
                span,
                not_expr(span, send_method(span, val, "is_a?", vec![klass])),
                "assert_kind_of failed".to_string(),
            ))
        }
        "assert_instance_of" if args.len() >= 2 => {
            let klass = args[0].clone();
            let val = args[1].clone();
            Some(raise_if(
                span,
                not_expr(span, send_method(span, val, "instance_of?", vec![klass])),
                "assert_instance_of failed".to_string(),
            ))
        }
        // `assert_match` and `assert_operator` deliberately NOT lowered:
        //   - `assert_match` needs nilable-value handling that differs
        //     per target (Ruby nil-safe `=~`, Crystal `String?` typing,
        //     TS regex API). Each target's test_helper provides the
        //     method natively; Ruby's TestBase provides one too.
        //   - `assert_operator` can use Class-subclass `<` checks which
        //     TS has no equivalent for. Same story — left as a Send.
        "assert_predicate" if args.len() >= 2 => {
            // `assert_predicate obj, :sym` — Symbol literal gives us the
            // method name at lowering time. Emit `obj.<sym>()` directly.
            let sym = match &*args[1].node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                _ => return None,
            };
            let obj = args[0].clone();
            Some(raise_if(
                span,
                not_expr(span, send_method(span, obj, &sym, vec![])),
                "assert_predicate failed".to_string(),
            ))
        }
        "refute_predicate" if args.len() >= 2 => {
            // Inverted assert_predicate — raise *if* the predicate holds.
            let sym = match &*args[1].node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                _ => return None,
            };
            let obj = args[0].clone();
            Some(raise_if(
                span,
                send_method(span, obj, &sym, vec![]),
                "refute_predicate failed".to_string(),
            ))
        }
        "assert_raises" if !args.is_empty() => {
            lower_assert_raises(span, args, block.as_ref())
        }
        "assert_difference" | "assert_no_difference" => {
            lower_difference(span, method.as_str(), args, block.as_ref())
        }
        _ => None,
    }
}

/// `assert_raises(ErrorClass) { body }` → Seq of:
///   __raised = nil
///   begin
///     <block body>
///   rescue ErrorClass => __caught
///     __raised = __caught
///   end
///   raise "assert_raises failed" if __raised.nil?
///   __raised
///
/// Matches Minitest's `assert_raises` contract: returns the caught
/// exception so callers can write `err = assert_raises(K) { ... };
/// assert_match(/foo/, err.message)`. The expected-class arg(s)
/// become the `rescue` classes. Non-block call bails — leaves the
/// Send in place for the typer to surface.
fn lower_assert_raises(span: Span, args: &[Expr], block: Option<&Expr>) -> Option<Expr> {
    let block_body = match block.map(|b| &*b.node) {
        Some(ExprNode::Lambda { body, .. }) => body.clone(),
        _ => return None,
    };
    let raised_name = Symbol::from("__raised");
    let caught_name = Symbol::from("__caught");
    let init = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: raised_name.clone() },
            value: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
        },
    );
    // Inside the rescue body: __raised = __caught
    let capture = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: raised_name.clone() },
            value: Expr::new(
                span,
                ExprNode::Var { id: VarId(0), name: caught_name.clone() },
            ),
        },
    );
    let begin_rescue = Expr::new(
        span,
        ExprNode::BeginRescue {
            body: block_body,
            rescues: vec![RescueClause {
                classes: args.to_vec(),
                binding: Some(caught_name),
                body: capture,
            }],
            else_branch: None,
            ensure: None,
            implicit: false,
        },
    );
    let check = raise_if(
        span,
        send_method(
            span,
            Expr::new(span, ExprNode::Var { id: VarId(0), name: raised_name.clone() }),
            "nil?",
            vec![],
        ),
        "assert_raises failed".to_string(),
    );
    // Final expr in the Seq is the caught exception — gives the
    // surrounding `err = assert_raises(...) { ... }` its value.
    let yield_caught = Expr::new(
        span,
        ExprNode::Var { id: VarId(0), name: raised_name },
    );
    // Wrap the Seq in an explicit `begin … end` so the whole thing
    // is a single expression in assignment contexts (`err =
    // assert_raises(...) { ... }`). Without the wrapper, the Seq
    // would flatten into the surrounding Seq (the
    // `ExprNode::Seq` flatten arm in map_expr), and the RHS of
    // `err = ...` would silently swallow only the first statement.
    let body = Expr::new(
        span,
        ExprNode::Seq { exprs: vec![init, begin_rescue, check, yield_caught] },
    );
    Some(Expr::new(
        span,
        ExprNode::BeginRescue {
            body,
            rescues: vec![],
            else_branch: None,
            ensure: None,
            implicit: false,
        },
    ))
}

/// `assert_difference("Article.count"[, delta]) { body }` → Seq of
/// before-capture, inlined block body, after-capture, raise-on-mismatch.
/// Parses the literal String argument into a Send Expression
/// (`Const(Article).count()`); returns None for non-literal first args
/// (caller leaves the unhandled Send in place — typer will surface it
/// downstream).
fn lower_difference(
    span: Span,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
) -> Option<Expr> {
    if args.is_empty() {
        return None;
    }
    let ExprNode::Lit { value: Literal::Str { value: expr_str } } = &*args[0].node else {
        return None;
    };
    let probe = parse_const_dot_method(expr_str, span)?;
    let delta: i64 = if method == "assert_no_difference" {
        0
    } else if args.len() >= 2 {
        match &*args[1].node {
            ExprNode::Lit { value: Literal::Int { value } } => *value,
            _ => return None,
        }
    } else {
        1
    };
    // Block body — unwrap the Lambda's inner body. If no block given,
    // there's nothing to do between captures; bail and let the typer
    // surface it.
    let block_body = match block.map(|b| &*b.node) {
        Some(ExprNode::Lambda { body, .. }) => body.clone(),
        _ => return None,
    };

    let before_name = Symbol::from("__diff_before");
    let after_name = Symbol::from("__diff_after");
    let before_assign = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: before_name.clone() },
            value: probe.clone(),
        },
    );
    let after_assign = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: after_name.clone() },
            value: probe.clone(),
        },
    );
    let actual_diff = send_method(
        span,
        Expr::new(span, ExprNode::Var { id: VarId(0), name: after_name }),
        "-",
        vec![Expr::new(span, ExprNode::Var { id: VarId(0), name: before_name })],
    );
    let mismatch = send_method(
        span,
        actual_diff,
        "!=",
        vec![Expr::new(span, ExprNode::Lit { value: Literal::Int { value: delta } })],
    );
    let msg = format!("{} didn't change by {}", expr_str, delta);
    let check = raise_if(span, mismatch, msg);

    // Inline block body — flatten if it's already a Seq so the
    // outer Seq stays single-level.
    let mut stmts: Vec<Expr> = vec![before_assign];
    match &*block_body.node {
        ExprNode::Seq { exprs } => stmts.extend(exprs.iter().cloned()),
        _ => stmts.push(block_body.clone()),
    }
    stmts.push(after_assign);
    stmts.push(check);
    Some(Expr::new(span, ExprNode::Seq { exprs: stmts }))
}

/// Parse a String literal of the form `"<Const>.<method>"` into an
/// equivalent Send IR. Returns None for any other shape — callers
/// leave the assertion in place so the typer surfaces a real error.
fn parse_const_dot_method(s: &str, span: Span) -> Option<Expr> {
    let (const_part, method_part) = s.split_once('.')?;
    let const_part = const_part.trim();
    let method_part = method_part.trim();
    if const_part.is_empty() || method_part.is_empty() {
        return None;
    }
    // First char must be uppercase for a Const; method must be a
    // bare identifier (no further dots, no parens, no args).
    if !const_part.chars().next().map_or(false, |c| c.is_ascii_uppercase()) {
        return None;
    }
    if !method_part
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '?')
    {
        return None;
    }
    let const_expr = Expr::new(
        span,
        ExprNode::Const { path: vec![Symbol::from(const_part)] },
    );
    Some(Expr::new(
        span,
        ExprNode::Send {
            recv: Some(const_expr),
            method: Symbol::from(method_part),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    ))
}

// ── small constructors ────────────────────────────────────────────

/// `cond ? do-nothing : raise "msg"` rendered via the If form the
/// Ruby emitter recognizes — single-statement then-branch with empty
/// else collapses to `then if cond`. So we put the Raise in the
/// then-branch and the inverted condition in `cond`.
fn raise_if(span: Span, cond: Expr, msg: String) -> Expr {
    let raise = Expr::new(
        span,
        ExprNode::Raise {
            value: Expr::new(span, ExprNode::Lit { value: Literal::Str { value: msg } }),
        },
    );
    Expr::new(
        span,
        ExprNode::If {
            cond,
            then_branch: raise,
            else_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
        },
    )
}

fn send_method(span: Span, recv: Expr, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

/// Synthesize `!cond` as the unary `!` Send the Ruby emitter
/// recognizes at `emit/ruby/expr.rs::378` (prefix form).
fn not_expr(span: Span, cond: Expr) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(cond),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}
