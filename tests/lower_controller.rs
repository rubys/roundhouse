//! Controller body-normalization lowering passes.
//!
//! Target-neutral unit tests for the pre-emit passes in
//! `src/lower/controller.rs` that reshape an action body into a form
//! every emitter can walk without re-deriving Rails semantics. Each
//! pass is tested here for the IR shape it produces; per-target
//! rendering of that IR lives in the emit tests.

use roundhouse::expr::{Expr, ExprNode, Literal};
use roundhouse::ident::Symbol;
use roundhouse::lower::{
    has_toplevel_terminal, resolve_before_actions, synthesize_implicit_render,
    unwrap_respond_to,
};
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

// -- unwrap_respond_to ----------------------------------------------

fn lambda(body: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![],
            block_param: None,
            body,
            block_style: roundhouse::expr::BlockStyle::Do,
        },
    )
}

fn send_with_block(recv: Option<Expr>, method: &str, block: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args: vec![],
            block: Some(block),
            parenthesized: false,
        },
    )
}

/// Build `format.<branch> { body }` — the canonical respond_to
/// inner-call shape. The `format` receiver must be a Var (the
/// block param introduced by `do |format|`), not a Send —
/// `is_format_binding` specifically pattern-matches the Var form.
fn format_call(branch: &str, body: Expr) -> Expr {
    let format_recv = Expr::new(
        Span::synthetic(),
        ExprNode::Var {
            id: roundhouse::ident::VarId(0),
            name: Symbol::from("format"),
        },
    );
    send_with_block(Some(format_recv), branch, lambda(body))
}

/// Build `respond_to do |format| { body } end`.
fn respond_to(body: Expr) -> Expr {
    send_with_block(None, "respond_to", lambda(body))
}

#[test]
fn unwrap_respond_to_flattens_simple_html_json_pair() {
    // `respond_to { format.html { redirect_to(:x) }; format.json { head } }`
    // → `redirect_to(:x)` (html kept, json dropped).
    let body = seq(vec![
        format_call("html", send(None, "redirect_to", vec![lit_int(1)])),
        format_call("json", send(None, "head", vec![])),
    ]);
    let out = unwrap_respond_to(&respond_to(body));
    // The Seq wrapping a single expr collapses — walker sees a bare Send.
    let ExprNode::Send { method, .. } = &*out.node else {
        panic!("expected bare Send, got {:?}", out.node);
    };
    assert_eq!(method.as_str(), "redirect_to");
}

#[test]
fn unwrap_respond_to_drops_json_only_branch() {
    // `respond_to { format.json { ... } }` with no html — flattens
    // to an empty Seq. Downstream `synthesize_implicit_render` will
    // then append a terminal.
    let body = seq(vec![format_call("json", send(None, "head", vec![]))]);
    let out = unwrap_respond_to(&respond_to(body));
    match &*out.node {
        ExprNode::Seq { exprs } => assert!(exprs.is_empty()),
        other => panic!("expected empty Seq, got {:?}", other),
    }
}

#[test]
fn unwrap_respond_to_preserves_if_branching() {
    // `respond_to { if cond; format.html { a1 }; format.json { b1 }
    //                       else;  format.html { a2 }; format.json { b2 } end }`
    // → `if cond; a1 else a2 end` — the if wrapper stays, each
    // branch is replaced by its html contents.
    let then_pair = seq(vec![
        format_call("html", send(None, "render", vec![lit_int(1)])),
        format_call("json", send(None, "head", vec![])),
    ]);
    let else_pair = seq(vec![
        format_call("html", send(None, "render", vec![lit_int(2)])),
        format_call("json", send(None, "head", vec![])),
    ]);
    let branched = if_expr(lit_int(0), then_pair, else_pair);
    let out = unwrap_respond_to(&respond_to(branched));
    let ExprNode::If { then_branch, else_branch, .. } = &*out.node else {
        panic!("expected If, got {:?}", out.node);
    };
    // Each branch has been collapsed from a format pair to the html
    // branch's render call.
    let ExprNode::Send { method: m1, .. } = &*then_branch.node else {
        panic!("expected Send in then, got {:?}", then_branch.node);
    };
    assert_eq!(m1.as_str(), "render");
    let ExprNode::Send { method: m2, .. } = &*else_branch.node else {
        panic!("expected Send in else, got {:?}", else_branch.node);
    };
    assert_eq!(m2.as_str(), "render");
}

