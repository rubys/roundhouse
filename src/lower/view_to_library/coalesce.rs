//! Coalesce adjacent `io << x` appends in a lowered view body into a
//! single append driven by a `StringInterp`. The view lowerer already
//! emits each ERB chunk as its own `io << ...` step (literal text,
//! escaped interpolation, helper call, …); successive statements are
//! frequently all appends with no intervening control flow. Collapsing
//! them shrinks the IR the typer + every per-target emitter walks,
//! and gives rust2/go2 one wider write instead of many tiny ones.
//!
//! Recursion: control-flow nodes break a run, but their inner bodies
//! are themselves coalesced (then/else branches, each-block bodies,
//! nested Seqs). The coalescer is a pure IR-to-IR rewrite — no typing,
//! no target awareness.

use crate::expr::{Expr, ExprNode, InterpPart, IrHint, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

/// Coalesce a flat list of view-body statements. Runs of `io << arg`
/// (recognized by accumulator name + `<<` method) are merged into one
/// `io << StringInterp { … }`. Single-statement runs pass through
/// unchanged. Non-append statements break the run and are recursed
/// into so nested control-flow bodies get the same treatment.
pub(super) fn coalesce_appends(stmts: Vec<Expr>, accumulator: &str) -> Vec<Expr> {
    let mut out: Vec<Expr> = Vec::new();
    let mut run: Vec<Expr> = Vec::new();
    for stmt in stmts {
        if let Some(arg) = take_append_arg(&stmt, accumulator) {
            run.push(arg);
        } else {
            flush_run(&mut out, &mut run, accumulator);
            out.push(recurse(stmt, accumulator));
        }
    }
    flush_run(&mut out, &mut run, accumulator);
    out
}

/// If `e` is an `io << arg` append against the named accumulator,
/// return Some(arg). Recognition is structural: Send with recv = Var
/// matching the accumulator name, method `<<`, exactly one arg, no
/// block. We intentionally do not require `IrHint::StringBuilderAppend`
/// — TODO appends and other lowerer-synthesized appends share the
/// shape and benefit from coalescing the same way.
fn take_append_arg(e: &Expr, accumulator: &str) -> Option<Expr> {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*e.node else {
        return None;
    };
    if method.as_str() != "<<" || args.len() != 1 {
        return None;
    }
    let ExprNode::Var { name, .. } = &*recv.node else {
        return None;
    };
    if name.as_str() != accumulator {
        return None;
    }
    Some(args[0].clone())
}

fn flush_run(out: &mut Vec<Expr>, run: &mut Vec<Expr>, accumulator: &str) {
    let collected = std::mem::take(run);
    match collected.len() {
        0 => {}
        1 => {
            let arg = collected.into_iter().next().unwrap();
            out.push(make_append(arg, accumulator));
        }
        _ => {
            let merged = build_merged_arg(collected);
            out.push(make_append(merged, accumulator));
        }
    }
}

/// Combine a list of append args into a single expression. Strings
/// become `Text` parts, existing StringInterps are splatted, anything
/// else becomes an `Expr` part. Adjacent Texts collapse. Degenerate
/// outputs (one Text → Lit::Str, one Expr → bare expr) stay scalar
/// so emitters keep their literal-arg fast paths.
fn build_merged_arg(args: Vec<Expr>) -> Expr {
    let mut parts: Vec<InterpPart> = Vec::new();
    for arg in args {
        match *arg.node {
            ExprNode::Lit { value: Literal::Str { value } } => {
                push_text(&mut parts, value);
            }
            ExprNode::StringInterp { parts: inner } => {
                for p in inner {
                    match p {
                        InterpPart::Text { value } => push_text(&mut parts, value),
                        InterpPart::Expr { expr } => parts.push(InterpPart::Expr { expr }),
                    }
                }
            }
            _ => {
                parts.push(InterpPart::Expr { expr: arg });
            }
        }
    }
    match parts.len() {
        1 => match parts.into_iter().next().unwrap() {
            InterpPart::Text { value } => Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Str { value } },
            ),
            InterpPart::Expr { expr } => expr,
        },
        _ => Expr::new(Span::synthetic(), ExprNode::StringInterp { parts }),
    }
}

fn push_text(parts: &mut Vec<InterpPart>, value: String) {
    if value.is_empty() {
        return;
    }
    if let Some(InterpPart::Text { value: tail }) = parts.last_mut() {
        tail.push_str(&value);
        return;
    }
    parts.push(InterpPart::Text { value });
}

/// Rebuild `io << arg` with the `StringBuilderAppend` hint so emitters
/// that key off it (go2 → `WriteString`) still recognize the merged
/// append. We synthesize here rather than reusing
/// `accumulator_append_call` because the coalescer is ViewCtx-free.
fn make_append(arg: Expr, accumulator: &str) -> Expr {
    let recv = Expr::new(
        Span::synthetic(),
        ExprNode::Var { id: VarId(0), name: Symbol::from(accumulator) },
    );
    let mut e = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("<<"),
            args: vec![arg],
            block: None,
            parenthesized: false,
        },
    );
    e.hint = Some(IrHint::StringBuilderAppend);
    e
}

