//! Action-body normalization — the pre-emit pipeline that reshapes
//! an action's body `Expr` so every target emitter can walk it
//! without per-target special cases:
//!
//!   1. Inline applicable `before_action` callback bodies
//!      (`actions::resolve_before_actions`).
//!   2. Flatten `respond_to { format.html {…} format.json {…} }` to
//!      just its HTML branch (`unwrap_respond_to`).
//!   3. Append a synthetic `render :<action>` when the body has no
//!      explicit response terminal (`synthesize_implicit_render`).
//!
//! Per-target ivar/params rewrites run AFTER this pipeline.

use crate::dialect::Controller;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;

use super::actions::resolve_before_actions;
use super::util::{is_format_binding, unwrap_lambda};

/// Flatten every `respond_to do |format| ... end` block in `expr`
/// into just its HTML branch: each `format.html { body }` is
/// replaced with its block body contents, and each `format.json
/// { … }` is dropped. Mirrors the Phase-4c convention already baked
/// into `SendKind::FormatJson` — JSON branches are deferred to a
/// later phase, so flattening to HTML-only is target-neutral and
/// lossless for the HTTP-HTML paths every emitter targets today.
///
/// Handles both scaffold shapes:
///   - Simple:    `respond_to { format.html { a }; format.json { b } }` → `a`
///   - Branched:  `respond_to { if c; format.html { a1 }; format.json { b1 }
///                              else;  format.html { a2 }; format.json { b2 } end }`
///                 → `if c; a1 else a2 end`
///
/// Walks recursively — nested `respond_to` calls (rare) flatten
/// bottom-up, and non-respond_to sub-expressions pass through their
/// structural variants so anything already at the top level is
/// preserved.
pub fn unwrap_respond_to(expr: &Expr) -> Expr {
    // Top-level `respond_to` with a block — replace the whole Send
    // with its flattened HTML-only body. This short-circuits the
    // structural recursion so we don't re-enter the respond_to's
    // Send/Lambda children via the generic path.
    if let ExprNode::Send { recv: None, method, block: Some(block), .. } = &*expr.node {
        if method.as_str() == "respond_to" {
            let lambda_body = unwrap_lambda(block);
            return flatten_respond_to_body(lambda_body);
        }
    }
    let new_node = match &*expr.node {
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(unwrap_respond_to).collect(),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: unwrap_respond_to(cond),
            then_branch: unwrap_respond_to(then_branch),
            else_branch: unwrap_respond_to(else_branch),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(unwrap_respond_to),
            method: method.clone(),
            args: args.iter().map(unwrap_respond_to).collect(),
            block: block.as_ref().map(unwrap_respond_to),
            parenthesized: *parenthesized,
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: unwrap_respond_to(left),
            right: unwrap_respond_to(right),
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: unwrap_respond_to(body),
            block_style: *block_style,
        },
        ExprNode::Assign { target, value } => {
            let new_target = match target {
                LValue::Attr { recv, name } => LValue::Attr {
                    recv: unwrap_respond_to(recv),
                    name: name.clone(),
                },
                LValue::Index { recv, index } => LValue::Index {
                    recv: unwrap_respond_to(recv),
                    index: unwrap_respond_to(index),
                },
                other => other.clone(),
            };
            ExprNode::Assign {
                target: new_target,
                value: unwrap_respond_to(value),
            }
        }
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(unwrap_respond_to).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, kwargs } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (unwrap_respond_to(k), unwrap_respond_to(v)))
                .collect(),
            kwargs: *kwargs,
        },
        // Literal, Const, Var, Ivar, Apply, Case, Yield, Raise,
        // RescueModifier, StringInterp, Let — no respond_to inside
        // today's fixtures; clone-verbatim. If future fixtures nest
        // respond_to inside these variants the recursion extends
        // here.
        other => other.clone(),
    };
    Expr {
        span: expr.span,
        node: Box::new(new_node),
        ty: expr.ty.clone(),
        effects: expr.effects.clone(),
        leading_blank_line: expr.leading_blank_line,
        diagnostic: expr.diagnostic.clone(),
    }
}

