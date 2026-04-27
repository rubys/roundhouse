//! Walker: traverse a compiled-ERB body and produce the corresponding
//! spinel-shape statement list. Dispatches output-position expressions
//! to the helper / partial / form-with / form-builder sub-modules.

use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::span::Span;

use crate::lower::view::{
    classify_form_builder_method, classify_render_partial, classify_view_helper, ViewHelperKind,
};

use super::form_builder::emit_form_builder_call;
use super::form_with::{emit_form_with_capture, is_errors_each, rewrite_errors_each_body};
use super::helpers::emit_view_helper_call;
use super::partial::{emit_render_partial, emit_yield};
use super::predicates::rewrite_predicates;
use super::{
    accumulator_append_call, lit_sym, nil_lit, seq, todo_io_append, view_helpers_call, ViewCtx,
};

/// Walk a compiled-ERB body (`Seq` of `_buf = …` statements + control-
/// flow) and produce the corresponding spinel-shape statement list:
/// `io << ...` / `if cond ... end` / `coll.each { |x| ... }` / bare
/// helper-call statements (content_for setter), in source order.
pub(super) fn walk_body(body: &Expr, ctx: &ViewCtx) -> Vec<Expr> {
    let stmts: Vec<&Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };
    let mut out = Vec::new();
    for stmt in &stmts {
        out.extend(walk_stmt(stmt, ctx));
    }
    out
}

fn walk_stmt(stmt: &Expr, ctx: &ViewCtx) -> Vec<Expr> {
    match &*stmt.node {
        // Prologue `_buf = ""` — drop; we inject `io = String.new` once
        // at method-body construction time.
        ExprNode::Assign { target: LValue::Var { name, .. }, value }
            if name.as_str() == "_buf" =>
        {
            if let ExprNode::Lit { value: Literal::Str { value: s } } = &*value.node {
                if s.is_empty() {
                    return Vec::new();
                }
            }
            // `_buf = _buf + X` — the working shape.
            if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*value.node {
                if method.as_str() == "+" && args.len() == 1 {
                    if let ExprNode::Var { name: rn, .. } = &*recv.node {
                        if rn.as_str() == "_buf" {
                            return emit_io_append(&args[0], ctx);
                        }
                    }
                }
            }
            // Unrecognized `_buf = …` shape — emit as TODO comment-style
            // io append so the file still parses; the test asserts on
            // recognized shapes only.
            vec![todo_io_append("unknown _buf shape")]
        }
        // Epilogue `_buf` read — drop; the explicit trailing `io` is
        // appended once at method-body construction.
        ExprNode::Var { name, .. } if name.as_str() == "_buf" => Vec::new(),
        // Conditional branching at the template level. Cond goes
        // through `rewrite_predicates` so Rails-style `.present?` /
        // `.any?` / `.none?` / `.blank?` collapse to the
        // `.empty?`-based forms spinel's runtime expects.
        ExprNode::If { cond, then_branch, else_branch } => {
            let then_seq = walk_body(then_branch, ctx);
            let then_body = if then_seq.len() == 1 {
                then_seq.into_iter().next().unwrap()
            } else {
                seq(then_seq)
            };
            let else_body = if matches!(
                &*else_branch.node,
                ExprNode::Lit { value: Literal::Nil }
            ) {
                nil_lit()
            } else {
                let s = walk_body(else_branch, ctx);
                if s.len() == 1 {
                    s.into_iter().next().unwrap()
                } else {
                    seq(s)
                }
            };
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: rewrite_predicates(cond, &ctx.nullable_locals),
                    then_branch: then_body,
                    else_branch: else_body,
                },
            )]
        }
        // Statement-form view-helper calls. Today: `<% content_for
        // :title, "Articles" %>` lowers to `ViewHelpers.content_for_set
        // (:title, "Articles")` — a bare call, not appended to `io`.
        // Other recognized statement-form helpers fall through to a
        // TODO append until a fixture exercises them.
        ExprNode::Send { recv: None, method, args, block: None, .. } => {
            if let Some(ViewHelperKind::ContentForSetter { slot, body }) =
                classify_view_helper(method.as_str(), args)
            {
                return vec![view_helpers_call(
                    "content_for_set",
                    vec![lit_sym(Symbol::from(slot)), body.clone()],
                )];
            }
            vec![todo_io_append("unknown stmt")]
        }
        // Block-form `<% coll.each do |x| %>...<% end %>` at the
        // template level (rare — usually the each is inside a `<%= %>`
        // wrapper for collection partial render). When it shows up, we
        // recurse on the block body so inner `_buf = _buf + …` lines
        // become `io << …` against the outer io.
        ExprNode::Send {
            recv: Some(recv),
            method,
            args,
            block: Some(block),
            ..
        } if method.as_str() == "each" && args.is_empty() => {
            let ExprNode::Lambda { params, body, block_style, .. } = &*block.node else {
                return vec![todo_io_append("each block shape")];
            };
            let var_name = params
                .first()
                .map(|p| p.as_str().to_string())
                .unwrap_or_else(|| "item".into());
            // Spinel's `errors` is a `Vec<String>`, not a Vec of error
            // objects. Real Rails templates iterate via `e.full_message`;
            // rewrite that bareword projection back to the local so it
            // type-checks against spinel's runtime.
            let body = if is_errors_each(recv) {
                rewrite_errors_each_body(body, &var_name)
            } else {
                body.clone()
            };
            let inner_ctx = ctx.with_locals([var_name.clone()]);
            let inner_stmts = walk_body(&body, &inner_ctx);
            let inner_body = if inner_stmts.len() == 1 {
                inner_stmts.into_iter().next().unwrap()
            } else {
                seq(inner_stmts)
            };
            let block_lambda = Expr::new(
                Span::synthetic(),
                ExprNode::Lambda {
                    params: params.clone(),
                    block_param: None,
                    body: inner_body,
                    block_style: *block_style,
                },
            );
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(recv.clone()),
                    method: method.clone(),
                    args: Vec::new(),
                    block: Some(block_lambda),
                    parenthesized: false,
                },
            )]
        }
        _ => vec![todo_io_append("unknown stmt")],
    }
}

