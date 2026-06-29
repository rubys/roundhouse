//! Walker: traverse a compiled-ERB body and produce the corresponding
//! spinel-shape statement list. Dispatches output-position expressions
//! to the helper / partial / form-with / form-builder sub-modules.

use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::span::Span;

use crate::lower::view::{
    classify_form_builder_method, classify_render_partial, classify_view_helper, ViewHelperKind,
};

use super::form_builder::emit_form_builder_inline;
use super::form_with::{emit_form_with_inline, is_errors_each, rewrite_errors_each_body};
use super::helpers::emit_view_helper_call;
use super::partial::{emit_render_partial, emit_yield};
use super::predicates::rewrite_predicates;
use super::{
    accumulator_append_call, accumulator_result_ref, assign_accumulator_string_new, lit_sym,
    nil_lit, seq, todo_io_append, view_helpers_call, ViewCtx,
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
        let mut lowered = walk_stmt(stmt, ctx);
        // Synthesis choke point: everything walk_stmt invented for this
        // source statement (helper sends, io appends, TODO markers, …)
        // attributes back to the statement it was derived from. Inner
        // recursions (if-branches, each-bodies, form_with bodies) have
        // already stamped their own, finer spans — those win.
        for e in &mut lowered {
            e.inherit_span(stmt.span);
        }
        out.extend(lowered);
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
                            // Stamp with the appended chunk's span — one
                            // notch tighter than the enclosing
                            // `_buf = _buf + …` statement walk_body uses.
                            let mut out = emit_io_append(&args[0], ctx);
                            for e in &mut out {
                                e.inherit_span(args[0].span);
                            }
                            return out;
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

    // `<%= expr if cond %>` — modifier-if (no else). Rails renders the
    // expr only when cond is truthy, nothing otherwise. Emit a GUARDED
    // append so the then-branch goes through the same render/helper/escape
    // classifiers and a nil/false cond yields no output — instead of
    // `html_escape(<If>)`, which both wrongly escapes html_safe render
    // output and crashes on `html_escape(nil)`. Full if/else and ternaries
    // (`a ? b : c`, non-nil else) fall through to the default escape path.
    if let ExprNode::If { cond, then_branch, else_branch } = &*inner.node {
        let no_else = matches!(&*else_branch.node, ExprNode::Lit { value: Literal::Nil })
            || matches!(&*else_branch.node, ExprNode::Seq { exprs } if exprs.is_empty());
        if no_else {
            let then_stmts = emit_io_append(then_branch, ctx);
            let guarded = Expr::new(
                inner.span,
                ExprNode::If {
                    cond: rewrite_helpers_in_expr(cond, ctx),
                    then_branch: seq(then_stmts),
                    else_branch: Expr::new(inner.span, ExprNode::Lit { value: Literal::Nil }),
                },
            );
            return vec![guarded];
        }
    }

    // form_with capture: `<%= form_with(opts) do |form| ...inner... %>`
    // — inline-expanded at lower time. Emits the opening `<form ...>`
    // tag, runtime CSRF + _method override helpers, a typed
    // FormBuilder constructor (no `ViewHelpers.form_with(HashMap)`
    // call), the walked body directly against the outer accumulator,
    // and the closing `</form>`. See `emit_form_with_inline` for the
    // shape rationale (Wedge 1b-i of the form_with macro-inline
    // retirement; tracking memo project_form_with_inlining.md).
    if let ExprNode::Send {
        recv: None,
        method,
        args: sa,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if method.as_str() == "form_with" {
            return emit_form_with_inline(sa, block, ctx);
        }
    }

    // FormBuilder method dispatch: `<%= form.text_field :title, opts
    // %>` where `form` is the active form_with block param. After
    // Wedge 1b-ii these inline-expand to direct HTML accumulation —
    // no runtime FormBuilder dispatch survives in lowered output.
    // `textarea` alias normalizes to `text_area` via
    // `classify_form_builder_method`; class-array opts simplify to
    // base + first-key composition.
    if let ExprNode::Send {
        recv: Some(r),
        method,
        args: sa,
        block: None,
        ..
    } = &*inner.node
    {
        if let ExprNode::Var { name, .. } = &*r.node {
            if let Some(binding) = ctx
                .form_records
                .iter()
                .find(|b| b.form_param == name.as_str())
            {
                if let Some(fb) = classify_form_builder_method(method.as_str()) {
                    return emit_form_builder_inline(binding, fb, sa, ctx);
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

    // Generic block-form output helper: `<%= form_tag(...) do %> INNER
    // <% end %>` (form_tag / content_tag / link_to-with-block — anything
    // not form_with, form_builder, or render). The block body is template
    // buffer ops; walk it into a fresh capture accumulator the block
    // *returns*, so the inner `_buf = _buf + …` lines become real appends
    // instead of surviving raw (an undefined `_buf`, and a paren-less
    // helper arg whose comma Ruby reads as a multi-assign target). The
    // wrapping call's parens bind the `do`-block to the helper, not `<<`.
    if let ExprNode::Send {
        recv,
        method,
        args: sa,
        block: Some(block),
        parenthesized,
    } = &*inner.node
    {
        if let ExprNode::Lambda { params, block_param, body, block_style } = &*block.node {
            if block_body_is_template(body) {
                let cap = "_cap";
                let cap_ctx = ViewCtx {
                    accumulator: cap.to_string(),
                    ..ctx.with_locals(params.iter().map(|p| p.as_str().to_string()))
                };
                let mut cap_stmts = vec![assign_accumulator_string_new(cap)];
                cap_stmts.extend(walk_body(body, &cap_ctx));
                cap_stmts.push(accumulator_result_ref(cap));
                let new_block = Expr::new(
                    block.span,
                    ExprNode::Lambda {
                        params: params.clone(),
                        block_param: block_param.clone(),
                        body: seq(cap_stmts),
                        block_style: *block_style,
                    },
                );
                let rebuilt = Expr::new(
                    inner.span,
                    ExprNode::Send {
                        recv: recv.as_ref().map(|r| rewrite_helpers_in_expr(r, ctx)),
                        method: method.clone(),
                        args: sa.iter().map(|a| rewrite_helpers_in_expr(a, ctx)).collect(),
                        block: Some(new_block),
                        parenthesized: *parenthesized,
                    },
                );
                let escaped = view_helpers_call("html_escape", vec![rebuilt]);
                return vec![accumulator_append_call(escaped, ctx)];
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
    let escaped = view_helpers_call("html_escape", vec![coerce_to_s(rewritten)]);
    vec![accumulator_append_call(escaped, ctx)]
}

/// Re-add the `.to_s` coercion that `unwrap_to_s` stripped, so the
/// auto-escape `html_escape(...)` wrap always feeds a String. The ERB
/// compiler wraps every `<%= expr %>` as `(expr).to_s`; we strip that
/// up front so the render / yield / helper / modifier-if classifiers can
/// pattern-match the bare inner expr, but the bare-interpolation default
/// then has to put it back. `html_escape` is deliberately monomorphic
/// `(String) -> String` (it calls `.gsub`; see ViewHelpers.html_escape),
/// so a bare `<%= article.id %>` / `<%= comment.score %>` — an Integer —
/// would otherwise crash. Rails likewise coerces with `to_s` before
/// escaping, and `nil.to_s == ""` gives the empty-render Rails produces
/// for a nil interpolation.
///
/// String literals are returned untouched so `view_helpers_call` can
/// still constant-fold `html_escape("literal")`; coercing one would be a
/// no-op (`String#to_s` is identity) that only defeats the fold.
fn coerce_to_s(expr: Expr) -> Expr {
    if matches!(&*expr.node, ExprNode::Lit { value: Literal::Str { .. } }) {
        return expr;
    }
    // Already a `.to_s` send — the source wrote `<%= x.to_s %>` and
    // `unwrap_to_s` peeled only the compiler's outer wrap, leaving the
    // explicit one. `String#to_s` is identity, so don't double it.
    if let ExprNode::Send { method, args, .. } = &*expr.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return expr;
        }
    }
    let span = expr.span;
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(expr),
            method: Symbol::from("to_s"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    )
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
                if let Some(mut call) = emit_view_helper_call(&kind, ctx) {
                    call.inherit_span(e.span);
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

/// Does a block body hold compiled-template buffer ops (`_buf = _buf + …`)?
/// Distinguishes a capture block (`<%= form_tag … do %> INNER <% end %>`)
/// from a plain value block (`<%= items.map { |x| … } %>`), so only the
/// former is rewritten into a returned capture accumulator.
fn block_body_is_template(body: &Expr) -> bool {
    let stmts: Vec<&Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };
    stmts.iter().any(|s| {
        matches!(&*s.node,
            ExprNode::Assign { target: LValue::Var { name, .. }, .. }
                if name.as_str() == "_buf")
    })
}

fn unwrap_to_s(expr: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(inner), method, args, .. } = &*expr.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return inner;
        }
    }
    expr
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::BlockStyle;
    use crate::ident::VarId;
    use crate::span::Span;

    fn str_lit(s: &str) -> Expr {
        Expr::new(Span::default(), ExprNode::Lit { value: Literal::Str { value: s.into() } })
    }
    fn var(name: &str) -> Expr {
        Expr::new(Span::default(), ExprNode::Var { id: VarId(0), name: Symbol::from(name) })
    }
    /// `recv.to_s` — a no-arg to_s send.
    fn send_to_s(recv: Expr) -> Expr {
        Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from("to_s"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        )
    }
    /// `_buf = _buf + arg` — the compiled-ERB append shape.
    fn buf_append(arg: Expr) -> Expr {
        let plus = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(var("_buf")),
                method: Symbol::from("+"),
                args: vec![arg],
                block: None,
                parenthesized: false,
            },
        );
        Expr::new(
            Span::default(),
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: Symbol::from("_buf") },
                value: plus,
            },
        )
    }
    fn test_ctx() -> ViewCtx {
        ViewCtx {
            locals: Vec::new(),
            arg_name: String::new(),
            resource_dir: String::new(),
            accumulator: "io".to_string(),
            form_records: Vec::new(),
            nullable_locals: Default::default(),
            stylesheets: Vec::new(),
            partial_ivars: Default::default(),
        }
    }

    #[test]
    fn form_block_body_lowers_to_capture_accumulator() {
        // Compiled `<%= form_tag(x) do %> inner <% end %>` is
        //   _buf = _buf + (form_tag(x) do _buf = _buf + "inner" end).to_s
        // The inner `_buf` ops must be walked into a returned capture
        // accumulator, not left raw (the bug found against lobsters).
        let inner = Expr::new(
            Span::default(),
            ExprNode::Lambda {
                params: Vec::new(),
                block_param: None,
                body: buf_append(str_lit("inner")),
                block_style: BlockStyle::Do,
            },
        );
        let form_call = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("form_tag"),
                args: vec![var("x")],
                block: Some(inner),
                parenthesized: false,
            },
        );
        let to_s = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(form_call),
                method: Symbol::from("to_s"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        let stmts = walk_body(&buf_append(to_s), &test_ctx());
        let emitted = stmts
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(emitted.contains("_cap"), "expected capture accumulator:\n{emitted}");
        assert!(emitted.contains("form_tag"), "form_tag call preserved:\n{emitted}");
        assert!(!emitted.contains("_buf"), "raw _buf must not survive:\n{emitted}");
    }

    #[test]
    fn auto_escape_recoerces_with_to_s() {
        // Compiled `<%= comment.score %>` is `_buf = _buf + (comment.score).to_s`.
        // `unwrap_to_s` strips the `.to_s` so the classifiers see the bare
        // `comment.score`; the auto-escape default must re-add it before the
        // `html_escape` wrap, or the monomorphic `(String) -> String` helper
        // crashes on an Integer score.
        let score = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(var("comment")),
                method: Symbol::from("score"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        let to_s = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(score),
                method: Symbol::from("to_s"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        let stmts = walk_body(&buf_append(to_s), &test_ctx());
        let emitted = stmts
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(emitted.contains("html_escape"), "auto-escape wrap present:\n{emitted}");
        assert!(
            emitted.contains("comment.score.to_s") || emitted.contains("(comment.score).to_s"),
            "score must be coerced with .to_s before html_escape:\n{emitted}"
        );
    }

    #[test]
    fn auto_escape_explicit_to_s_is_not_doubled() {
        // `<%= x.to_s %>` compiles to `_buf = _buf + (x.to_s).to_s`;
        // `unwrap_to_s` strips one, and the auto-escape coercion must not
        // re-add a second — `html_escape(x.to_s)`, not `x.to_s.to_s`.
        let inner = send_to_s(var("x"));
        let stmts = walk_body(&buf_append(send_to_s(inner)), &test_ctx());
        let emitted = stmts
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(emitted.contains("html_escape(x.to_s)"), "expected single .to_s:\n{emitted}");
        assert!(!emitted.contains("to_s.to_s"), "must not double .to_s:\n{emitted}");
    }

    #[test]
    fn auto_escape_string_literal_stays_foldable() {
        // A bare `<%= "hi" %>` must NOT pick up `.to_s` — `view_helpers_call`
        // constant-folds `html_escape("literal")`, and coercing a String
        // literal is a no-op that only defeats the fold.
        let lit = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(str_lit("hi")),
                method: Symbol::from("to_s"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        let stmts = walk_body(&buf_append(lit), &test_ctx());
        let emitted = stmts
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!emitted.contains("to_s"), "string literal must not be coerced:\n{emitted}");
    }
}