/// Flatten the immediate body of a `respond_to` block. Recognized
/// shapes at this level are `Seq` (the `format.html/.json` pair) and
/// `If` (conditional branching to different format pairs); anything
/// else is handled via `format_stmt_to_html_only` directly.
fn flatten_respond_to_body(body: &Expr) -> Expr {
    match &*body.node {
        ExprNode::Seq { exprs } => {
            let kept: Vec<Expr> =
                exprs.iter().filter_map(format_stmt_to_html_only).collect();
            // Single-element Seq → unwrap so the downstream walker
            // sees an ordinary Send instead of a Seq-of-one.
            match kept.len() {
                0 => Expr::new(body.span, ExprNode::Seq { exprs: vec![] }),
                1 => kept.into_iter().next().unwrap(),
                _ => Expr::new(body.span, ExprNode::Seq { exprs: kept }),
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => Expr::new(
            body.span,
            ExprNode::If {
                cond: unwrap_respond_to(cond),
                then_branch: flatten_respond_to_body(then_branch),
                else_branch: flatten_respond_to_body(else_branch),
            },
        ),
        // A single expression at respond_to-body scope — either a
        // lone `format.html`/`format.json`, or some unrelated shape
        // the pass leaves to the generic walker.
        _ => format_stmt_to_html_only(body).unwrap_or_else(|| unwrap_respond_to(body)),
    }
}

/// Map one statement inside a respond_to body:
/// - `format.html { body }` → `Some(body)` (the block contents are lifted out)
/// - `format.html` (no block) → `Some(empty Seq)` (the header-only form)
/// - `format.json { … }` → `None` (drop)
/// - anything else → `Some(unwrap_respond_to(e))` (keep, recursively flattened)
fn format_stmt_to_html_only(e: &Expr) -> Option<Expr> {
    if let ExprNode::Send { recv: Some(recv), method, block, .. } = &*e.node {
        if is_format_binding(recv) {
            match method.as_str() {
                "html" => {
                    let content = match block.as_ref() {
                        Some(b) => unwrap_lambda(b).clone(),
                        None => Expr::new(e.span, ExprNode::Seq { exprs: vec![] }),
                    };
                    return Some(unwrap_respond_to(&content));
                }
                "json" => return None,
                _ => {}
            }
        }
    }
    Some(unwrap_respond_to(e))
}

/// Append a synthesized `render :<action_name>` Send to `body` when
/// `body` has no top-level render / redirect_to / head terminal.
/// Encodes the Rails convention that an action falling off the end
/// renders its eponymous view.
///
/// Target-neutral — every emitter walking the result sees an explicit
/// terminal that `classify_controller_send` resolves to `Render`.
/// Before this pass, each scaffold template synthesized the terminal
/// ad-hoc at emit time; after, the walker path needs no special case.
pub fn synthesize_implicit_render(body: &Expr, action_name: &str) -> Expr {
    if has_toplevel_terminal(body) {
        return body.clone();
    }
    let render = render_symbol_send(action_name, body.span);
    append_statement(body, render)
}

/// True when `body` is guaranteed to hit a response-terminal
/// (`render` / `redirect_to` / `head` / `respond_to`) at its top
/// level — including every branch of the final if/else, since both
/// branches must terminate for the action to have a response. A
/// `respond_to` block counts as terminal because the emitter's
/// SendKind render table expands it into per-format terminals.
pub fn has_toplevel_terminal(body: &Expr) -> bool {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.last().map_or(false, has_toplevel_terminal),
        ExprNode::Send { recv: None, method, block, .. } => {
            matches!(method.as_str(), "render" | "redirect_to" | "head")
                || (method.as_str() == "respond_to" && block.is_some())
        }
        ExprNode::If { then_branch, else_branch, .. } => {
            has_toplevel_terminal(then_branch) && has_toplevel_terminal(else_branch)
        }
        _ => false,
    }
}

/// Build a synthetic `render :<name>` Send with the given span.
/// Used by `synthesize_implicit_render`; span is inherited from the
/// containing body so diagnostics / effect annotations point at a
/// meaningful location rather than a free-floating synthetic span.
fn render_symbol_send(action_name: &str, span: crate::span::Span) -> Expr {
    let sym = Expr::new(
        span,
        ExprNode::Lit {
            value: Literal::Sym { value: Symbol::from(action_name) },
        },
    );
    Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("render"),
            args: vec![sym],
            block: None,
            parenthesized: false,
        },
    )
}

/// Append `tail` as the final statement of `body`. If `body` is
/// already a `Seq`, the result is a `Seq` with one more element;
/// otherwise the result wraps both in a new `Seq`.
fn append_statement(body: &Expr, tail: Expr) -> Expr {
    let mut exprs = match &*body.node {
        ExprNode::Seq { exprs } => exprs.clone(),
        _ => vec![body.clone()],
    };
    exprs.push(tail);
    Expr::new(body.span, ExprNode::Seq { exprs })
}

/// Apply the full pre-emit normalization pipeline to an action
/// body — the canonical three-pass sequence every target emitter
/// runs verbatim before walking. Returns a new `Expr`; the input
/// body is untouched.
///
///   1. `resolve_before_actions` — inline `before_action` callback
///      bodies into each action that uses them.
///   2. `unwrap_respond_to` — flatten `respond_to { format.html {…}
///      format.json {…} }` blocks to just their HTML branch.
///   3. `synthesize_implicit_render` — append `render :<action>`
///      when the body has no explicit response terminal.
///
/// Per-target ivar/params rewrites happen AFTER this pipeline
/// (e.g. TS's `rewrite_for_controller`), since the rewrite shape
/// differs between targets (JS-friendly `context.params.k` vs
/// Rust's axum-extractor locals).
pub fn normalize_action_body(
    controller: &Controller,
    action_name: &str,
    body: &Expr,
) -> Expr {
    let with_callbacks = resolve_before_actions(controller, action_name, body);
    let flattened = unwrap_respond_to(&with_callbacks);
    synthesize_implicit_render(&flattened, action_name)
}

/// True when `body` is an empty `Seq` or a `nil` literal — the two
/// shapes every walker needs to recognize so `if cond; A; end` with
/// no else-branch doesn't emit a spurious empty `else { }` block.
pub fn is_empty_body(body: &Expr) -> bool {
    matches!(&*body.node, ExprNode::Seq { exprs } if exprs.is_empty())
        || matches!(&*body.node, ExprNode::Lit { value: Literal::Nil })
}