/// Emit the IR for `io << <argument>` given the argument expression
/// from a `_buf = _buf + ARG` step. Splits into text-chunk vs.
/// output-interpolation handling — the latter goes through the
/// helper / partial / auto-escape classifiers.
fn emit_io_append(arg: &Expr, ctx: &ViewCtx) -> Vec<Expr> {
    // Text chunk → io << "literal".
    if let ExprNode::Lit { value: Literal::Str { .. } } = &*arg.node {
        return vec![accumulator_append_call(arg.clone(), ctx)];
    }
    // The compiler wraps `<%= expr %>` as `(expr).to_s`; strip that
    // wrapper. If the source wrote `<%= x.to_s %>` explicitly, we lose
    // the trailing `.to_s` — the round-trip is stable on the second
    // pass either way (matches the existing reconstruct_erb policy).
    let inner = unwrap_to_s(arg);

    // `<%= yield %>` and `<%= yield :slot %>` — appears in layouts
    // (and other capture-style templates that delegate body
    // rendering). Bare `yield` resolves to the layout's `body`
    // parameter (the rendered inner-view string); `yield :slot` is
    // a slot lookup against the content_for store.
    if let ExprNode::Yield { args: ya } = &*inner.node {
        return vec![accumulator_append_call(emit_yield(ya, ctx), ctx)];
    }

    // form_with capture: `<%= form_with(opts) do |form| ...inner... %>`
    // — wraps a sub-template. Lowers to a `ViewHelpers.form_with(opts)
    // do |form| body = String.new; <walked inner>; body end` call,
    // appended to the outer accumulator. The inner walk uses a fresh
    // `body` accumulator so the captured string is the form contents
    // (not concatenated to the outer io).
    if let ExprNode::Send {
        recv: None,
        method,
        args: sa,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if method.as_str() == "form_with" {
            return vec![emit_form_with_capture(sa, block, ctx)];
        }
    }

    // FormBuilder method dispatch: `<%= form.text_field :title, opts
    // %>` where `form` is a known FormBuilder local. Pass through as
    // `<accumulator> << form.<method>(args)` (raw — FormBuilder output
    // is html-safe). `textarea` aliases to `text_area`; `submit` with
    // no positional gets a leading `nil`. Class-array opts simplify
    // to the base string entry.
    if let ExprNode::Send {
        recv: Some(r),
        method,
        args: sa,
        block: None,
        ..
    } = &*inner.node
    {
        if let ExprNode::Var { name, .. } = &*r.node {
            if ctx.form_records.iter().any(|(n, _)| n == name.as_str()) {
                if let Some(fb) = classify_form_builder_method(method.as_str()) {
                    let call = emit_form_builder_call(name.clone(), fb, sa);
                    return vec![accumulator_append_call(call, ctx)];
                }
            }
        }
    }

    // Render-partial classifier: `render @articles` / `render
    // @article.comments` / `render "x", k: v` → spinel-shape iteration
    // or named-partial dispatch. Wins over the helper classifier
    // because `render` is reserved.
    if let ExprNode::Send { recv, method, args: sa, block, .. } = &*inner.node {
        let is_local = |n: &str| ctx.is_local(n);
        if let Some(rp) =
            classify_render_partial(recv.as_ref(), method.as_str(), sa, block.as_ref(), &is_local)
        {
            if let Some(stmt) = emit_render_partial(&rp, ctx) {
                return vec![stmt];
            }
        }
    }

    // View-helper classifier: `link_to`, `dom_id`, `pluralize`,
    // `truncate`, `turbo_stream_from`, `content_for(:slot)`, …. The
    // classifier matches bare Sends (no recv, no block) only.
    if let ExprNode::Send { recv: None, method, args: sa, block: None, .. } = &*inner.node {
        if let Some(kind) = classify_view_helper(method.as_str(), sa) {
            if let Some(call) = emit_view_helper_call(&kind, ctx) {
                return vec![accumulator_append_call(call, ctx)];
            }
        }
    }

    // Default: bare interpolation `<%= expr %>` of a non-helper —
    // auto-escape. `<%= article.title %>` becomes
    // `io << ViewHelpers.html_escape(article.title)`. This matches
    // Rails's default behavior on `<%= %>` outside of helper output.
    //
    // Before wrapping, recurse through the expression and rewrite
    // any nested helper Sends to their ViewHelpers.* form so shapes
    // like `<%= content_for(:title) || "Real Blog" %>` (a BoolOp
    // whose left side is a bare helper call) come out as
    // `html_escape(ViewHelpers.content_for_get(:title) || "Real
    // Blog")` rather than carrying the raw `content_for` Send.
    let rewritten = rewrite_helpers_in_expr(inner, ctx);
    let escaped = view_helpers_call("html_escape", vec![rewritten]);
    vec![accumulator_append_call(escaped, ctx)]
}

