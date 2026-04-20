//! Controller body-normalization lowering passes.
//!
//! Target-neutral unit tests for the pre-emit passes in
//! `src/lower/controller.rs` that reshape an action body into a form
//! every emitter can walk without re-deriving Rails semantics. Each
//! pass is tested here for the IR shape it produces; per-target
//! rendering of that IR lives in the emit tests.

use roundhouse::expr::{Expr, ExprNode, Literal};
use roundhouse::ident::Symbol;
use roundhouse::lower::{has_toplevel_terminal, synthesize_implicit_render};
use roundhouse::span::Span;

fn lit_int(n: i64) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Int { value: n } },
    )
}

fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: false,
        },
    )
}

fn seq(exprs: Vec<Expr>) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Seq { exprs })
}

fn if_expr(cond: Expr, then_: Expr, else_: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond,
            then_branch: then_,
            else_branch: else_,
        },
    )
}

/// True when the last Send in a Seq body matches the given method name.
fn last_send_is(body: &Expr, method: &str) -> bool {
    let ExprNode::Seq { exprs } = &*body.node else { return false };
    let Some(last) = exprs.last() else { return false };
    let ExprNode::Send { method: m, .. } = &*last.node else { return false };
    m.as_str() == method
}

#[test]
fn has_toplevel_terminal_detects_bare_render() {
    // `render :show` as the sole body expr — terminal.
    let body = send(None, "render", vec![lit_int(1)]);
    assert!(has_toplevel_terminal(&body));
}

#[test]
fn has_toplevel_terminal_detects_terminal_in_seq_tail() {
    // `@x = 1; render :foo` — seq whose last expr is render.
    let body = seq(vec![lit_int(1), send(None, "render", vec![])]);
    assert!(has_toplevel_terminal(&body));
}

#[test]
fn has_toplevel_terminal_requires_both_if_branches() {
    // `if c; render :a; else; nil; end` — else branch has no
    // terminal, so the overall body can fall through.
    let partial = if_expr(
        lit_int(1),
        send(None, "render", vec![]),
        lit_int(0),
    );
    assert!(!has_toplevel_terminal(&partial));

    // Both branches terminate — full coverage.
    let full = if_expr(
        lit_int(1),
        send(None, "render", vec![]),
        send(None, "redirect_to", vec![]),
    );
    assert!(has_toplevel_terminal(&full));
}

#[test]
fn has_toplevel_terminal_recognizes_respond_to_block() {
    // `respond_to do |format| ... end` — the block's branches
    // contain terminals. The pass treats the `respond_to` call
    // itself as terminal-bearing because every emitter's SendKind
    // render table expands it into per-format terminals.
    let empty_block = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![],
            block_param: None,
            body: seq(vec![]),
            block_style: roundhouse::expr::BlockStyle::Do,
        },
    );
    let respond_to = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from("respond_to"),
            args: vec![],
            block: Some(empty_block),
            parenthesized: false,
        },
    );
    assert!(has_toplevel_terminal(&respond_to));
}

#[test]
fn has_toplevel_terminal_rejects_plain_assign() {
    // `@x = 1` — no terminal, Rails falls through to implicit render.
    let body = lit_int(42);
    assert!(!has_toplevel_terminal(&body));
}

#[test]
fn synthesize_implicit_render_appends_when_missing() {
    // Body without a terminal gets `render :action_name` appended.
    let body = lit_int(42);
    let out = synthesize_implicit_render(&body, "show");
    assert!(last_send_is(&out, "render"), "expected render terminal; got shape: {:#?}", out.node);
    // Seq preserves the original expr as the first statement.
    let ExprNode::Seq { exprs } = &*out.node else {
        panic!("expected Seq, got {:?}", out.node);
    };
    assert_eq!(exprs.len(), 2);
}

#[test]
fn synthesize_implicit_render_is_noop_when_terminal_present() {
    // Body already terminates — pass returns a clone, not a double-render.
    let body = send(None, "render", vec![lit_int(1)]);
    let out = synthesize_implicit_render(&body, "show");
    // No Seq wrapping — the original Send shape is preserved.
    assert!(
        matches!(&*out.node, ExprNode::Send { .. }),
        "pass should be a no-op; got {:?}",
        out.node,
    );
}

#[test]
fn synthesize_implicit_render_uses_action_name_as_view_symbol() {
    // `show` → `render :show`; `headline` → `render :headline`.
    // Verifies the appended Send's first arg is a symbol literal
    // matching the action name.
    let body = lit_int(42);
    let out = synthesize_implicit_render(&body, "headline");
    let ExprNode::Seq { exprs } = &*out.node else { panic!() };
    let tail = exprs.last().unwrap();
    let ExprNode::Send { args, .. } = &*tail.node else { panic!() };
    let ExprNode::Lit { value: Literal::Sym { value } } = &*args[0].node else {
        panic!("expected symbol arg, got {:?}", args[0].node);
    };
    assert_eq!(value.as_str(), "headline");
}