#[test]
fn unwrap_respond_to_is_noop_without_respond_to_call() {
    // A body with no respond_to — the pass leaves it structurally
    // equivalent (recursive walk reconstructs Nodes but same shape).
    let body = seq(vec![
        send(None, "redirect_to", vec![lit_int(1)]),
    ]);
    let out = unwrap_respond_to(&body);
    assert!(matches!(&*out.node, ExprNode::Seq { .. }));
}

// -- resolve_before_actions -----------------------------------------

fn action(name: &str, body: Expr) -> roundhouse::dialect::Action {
    use roundhouse::{EffectSet, RenderTarget, Row};
    roundhouse::dialect::Action {
        name: Symbol::from(name),
        params: Row::closed(),
        body,
        renders: RenderTarget::Inferred,
        effects: EffectSet::pure(),
    }
}

fn action_item(a: roundhouse::dialect::Action) -> roundhouse::dialect::ControllerBodyItem {
    roundhouse::dialect::ControllerBodyItem::Action {
        action: a,
        leading_comments: vec![],
        leading_blank_line: false,
    }
}

fn filter_item(
    target: &str,
    only: &[&str],
) -> roundhouse::dialect::ControllerBodyItem {
    use roundhouse::dialect::{Filter, FilterKind};
    roundhouse::dialect::ControllerBodyItem::Filter {
        filter: Filter {
            kind: FilterKind::Before,
            target: Symbol::from(target),
            only: only.iter().map(|s| Symbol::from(*s)).collect(),
            except: vec![],
            only_style: roundhouse::expr::ArrayStyle::default(),
            except_style: roundhouse::expr::ArrayStyle::default(),
        },
        leading_comments: vec![],
        leading_blank_line: false,
    }
}

fn controller(
    items: Vec<roundhouse::dialect::ControllerBodyItem>,
) -> roundhouse::dialect::Controller {
    use roundhouse::ClassId;
    roundhouse::dialect::Controller {
        name: ClassId(Symbol::from("ArticlesController")),
        parent: None,
        body: items,
    }
}

#[test]
fn resolve_before_actions_prepends_callback_body() {
    // `before_action :set_article, only: [:show]` plus a private
    // `def set_article; @a = 1; end` → show's body gets the
    // callback body prepended.
    let ctrl = controller(vec![
        filter_item("set_article", &["show"]),
        action_item(action("set_article", lit_int(1))),
        action_item(action("show", lit_int(2))),
    ]);
    let original_show = lit_int(2);
    let out = resolve_before_actions(&ctrl, "show", &original_show);
    // Result is a Seq: [callback_body, original_body].
    let ExprNode::Seq { exprs } = &*out.node else {
        panic!("expected Seq, got {:?}", out.node);
    };
    assert_eq!(exprs.len(), 2);
    // First statement is the callback body (lit 1).
    let ExprNode::Lit { value: Literal::Int { value } } = &*exprs[0].node else {
        panic!("expected int lit, got {:?}", exprs[0].node);
    };
    assert_eq!(*value, 1);
    // Second is the original (lit 2).
    let ExprNode::Lit { value: Literal::Int { value } } = &*exprs[1].node else {
        panic!("expected int lit, got {:?}", exprs[1].node);
    };
    assert_eq!(*value, 2);
}

#[test]
fn resolve_before_actions_skips_non_matching_actions() {
    // `before_action :set_article, only: [:show]` — create is not
    // in the only list, so its body is untouched.
    let ctrl = controller(vec![
        filter_item("set_article", &["show"]),
        action_item(action("set_article", lit_int(1))),
        action_item(action("create", lit_int(99))),
    ]);
    let create_body = lit_int(99);
    let out = resolve_before_actions(&ctrl, "create", &create_body);
    // No change — still the single lit.
    assert!(matches!(&*out.node, ExprNode::Lit { .. }));
}