/// Recursively walk `expr` and rewrite any bare view-helper Send
/// (`Send { recv: None, method, args, block: None }`) into its
/// `ViewHelpers.*` form via `classify_view_helper` +
/// `emit_view_helper_call`. Threads through `BoolOp` and `Send`
/// children so helpers nested inside expressions get reached. Other
/// shapes pass through; leaf nodes are unchanged.
///
/// The auto-escape fallback uses this so `<%= helper(...) || default
/// %>` and similar combinations end up with the helper rewritten
/// before the html_escape wrap, matching the convention every
/// emitter would otherwise have to repeat.
fn rewrite_helpers_in_expr(e: &Expr, ctx: &ViewCtx) -> Expr {
    let new_node = match &*e.node {
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            parenthesized,
        } => {
            if let Some(kind) = classify_view_helper(method.as_str(), args) {
                if let Some(call) = emit_view_helper_call(&kind, ctx) {
                    return call;
                }
            }
            ExprNode::Send {
                recv: None,
                method: method.clone(),
                args: args.iter().map(|a| rewrite_helpers_in_expr(a, ctx)).collect(),
                block: None,
                parenthesized: *parenthesized,
            }
        }
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(|r| rewrite_helpers_in_expr(r, ctx)),
            method: method.clone(),
            args: args.iter().map(|a| rewrite_helpers_in_expr(a, ctx)).collect(),
            block: block.as_ref().map(|b| rewrite_helpers_in_expr(b, ctx)),
            parenthesized: *parenthesized,
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_helpers_in_expr(left, ctx),
            right: rewrite_helpers_in_expr(right, ctx),
        },
        other => other.clone(),
    };
    Expr::new(e.span, new_node)
}

fn unwrap_to_s(expr: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(inner), method, args, .. } = &*expr.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return inner;
        }
    }
    expr
}