/// Walk into a non-append statement and coalesce any append runs
/// nested inside it (then/else branches, block bodies, Seq children).
/// Other shapes pass through unchanged. We mutate the boxed node in
/// place so all `Expr` annotations (ty, hint, decisions, span,
/// leading_blank_line, diagnostic, effects) survive untouched.
fn recurse(mut stmt: Expr, accumulator: &str) -> Expr {
    let placeholder = ExprNode::Lit { value: Literal::Nil };
    let node = std::mem::replace(&mut *stmt.node, placeholder);
    *stmt.node = match node {
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: coalesce_appends(exprs, accumulator),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond,
            then_branch: recurse(then_branch, accumulator),
            else_branch: recurse(else_branch, accumulator),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => {
            let block = block.map(|b| recurse(b, accumulator));
            ExprNode::Send { recv, method, args, block, parenthesized }
        }
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params,
            block_param,
            body: recurse(body, accumulator),
            block_style,
        },
        other => other,
    };
    stmt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{Expr, ExprNode, Literal};
    use crate::ident::{Symbol, VarId};
    use crate::span::Span;

    fn lit_str(s: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Str { value: s.into() } },
        )
    }

    fn var(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: Symbol::from(name) },
        )
    }

    fn append(arg: Expr) -> Expr {
        make_append(arg, "io")
    }

    fn body_of_append(e: &Expr) -> &Expr {
        let ExprNode::Send { args, .. } = &*e.node else { panic!("not Send") };
        &args[0]
    }

    #[test]
    fn three_text_appends_collapse_to_one() {
        let stmts = vec![append(lit_str("a")), append(lit_str("b")), append(lit_str("c"))];
        let out = coalesce_appends(stmts, "io");
        assert_eq!(out.len(), 1);
        match &*body_of_append(&out[0]).node {
            ExprNode::Lit { value: Literal::Str { value } } => assert_eq!(value, "abc"),
            other => panic!("expected merged Lit::Str, got {other:?}"),
        }
    }

    #[test]
    fn text_expr_text_becomes_string_interp() {
        let stmts = vec![append(lit_str("<h1>")), append(var("title")), append(lit_str("</h1>"))];
        let out = coalesce_appends(stmts, "io");
        assert_eq!(out.len(), 1);
        match &*body_of_append(&out[0]).node {
            ExprNode::StringInterp { parts } => {
                assert_eq!(parts.len(), 3);
                assert!(matches!(parts[0], InterpPart::Text { ref value } if value == "<h1>"));
                assert!(matches!(parts[1], InterpPart::Expr { .. }));
                assert!(matches!(parts[2], InterpPart::Text { ref value } if value == "</h1>"));
            }
            other => panic!("expected StringInterp, got {other:?}"),
        }
    }

    #[test]
    fn single_append_passes_through_unchanged() {
        let stmts = vec![append(lit_str("solo"))];
        let out = coalesce_appends(stmts, "io");
        assert_eq!(out.len(), 1);
        match &*body_of_append(&out[0]).node {
            ExprNode::Lit { value: Literal::Str { value } } => assert_eq!(value, "solo"),
            other => panic!("expected Lit::Str, got {other:?}"),
        }
    }

    #[test]
    fn non_append_breaks_run() {
        let other = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("ViewHelpers")] },
                )),
                method: Symbol::from("content_for_set"),
                args: vec![lit_str(":title"), lit_str("hi")],
                block: None,
                parenthesized: false,
            },
        );
        let stmts = vec![
            append(lit_str("a")),
            append(lit_str("b")),
            other,
            append(lit_str("c")),
            append(lit_str("d")),
        ];
        let out = coalesce_appends(stmts, "io");
        assert_eq!(out.len(), 3);
        match &*body_of_append(&out[0]).node {
            ExprNode::Lit { value: Literal::Str { value } } => assert_eq!(value, "ab"),
            _ => panic!(),
        }
        match &*body_of_append(&out[2]).node {
            ExprNode::Lit { value: Literal::Str { value } } => assert_eq!(value, "cd"),
            _ => panic!(),
        }
    }

    #[test]
    fn splats_existing_string_interp() {
        let interp = Expr::new(
            Span::synthetic(),
            ExprNode::StringInterp {
                parts: vec![
                    InterpPart::Text { value: "x".into() },
                    InterpPart::Expr { expr: var("y") },
                ],
            },
        );
        let stmts = vec![append(lit_str("<p>")), append(interp), append(lit_str("</p>"))];
        let out = coalesce_appends(stmts, "io");
        assert_eq!(out.len(), 1);
        match &*body_of_append(&out[0]).node {
            ExprNode::StringInterp { parts } => {
                assert_eq!(parts.len(), 3);
                assert!(matches!(parts[0], InterpPart::Text { ref value } if value == "<p>x"));
                assert!(matches!(parts[1], InterpPart::Expr { .. }));
                assert!(matches!(parts[2], InterpPart::Text { ref value } if value == "</p>"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn recurses_into_if_branches() {
        let then_branch = Expr::new(
            Span::synthetic(),
            ExprNode::Seq {
                exprs: vec![append(lit_str("yes-")), append(lit_str("path"))],
            },
        );
        let else_branch = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil });
        let if_expr = Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: var("flag"),
                then_branch,
                else_branch,
            },
        );
        let out = coalesce_appends(vec![if_expr], "io");
        assert_eq!(out.len(), 1);
        let ExprNode::If { then_branch, .. } = &*out[0].node else { panic!() };
        let ExprNode::Seq { exprs } = &*then_branch.node else { panic!() };
        assert_eq!(exprs.len(), 1);
        match &*body_of_append(&exprs[0]).node {
            ExprNode::Lit { value: Literal::Str { value } } => assert_eq!(value, "yes-path"),
            _ => panic!(),
        }
    }
}