#[test]
fn resolve_before_actions_drops_unresolvable_callback() {
    // `before_action :authenticate_user` with no matching private
    // method (it's inherited from ApplicationController) — drop
    // silently; the action body is returned unchanged.
    let ctrl = controller(vec![
        filter_item("authenticate_user", &[]),
        action_item(action("index", lit_int(7))),
    ]);
    let index_body = lit_int(7);
    let out = resolve_before_actions(&ctrl, "index", &index_body);
    assert!(matches!(&*out.node, ExprNode::Lit { .. }));
}

#[test]
fn resolve_before_actions_respects_except_list() {
    // `before_action :set_x, except: [:index]` — applies to show
    // but not to index.
    use roundhouse::dialect::{ControllerBodyItem, Filter, FilterKind};
    let filter_with_except = ControllerBodyItem::Filter {
        filter: Filter {
            kind: FilterKind::Before,
            target: Symbol::from("set_x"),
            only: vec![],
            except: vec![Symbol::from("index")],
            only_style: roundhouse::expr::ArrayStyle::default(),
            except_style: roundhouse::expr::ArrayStyle::default(),
        },
        leading_comments: vec![],
        leading_blank_line: false,
    };
    let ctrl = controller(vec![
        filter_with_except,
        action_item(action("set_x", lit_int(1))),
        action_item(action("index", lit_int(2))),
        action_item(action("show", lit_int(3))),
    ]);
    // index: except-list excludes it → untouched.
    let out_index = resolve_before_actions(&ctrl, "index", &lit_int(2));
    assert!(matches!(&*out_index.node, ExprNode::Lit { .. }));
    // show: not excluded → prepended.
    let out_show = resolve_before_actions(&ctrl, "show", &lit_int(3));
    assert!(matches!(&*out_show.node, ExprNode::Seq { .. }));
}

#[test]
fn resolve_before_actions_prepends_in_declaration_order() {
    // Two applicable before_actions — both prepend, first-declared
    // ends up at the front.
    let ctrl = controller(vec![
        filter_item("cb_a", &["show"]),
        filter_item("cb_b", &["show"]),
        action_item(action("cb_a", lit_int(10))),
        action_item(action("cb_b", lit_int(20))),
        action_item(action("show", lit_int(30))),
    ]);
    let out = resolve_before_actions(&ctrl, "show", &lit_int(30));
    let ExprNode::Seq { exprs } = &*out.node else {
        panic!("expected Seq, got {:?}", out.node);
    };
    assert_eq!(exprs.len(), 3);
    // Order: cb_a (10), cb_b (20), original (30).
    let values: Vec<i64> = exprs
        .iter()
        .map(|e| match &*e.node {
            ExprNode::Lit { value: Literal::Int { value } } => *value,
            other => panic!("expected int lit, got {:?}", other),
        })
        .collect();
    assert_eq!(values, vec![10, 20, 30]);
}

#[test]
fn unwrap_respond_to_recurses_through_non_respond_to_structure() {
    // `if cond; respond_to { format.html { a } } else; b end` —
    // the respond_to inside the then-branch still unwraps.
    let nested_then = respond_to(seq(vec![
        format_call("html", send(None, "render", vec![lit_int(1)])),
        format_call("json", send(None, "head", vec![])),
    ]));
    let body = if_expr(lit_int(0), nested_then, send(None, "render", vec![lit_int(2)]));
    let out = unwrap_respond_to(&body);
    let ExprNode::If { then_branch, .. } = &*out.node else {
        panic!("expected If, got {:?}", out.node);
    };
    // then-branch's respond_to collapsed to a Send.
    let ExprNode::Send { method, .. } = &*then_branch.node else {
        panic!("expected Send in then, got {:?}", then_branch.node);
    };
    assert_eq!(method.as_str(), "render");
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
