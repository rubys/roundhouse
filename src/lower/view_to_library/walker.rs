//! Walker: traverse a compiled-ERB body and produce the corresponding
//! spinel-shape statement list. Dispatches output-position expressions
//! to the helper / partial / form-with / form-builder sub-modules.

use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal};
use crate::ident::Symbol;
use crate::span::Span;

use crate::lower::view::{
    classify_form_builder_method, classify_render_partial, classify_view_helper,
    extract_sym_or_str, ViewHelperKind,
};

use super::form_builder::{
    emit_button_tag, emit_check_box_tag, emit_form_builder_block_inline, emit_form_builder_inline, emit_label_tag,
    emit_submit_tag,
};
use super::form_with::{
    emit_form_tag_inline, emit_form_with_inline, emit_tag_builder_inline, emit_tag_builder_void_or_content, is_errors_each,
    rewrite_errors_each_body,
};
use super::helpers::{emit_inline_helper_block, emit_view_helper_call};
use super::partial::{emit_render_partial, emit_yield};
use super::turbo_drive::emit_turbo_drive_directive;
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
            vec![todo_io_append("unknown _buf shape", stmt.span)]
        }
        // Epilogue `_buf` read — drop; the explicit trailing `io` is
        // appended once at method-body construction.
        ExprNode::Var { name, .. } if name.as_str() == "_buf" => Vec::new(),
        // Template-local `||=` (`<% is_unread ||= false %>`) — same
        // statement treatment as plain assignment; Ruby's `x ||= v`
        // defines the local when unset.
        ExprNode::OpAssign { target: LValue::Var { name, id }, op, value } => {
            vec![Expr::new(
                stmt.span,
                ExprNode::OpAssign {
                    target: LValue::Var { name: name.clone(), id: *id },
                    op: op.clone(),
                    value: rewrite_predicates(
                        value,
                        &ctx.nullable_locals,
                        &ctx.reference_reads,
                        &ctx.nilable_scalar_reads,
                    ),
                },
            )]
        }
        // Template-local assignment (`<% flagged = comment.current_vote
        // && … %>`) — a real statement, not output; later
        // interpolations read the local by name. The value gets the
        // same predicate rewriting a condition does (`<% x =
        // list.any? %>`). Was silently swallowed by the catch-all
        // TODO append, leaving every read a NameError.
        ExprNode::Assign { target: LValue::Var { name, id }, value } => {
            vec![Expr::new(
                stmt.span,
                ExprNode::Assign {
                    target: LValue::Var { name: name.clone(), id: *id },
                    value: rewrite_predicates(
                        value,
                        &ctx.nullable_locals,
                        &ctx.reference_reads,
                        &ctx.nilable_scalar_reads,
                    ),
                },
            )]
        }
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
                    cond: rewrite_predicates(cond, &ctx.nullable_locals, &ctx.reference_reads, &ctx.nilable_scalar_reads),
                    then_branch: then_body,
                    else_branch: else_body,
                },
            )]
        }
        // `<% cache <key> do %> … <% end %>` — Rails fragment caching.
        // The block is a pure OPTIMIZATION wrapper: its body is what the
        // page renders, served from the store on a hit and evaluated on
        // a miss. With no fragment store, evaluating it every time is
        // the correct output and only forgoes the cache.
        //
        // Transparent rather than dropped, because dropped is what it
        // was: `cache` fell through to the catch-all and took the whole
        // BODY with it, so lobsters' `users/tree.html.erb` (44 lines,
        // entirely wrapped) and `users/list.html.erb` each emitted
        // `io << ""` — two blank pages, reported by nothing. The
        // residue ledger this commit adds to `todo_io_append` is what
        // surfaced them.
        ExprNode::Send { recv: None, method, block: Some(block), .. }
            if method.as_str() == "cache" =>
        {
            match &*block.node {
                ExprNode::Lambda { body, .. } => walk_body(body, ctx),
                _ => vec![todo_io_append("cache block shape", stmt.span)],
            }
        }
        // Block-form `<% content_for :subnav do %> … <% end %>` —
        // slot capture (lobsters' subnav pattern: pages and partials
        // deposit nav markup, the layout gates on `content_for? :subnav`
        // and splices via `yield :subnav`). Render the block body into
        // its own accumulator and register it in the slot store; the
        // statement itself contributes no template output. The
        // accumulator name carries the slot so nested captures of
        // different slots can't shadow each other.
        ExprNode::Send { recv: None, method, args, block: Some(block), .. }
            if method.as_str() == "content_for" && args.len() == 1 =>
        {
            let Some(slot) = extract_sym_or_str(&args[0]) else {
                return vec![todo_io_append("dynamic content_for slot", stmt.span)];
            };
            let ExprNode::Lambda { body, .. } = &*block.node else {
                return vec![todo_io_append("content_for block shape", stmt.span)];
            };
            let cap = format!("cf_{slot}");
            let cap_ctx = ViewCtx { accumulator: cap.clone(), ..ctx.clone() };
            let mut out = vec![assign_accumulator_string_new(&cap)];
            out.extend(walk_body(body, &cap_ctx));
            // The capture is an ACCUMULATOR, and on a strict target
            // that is a builder, not a String — Crystal rejected
            // `content_for_set(:head, String::Builder)` the moment a
            // fixture first wrote the block form. Same tagged read the
            // tail of a view function uses, so each emitter finishes
            // the builder its own way.
            out.push(view_helpers_call(
                "content_for_set",
                vec![lit_sym(Symbol::from(slot)), accumulator_result_ref(&cap)],
            ));
            out
        }
        // Statement-form view-helper calls. Today: `<% content_for
        // :title, "Articles" %>` lowers to `ViewHelpers.content_for_set
        // (:title, "Articles")` — a bare call, not appended to `io`.
        // Other recognized statement-form helpers fall through to a
        // TODO append until a fixture exercises them.
        ExprNode::Send { recv: None, method, args, block: None, .. } => {
            // `<% turbo_page_requires_reload %>` — a turbo Drive
            // directive. Deposits into `:head`, appends nothing.
            if let Some(out) = emit_turbo_drive_directive(method.as_str(), args, stmt.span) {
                return out;
            }
            if let Some(ViewHelperKind::ContentForSetter { slot, body }) =
                classify_view_helper(method.as_str(), args)
            {
                return vec![view_helpers_call(
                    "content_for_set",
                    vec![lit_sym(Symbol::from(slot)), body.clone()],
                )];
            }
            // Statement-position render (`<% render partial:
            // 'stories/subnav' %>` — no `=`): Rails evaluates the
            // partial for its side effects (content_for deposits) and
            // DISCARDS the returned markup. Route the append into a
            // throwaway accumulator so the call still happens but
            // nothing lands on the page.
            let is_local = |n: &str| ctx.is_local(n);
            let is_options_ivar = |n: &str| ctx.is_options_ivar(n);
            if let Some(rp) = classify_render_partial(
                None,
                method.as_str(),
                args,
                None,
                &is_local,
                &is_options_ivar,
            ) {
                let disc = "discard";
                let disc_ctx = ViewCtx { accumulator: disc.to_string(), ..ctx.clone() };
                if let Some(stmt) = emit_render_partial(&rp, &disc_ctx) {
                    return vec![assign_accumulator_string_new(disc), stmt];
                }
            }
            vec![todo_io_append("unknown stmt", stmt.span)]
        }
        // `<% case action_name when "all" %>…<% when … %>…<% end %>` at
        // the template level (lobsters' replies help text switches copy
        // on the action). Same treatment as If: recurse each arm's body
        // so inner `_buf` appends land on the accumulator; scrutinee
        // and guards pass through untouched. Was swallowed by the
        // catch-all TODO append — the whole case emitted `io << ""`.
        ExprNode::Case { scrutinee, arms } => {
            let new_arms = arms
                .iter()
                .map(|arm| crate::expr::Arm {
                    pattern: arm.pattern.clone(),
                    guard: arm.guard.clone(),
                    body: {
                        let stmts = walk_body(&arm.body, ctx);
                        if stmts.len() == 1 {
                            stmts.into_iter().next().unwrap()
                        } else {
                            seq(stmts)
                        }
                    },
                })
                .collect();
            vec![Expr::new(
                stmt.span,
                ExprNode::Case { scrutinee: scrutinee.clone(), arms: new_arms },
            )]
        }
        // `<% while cond %>…<% end %>` at the template level —
        // lobsters' users/tree.html.erb walks the invitation tree with
        // an explicit stack (`while subtree`, `ancestors << subtree`,
        // `subtree = ancestors.pop`). Recurse on the body so inner
        // `_buf` appends land on the outer io; cond rides the same
        // predicate rewriting an If cond gets.
        ExprNode::While { cond, body, until_form } => {
            let inner = walk_body(body, ctx);
            let inner_body =
                if inner.len() == 1 { inner.into_iter().next().unwrap() } else { seq(inner) };
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::While {
                    cond: rewrite_predicates(
                        cond,
                        &ctx.nullable_locals,
                        &ctx.reference_reads,
                        &ctx.nilable_scalar_reads,
                    ),
                    body: inner_body,
                    until_form: *until_form,
                },
            )]
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
                return vec![todo_io_append("each block shape", stmt.span)];
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
        // A receiver'd, blockless call in statement position (`<%
        // ancestors << subtree %>`) — a side-effecting mutation whose
        // value is discarded. Pass it through verbatim: template
        // OUTPUT always arrives `_buf`-shaped, so anything here is
        // genuinely a statement (the prior TODO-append swallowed the
        // side effect and broke stack-walking templates).
        ExprNode::Send { recv: Some(_), block: None, .. } => vec![stmt.clone()],
        _ => vec![todo_io_append("unknown stmt", stmt.span)],
    }
}

/// Rebuild a form-wrapper helper's call site as the call it wraps:
/// `composer_form_tag(room) do |form| … end` → `form_with(model: …,
/// url: room_messages_path(room), …) do |form| … end`.
///
/// SUBSTITUTION, not inlining — the wrapper's body is one call by
/// construction ([`FormWrapperHelper`]), so the only thing to carry
/// across is each parameter's argument. Declines (leaving the site to
/// its loud NoMethodError) when an argument is anything but a
/// side-effect-free read: a parameter used twice would evaluate it
/// twice, and moving a call across the boundary changes when it runs.
/// campfire passes a plain local.
fn splice_form_wrapper(
    w: &super::FormWrapperHelper,
    args: &[Expr],
    block: &Expr,
) -> Option<Expr> {
    if args.len() != w.params.len() {
        return None;
    }
    if !args.iter().all(is_pure_read) {
        return None;
    }
    let binding: std::collections::HashMap<&Symbol, &Expr> =
        w.params.iter().zip(args.iter()).collect();
    let ExprNode::Send { method, args: wargs, parenthesized, .. } = &*w.call.node else {
        return None;
    };
    Some(Expr::new(
        w.call.span,
        ExprNode::Send {
            recv: None,
            method: method.clone(),
            args: wargs.iter().map(|a| substitute_vars(a, &binding)).collect(),
            block: Some(block.clone()),
            parenthesized: *parenthesized,
        },
    ))
}

/// A read with no side effect and no cost worth worrying about, so
/// substituting it for each use of a parameter is safe however many
/// times the body names it.
///
/// The zero-arg block-less bare send belongs here for a reason specific
/// to templates: prism cannot prove `room` is a local inside an
/// ERB-ingested body, so a template local arrives as a `Send`, not a
/// `Var`. `composer_form_tag(room)` passes exactly that, and reading it
/// as impure would decline the only site this pass exists for. Same
/// judgment `scope_chain::owner_reads_once` makes about the same shape.
fn is_pure_read(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::Lit { .. }
        | ExprNode::Const { .. } => true,
        ExprNode::Send { recv: None, args, block: None, .. } => args.is_empty(),
        _ => false,
    }
}

fn substitute_vars(e: &Expr, binding: &std::collections::HashMap<&Symbol, &Expr>) -> Expr {
    if let ExprNode::Var { name, .. } = &*e.node {
        if let Some(replacement) = binding.get(name) {
            return (*replacement).clone();
        }
    }
    let mut out = e.clone();
    out.node.for_each_child_mut(&mut |c| *c = substitute_vars(c, binding));
    out
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
                    // Same predicate rewrite as statement-level conds
                    // (see the value-position If arm in
                    // rewrite_helpers_in_expr).
                    cond: rewrite_predicates(
                        &rewrite_helpers_in_expr(cond, ctx),
                        &ctx.nullable_locals,
                        &ctx.reference_reads,
                        &ctx.nilable_scalar_reads,
                    ),
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
        // `<%= form_tag(action, opts) do ...inner... %>` — the
        // builder-less bare form, same inline expansion minus the
        // FormBuilder binding (lobsters' link_post).
        if method.as_str() == "form_tag" {
            return emit_form_tag_inline(sa, block, ctx);
        }
        // `<%= label_tag name do %>…<% end %>` — block-content label
        // (filters page); the open tag inlines, the body splices.
        if method.as_str() == "label_tag" {
            if let Some(out) = emit_label_tag(sa, Some(block), ctx) {
                return out;
            }
        }
    }

    // Bare `<%= submit_tag label, opts %>` / `<%= button_tag content,
    // opts %>` — builder-less controls, inline-expanded like the
    // `form.*` builder methods (the opts hashes are literal at every
    // call site; the runtime alternative is the CRuby overlay's
    // untyped opts-walk).
    if let ExprNode::Send { recv: None, method, args: sa, block: None, .. } = &*inner.node {
        // `<%= turbo_exempts_page_from_preview %>` (campfire's
        // rooms/show) — the OUTPUT spelling of a Drive directive.
        // Rails' `provide` returns nil once it is handed content, so
        // the `<%= %>` renders "": the deposit is the whole effect and
        // nothing joins the accumulator.
        if let Some(out) = emit_turbo_drive_directive(method.as_str(), sa, inner.span) {
            return out;
        }
        if method.as_str() == "submit_tag" {
            return emit_submit_tag(sa, ctx);
        }
        if method.as_str() == "button_tag" {
            return emit_button_tag(sa, ctx);
        }
        if method.as_str() == "check_box_tag" {
            return emit_check_box_tag(sa, ctx);
        }
        // Blockless `<%= label_tag name, content[, opts] %>` — the
        // settings page's labeled fields. No-content non-literal
        // shapes fall through to the runtime helper.
        if method.as_str() == "label_tag" {
            if let Some(out) = emit_label_tag(sa, None, ctx) {
                return out;
            }
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
        // Var inside a form_with lambda; bare Send in a bound PARTIAL
        // (the form local dropped out of the partial's params).
        if let Some(name) = super::form_with::form_param_ref_name(r) {
            if let Some(binding) = ctx
                .form_records
                .iter()
                .find(|b| b.form_param == name)
            {
                if let Some(fb) = classify_form_builder_method(method.as_str()) {
                    return emit_form_builder_inline(binding, fb, sa, ctx);
                }
            }
        }
    }

    // The same dispatch for the BLOCK form — `<%= form.button(opts) do
    // %> … <% end %>`, where the label is markup rather than a string
    // argument (campfire's sign-in page and message composer).
    if let ExprNode::Send {
        recv: Some(r),
        method,
        args: sa,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if let Some(name) = super::form_with::form_param_ref_name(r) {
            if ctx.form_records.iter().any(|b| b.form_param == name) {
                if let Some(fb) = classify_form_builder_method(method.as_str()) {
                    if let Some(out) = emit_form_builder_block_inline(fb, sa, block, ctx) {
                        return out;
                    }
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
        let is_options_ivar = |n: &str| ctx.is_options_ivar(n);
        if let Some(rp) = classify_render_partial(
            recv.as_ref(),
            method.as_str(),
            sa,
            block.as_ref(),
            &is_local,
            &is_options_ivar,
        ) {
            // `render layout: "x" do … end` lowers to SEVERAL statements
            // (a capture local, then the layout call), so it can't ride
            // `emit_render_partial`'s single-expression return. It also
            // has to be caught here rather than in the generic
            // block-form helper arm below, which would keep the bare
            // `render` and emit a call to a method no view module has.
            if let crate::lower::view::RenderPartial::LayoutBlock { layout, locals, block } = &rp {
                if let Some(stmts) =
                    super::partial::emit_layout_block(layout, *locals, block, ctx)
                {
                    return stmts;
                }
            }
            if let Some(stmt) = emit_render_partial(&rp, ctx) {
                return vec![stmt];
            }
        }
    }

    // `turbo_stream.<action>(...)` in a `.turbo_stream.erb` template.
    // Needs the RECEIVER (turbo_stream is a builder object, not a bare
    // helper), so it can't ride the classifier below.
    // Matched WITHOUT the `block: None` constraint so the block form
    // (`turbo_stream.append target do … end`) reaches the residue arm
    // below instead of falling through unmentioned.
    if let ExprNode::Send { recv, method, args: sa, block, .. } = &*inner.node {
        let classified = block.is_none().then(|| {
            crate::lower::view::classify_turbo_stream_call(recv.as_ref(), method.as_str(), sa)
        }).flatten();
        if let Some(ts) = &classified {
            if let Some(call) = super::helpers::emit_turbo_stream_fragment(ts, ctx) {
                return vec![accumulator_append_call(call, ctx)];
            }
        }
        // Recognized the builder but not this spelling — the option form
        // (`partial:`/`collection:`/`locals:`) or the block form. Left in
        // source shape, where `turbo_stream` resolves to nothing, so say
        // so rather than emitting a call that silently won't run.
        if super::helpers::is_turbo_stream_builder(recv.as_ref()) {
            crate::emit::diagnostics::push(crate::lower::residue_diagnostic(
                "turbo_stream_builder",
                "turbo_stream.<action>",
                inner.span,
                "only the positional `target[, record]` spelling is lowered",
                format!(
                    "`turbo_stream.{}` left in source shape — the option form \
                     (partial:/collection:/locals:) and the block form need the \
                     partial machinery a `render` call site gets, so this \
                     template will not render",
                    method.as_str()
                ),
            ));
        }
    }

    // turbo-rails' `turbo_frame_tag`, in both spellings — with a block
    // (the frame wraps template content) and without (a lazily-loaded
    // frame that is only a `src`). Placed ahead of the classifier
    // because the block form never reaches it: the classifier matches
    // `block: None`, so a framed template body fell through to the
    // generic block-form helper below, which captured the body and left
    // the `turbo_frame_tag` call itself unresolved.
    if let ExprNode::Send { recv: None, method, args: sa, block, .. } = &*inner.node {
        if method.as_str() == "turbo_frame_tag" && !ctx.is_local("turbo_frame_tag") {
            if let Some(stmts) = emit_turbo_frame_tag(sa, block.as_ref(), ctx) {
                return stmts;
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

    // ActionView dynamic tag builder: `<%= tag.<element>(opts) do
    // ...inner... %>` (lobsters' stories/_form `tag.details`). The
    // receiver is the bare `tag` builder and the method name is the HTML
    // element. Inline-expand to open/walk/close rather than let it fall
    // to the generic fallback, which rebuilds `tag.<element> do ... end`
    // verbatim — an unresolved `sp_raise_nomethod` under spinel AOT.
    if let ExprNode::Send {
        recv: Some(r),
        method,
        args: sa,
        block: Some(block),
        ..
    } = &*inner.node
    {
        let is_bare_tag = matches!(
            &*r.node,
            ExprNode::Send { recv: None, method: m, args, block: None, .. }
                if m.as_str() == "tag" && args.is_empty()
        );
        if is_bare_tag && !ctx.is_local("tag") {
            return emit_tag_builder_inline(method.as_str(), sa, block, ctx);
        }
    }

    // The same builder WITHOUT a block: `<%= tag.meta name: "…", content:
    // "…" %>` / `<%= tag.img src: … %>` (campfire's application layout
    // writes five of these, so every page depends on it). Without this
    // arm the call falls to the auto-escape default below, which is
    // wrong twice over — `tag` is unbound at run time, and even bound,
    // Rails' builder returns a SafeBuffer that must not be escaped.
    if let ExprNode::Send {
        recv: Some(r),
        method,
        args: sa,
        block: None,
        ..
    } = &*inner.node
    {
        let is_bare_tag = matches!(
            &*r.node,
            ExprNode::Send { recv: None, method: m, args, block: None, .. }
                if m.as_str() == "tag" && args.is_empty()
        );
        if is_bare_tag && !ctx.is_local("tag") {
            return emit_tag_builder_void_or_content(method.as_str(), sa, ctx);
        }
    }

    // `<%= composer_form_tag(room) do |form| %> … <% end %>` — a helper
    // that is nothing but a `form_with` with the block forwarded
    // through. Splice the wrapped call in with the caller's block
    // attached, then walk THAT: the form-builder macro-inline needs the
    // `form_with` and the `form.rich_text_area` calls visible at once,
    // and they are on opposite sides of the helper boundary until this
    // runs. See `form_wrapper_helpers` for why the other block-
    // forwarding wrappers don't need it.
    if let ExprNode::Send {
        recv: None,
        method,
        args: sa,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if let Some(w) = ctx.form_wrappers.get(method.as_str()) {
            if !ctx.is_local(method.as_str()) {
                if let Some(spliced) = splice_form_wrapper(w, sa, block) {
                    return emit_io_append(&spliced, ctx);
                }
            }
        }
    }

    // `<%= button_to url, opts do %> …markup… <% end %>` and the same
    // for `link_to` — the block spelling of a helper whose positional
    // form already inlines. Ahead of the generic arm below, which would
    // rebuild the call verbatim and leave a bare `button_to` / `link_to`
    // the emitted module does not define: the whole helper is inlined at
    // lower time, so nothing answers it at run time.
    if let ExprNode::Send {
        recv: None,
        method,
        args: sa,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if matches!(method.as_str(), "button_to" | "link_to") && !ctx.is_local(method.as_str()) {
            if let ExprNode::Lambda { params, body, .. } = &*block.node {
                if block_body_is_template(body) {
                    if let Some(stmts) =
                        emit_inline_helper_block(method.as_str(), sa, body, params, ctx)
                    {
                        return stmts;
                    }
                }
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
    // `<%= h(x) %>` — ERB's escape alias asks for exactly what the
    // default already does; unwrap it so the auto-escape applies once
    // rather than twice (nested `h` is handled — and double-escapes,
    // as in Rails — inside rewrite_helpers_in_expr).
    let mut inner = inner;
    while let ExprNode::Send { recv: None, method, args, block: None, .. } = &*inner.node {
        if method.as_str() == "h" && args.len() == 1 {
            inner = &args[0];
        } else {
            break;
        }
    }
    //
    // Before wrapping, recurse through the expression and rewrite
    // any nested helper Sends to their ViewHelpers.* form so shapes
    // like `<%= content_for(:title) || "Real Blog" %>` (a BoolOp
    // whose left side is a bare helper call) come out as
    // `html_escape(ViewHelpers.content_for_get(:title) || "Real
    // Blog")` rather than carrying the raw `content_for` Send.
    let rewritten = rewrite_helpers_in_expr(inner, ctx);
    // A marked value skips the wrap. `<%= x.html_safe %>` says so right
    // here; a call to a method whose body ends in `.html_safe` says it
    // one level down, and `lower::html_safe` recorded that on the App so
    // this side can see it. Escaping either one ships literal
    // `&lt;span&gt;` markup where the author asked for markup —
    // lobsters' `hat.to_html_label` renders a whole element.
    if let Some(safe) = html_safe_value(&rewritten, ctx) {
        return vec![accumulator_append_call(coerce_to_s(safe), ctx)];
    }
    let escaped = view_helpers_call("html_escape", vec![coerce_to_s(rewritten)]);
    vec![accumulator_append_call(escaped, ctx)]
}

/// `turbo_frame_tag <ids…>[, opts][ do … end]` → the `<turbo-frame …>`
/// element, the walked block body, and the closing tag — the same
/// open/walk/close splice `emit_tag_builder_inline` does, against the
/// SAME outer accumulator, because an ERB block body is template buffer
/// ops that have to be walked rather than captured.
///
/// The id positionals go through `rewrite_helpers_in_expr` first: that
/// is what turns campfire's `turbo_frame_tag dom_id(room, :involvement)`
/// into the `ViewHelpers.dom_id(…)` call an emitted view can answer.
/// Options are left as written, the way every other tag inlined here
/// leaves them (route helpers in their values are rewritten later, by
/// the routes pass, over the whole IR).
///
/// `None` when the argument shape declines (see
/// `turbo_frames::frame_open_parts`), which leaves the call to the
/// generic paths below rather than inventing an id.
fn emit_turbo_frame_tag(args: &[Expr], block: Option<&Expr>, ctx: &ViewCtx) -> Option<Vec<Expr>> {
    let args: Vec<Expr> = args
        .iter()
        .map(|a| match &*a.node {
            ExprNode::Hash { .. } => a.clone(),
            _ => rewrite_helpers_in_expr(a, ctx),
        })
        .collect();
    let is_record = |e: &Expr| super::turbo_frames::names_a_record(e, &ctx.model_singulars);
    let parts = super::turbo_frames::frame_open_parts(&args, &is_record)?;
    let mut out = vec![accumulator_append_call(
        super::attr_parts::string_interp(parts),
        ctx,
    )];
    if let Some(ExprNode::Lambda { params, body, .. }) = block.map(|b| &*b.node) {
        let inner_ctx = ctx.with_locals(params.iter().map(|p| p.as_str().to_string()));
        out.extend(walk_body(body, &inner_ctx));
    }
    out.push(accumulator_append_call(
        super::lit_str(super::turbo_frames::FRAME_CLOSE.to_string()),
        ctx,
    ));
    Some(out)
}

/// The value behind an html-safe interpolation, or `None` when the
/// expression has to be escaped. `<e>.html_safe` yields `<e>` (the mark
/// is answered here rather than left for a runtime that has no
/// safe-string type); a call to a recorded producer yields itself.
fn html_safe_value(e: &Expr, ctx: &ViewCtx) -> Option<Expr> {
    let ExprNode::Send { recv, method, args, block: None, .. } = &*e.node else {
        return None;
    };
    if method.as_str() == "html_safe" && args.is_empty() {
        return recv.clone();
    }
    if recv.is_some() && ctx.html_safe_methods.contains(method.as_str()) {
        return Some(e.clone());
    }
    None
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
pub(super) fn rewrite_helpers_in_expr(e: &Expr, ctx: &ViewCtx) -> Expr {
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
            // ERB's `h` alias, in a nested/statement position: an explicit
            // escape call. (Nested `h` inside an escaped interpolation
            // double-escapes — as it does in Rails, where interpolating a
            // safe string into a plain one drops safety and the outer
            // `<%= %>` escapes again.) The top-level `<%= h(x) %>` shape
            // never reaches here — the append path unwraps it so the
            // default auto-escape applies exactly once.
            if method.as_str() == "h" && args.len() == 1 {
                let mut call = view_helpers_call(
                    "html_escape",
                    vec![coerce_to_s(rewrite_helpers_in_expr(&args[0], ctx))],
                );
                call.inherit_span(e.span);
                return call;
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
        // Value-position compounds that can carry helper calls in their
        // legs — `<%= cond ? ": " + h(x) : "" %>` (lobsters rss title),
        // `<%= "#{h(x)}!" %>`, `<%= [link_to(...), …].join %>`. Without
        // these arms the nested helper Send survives raw and the view
        // module has no method to answer it.
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            // Value-position conds get the same predicate rewrite a
            // statement-level `<% if … %>` does — `<%= (if
            // title.present? … end) %>` (the lobsters layout <title>)
            // otherwise ships a verbatim `present?` no non-CRuby
            // runtime answers.
            cond: rewrite_predicates(
                &rewrite_helpers_in_expr(cond, ctx),
                &ctx.nullable_locals,
                &ctx.reference_reads,
                &ctx.nilable_scalar_reads,
            ),
            then_branch: rewrite_helpers_in_expr(then_branch, ctx),
            else_branch: rewrite_helpers_in_expr(else_branch, ctx),
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Text { value } => InterpPart::Text { value: value.clone() },
                    InterpPart::Expr { expr } => {
                        InterpPart::Expr { expr: rewrite_helpers_in_expr(expr, ctx) }
                    }
                })
                .collect(),
        },
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(|el| rewrite_helpers_in_expr(el, ctx)).collect(),
            style: *style,
        },
        // Statement compounds: a form-builder map lambda hoisted into a
        // select-options loop is a `Seq` of local Assigns building the
        // option text (`html = "<strong>#{h(t.tag)}</strong>"`;
        // `html << …`) — the helper calls live under the Assign values.
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(|s| rewrite_helpers_in_expr(s, ctx)).collect(),
        },
        ExprNode::Assign { target, value } => ExprNode::Assign {
            target: target.clone(),
            value: rewrite_helpers_in_expr(value, ctx),
        },
        ExprNode::OpAssign { target, op, value } => ExprNode::OpAssign {
            target: target.clone(),
            op: op.clone(),
            value: rewrite_helpers_in_expr(value, ctx),
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
            reference_reads: Default::default(),
            reference_targets: Default::default(),
            nilable_scalar_reads: Default::default(),
            html_safe_methods: Default::default(),
            model_singulars: Default::default(),
            slug_models: Default::default(),
            bool_readers: Default::default(),
            store_readers: Default::default(),
            route_helper_names: Default::default(),
            form_wrappers: Default::default(),
            stylesheets: Vec::new(),
            partial_ivars: Default::default(),
            dyn_pools: Default::default(),
            partial_extras: Default::default(),
            strict_locals: Default::default(),
            ivar_models: Default::default(),
        }
    }

    fn block_helper_call(method: &str) -> Expr {
        block_helper_call_with(method, vec![var("x")])
    }

    /// `<%= <helper> account_join_code_path, class: "btn" do %> inner
    /// <% end %>` — campfire's shape. A path-helper call in the URL
    /// position, because a bare local (what `block_helper_call` passes)
    /// is not a URL the inliner can resolve and would decline.
    fn inline_helper_block_call(helper: &str) -> Expr {
        let path = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("account_join_code_path"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        let opts = Expr::new(
            Span::default(),
            ExprNode::Hash {
                entries: vec![(lit_sym(Symbol::from("class")), str_lit("btn"))],
                kwargs: true,
            },
        );
        block_helper_call_with(helper, vec![path, opts])
    }

    /// The emitted Ruby for one of those, as a single string.
    fn inline_helper_block_emit(helper: &str) -> String {
        walk_body(&buf_append(inline_helper_block_call(helper)), &test_ctx())
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Compiled `<%= <method>(<args>) do %> inner <% end %>` is
    ///   _buf = _buf + (<method>(<args>) do _buf = _buf + "inner" end).to_s
    fn block_helper_call_with(method: &str, args: Vec<Expr>) -> Expr {
        let inner = Expr::new(
            Span::default(),
            ExprNode::Lambda {
                params: Vec::new(),
                block_param: None,
                body: buf_append(str_lit("inner")),
                block_style: BlockStyle::Do,
            },
        );
        let call = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from(method),
                args,
                block: Some(inner),
                parenthesized: false,
            },
        );
        Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(call),
                method: Symbol::from("to_s"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        )
    }

    /// `recv.<method>` — a no-arg send with an explicit receiver.
    fn send0(recv: Expr, method: &str) -> Expr {
        Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from(method),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        )
    }

    #[test]
    fn interpolating_a_marked_value_skips_the_escape() {
        // `<%= x.html_safe %>` says "do not escape this" at the call
        // site. Wrapping it ships `&lt;b&gt;` to the page; and the mark
        // itself has to go, since no target's String answers it.
        let stmts = walk_body(&buf_append(send_to_s(send0(var("x"), "html_safe"))), &test_ctx());
        let emitted = stmts.iter().map(crate::emit::ruby::emit_expr).collect::<Vec<_>>().join("\n");
        assert!(!emitted.contains("html_escape"), "marked value must not be escaped:\n{emitted}");
        assert!(!emitted.contains("html_safe"), "the mark itself must not survive:\n{emitted}");
        assert!(emitted.contains("io << x.to_s"), "value still appended:\n{emitted}");
    }

    #[test]
    fn interpolating_a_recorded_producer_skips_the_escape() {
        // One level down: `hat.to_html_label` ends in `.html_safe`, so
        // `lower::html_safe` recorded the method name and this side
        // must not escape the element it returns.
        let mut ctx = test_ctx();
        ctx.html_safe_methods =
            std::rc::Rc::new(["to_html_label".to_string()].into_iter().collect());
        let stmts = walk_body(&buf_append(send_to_s(send0(var("hat"), "to_html_label"))), &ctx);
        let emitted = stmts.iter().map(crate::emit::ruby::emit_expr).collect::<Vec<_>>().join("\n");
        assert!(!emitted.contains("html_escape"), "recorded producer must not be escaped:\n{emitted}");
        assert!(emitted.contains("hat.to_html_label"), "call preserved:\n{emitted}");
    }

    #[test]
    fn an_ordinary_read_is_still_escaped() {
        // The default has to survive the exception: a plain attribute
        // read is user text and Rails escapes it.
        let stmts = walk_body(&buf_append(send_to_s(send0(var("user"), "about"))), &test_ctx());
        let emitted = stmts.iter().map(crate::emit::ruby::emit_expr).collect::<Vec<_>>().join("\n");
        assert!(emitted.contains("html_escape"), "ordinary read stays escaped:\n{emitted}");
    }

    #[test]
    fn form_block_body_lowers_to_capture_accumulator() {
        // A generic block helper's inner `_buf` ops must be walked into
        // a returned capture accumulator, not left raw (the bug found
        // against lobsters). `form_tag` used to be the example here but
        // now inline-expands (test below); any unclassified block
        // helper still rides the capture machinery.
        let stmts = walk_body(&buf_append(block_helper_call("custom_wrapper")), &test_ctx());
        let emitted = stmts
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(emitted.contains("_cap"), "expected capture accumulator:\n{emitted}");
        assert!(emitted.contains("custom_wrapper"), "helper call preserved:\n{emitted}");
        assert!(!emitted.contains("_buf"), "raw _buf must not survive:\n{emitted}");
    }

    #[test]
    fn form_tag_block_inline_expands_to_form_statements() {
        // `<%= form_tag(x) do %> inner <% end %>` no longer survives as
        // a runtime call: the open tag (action through html_escape),
        // the CSRF hidden input, the walked body, and the close tag
        // splice directly into the outer accumulator.
        let stmts = walk_body(&buf_append(block_helper_call("form_tag")), &test_ctx());
        let emitted = stmts
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(emitted.contains("<form action="), "expected inline open tag:\n{emitted}");
        assert!(
            emitted.contains("csrf_token_hidden_input"),
            "CSRF input must follow the open tag:\n{emitted}"
        );
        assert!(emitted.contains("inner"), "block body walked inline:\n{emitted}");
        assert!(emitted.contains("</form>"), "close tag emitted:\n{emitted}");
        assert!(!emitted.contains("form_tag"), "no runtime form_tag call:\n{emitted}");
        assert!(!emitted.contains("_buf"), "raw _buf must not survive:\n{emitted}");
    }

    #[test]
    fn button_to_block_form_inlines_like_its_positional_twin() {
        // `<%= button_to url do %> markup <% end %>` — campfire's
        // join-code regenerate button. Without its own arm this fell to
        // the generic block helper below, which rebuilds the call
        // verbatim and leaves a bare `button_to` that nothing defines:
        // the helper is inlined at lower time, so there is no runtime
        // method to land on.
        let emitted = inline_helper_block_emit("button_to");

        assert!(emitted.contains("<form action="), "the wrapping form opens inline:\n{emitted}");
        assert!(
            emitted.contains("<button type=\\\"submit\\\""),
            "the button opens before the block body:\n{emitted}"
        );
        assert!(emitted.contains("inner"), "block body walked inline:\n{emitted}");
        assert!(
            emitted.contains("</button>"),
            "and the button closes after it:\n{emitted}"
        );
        assert!(
            emitted.contains("csrf_token_hidden_input"),
            "CSRF input rides the same wrapper the positional form uses:\n{emitted}"
        );
        assert!(
            !emitted.contains("button_to("),
            "no runtime button_to call survives:\n{emitted}"
        );
        assert!(!emitted.contains("_cap"), "no capture accumulator needed:\n{emitted}");
    }

    #[test]
    fn link_to_block_form_inlines_to_an_anchor() {
        // The same defect one helper over, and the one campfire writes
        // most — five templates, `messages/_actions` among them, which
        // renders per message on the room page.
        let emitted = inline_helper_block_emit("link_to");

        assert!(emitted.contains("<a href="), "the anchor opens inline:\n{emitted}");
        assert!(emitted.contains("inner"), "block body walked inline:\n{emitted}");
        assert!(emitted.contains("</a>"), "and closes after it:\n{emitted}");
        assert!(
            !emitted.contains("link_to("),
            "no runtime link_to call survives:\n{emitted}"
        );
        assert!(!emitted.contains("_cap"), "no capture accumulator needed:\n{emitted}");
    }

    #[test]
    fn an_inline_helper_block_body_is_not_escaped() {
        // The positional form escapes its label — it is a String. A
        // BLOCK yields markup (`image_tag`, a literal `<span>`), and
        // Rails renders it as the element's HTML. Escaping it would show
        // the tags as visible text, so the body must append raw.
        for helper in ["button_to", "link_to"] {
            let emitted = inline_helper_block_emit(helper);
            let body_line = emitted
                .lines()
                .find(|l| l.contains("inner"))
                .unwrap_or_else(|| panic!("{helper}: no block body line in\n{emitted}"));
            assert!(
                !body_line.contains("html_escape"),
                "{helper}: block body appends raw:\n{body_line}"
            );
        }
    }

    #[test]
    fn a_form_wrapper_helper_is_spliced_to_the_call_it_wraps() {
        // `composer_form_tag(room) do |form| … end` where the helper is
        // `def composer_form_tag(room, &) = form_with(url: …, &)`. The
        // splice has to happen before the walk reaches the block, or the
        // form-builder macro-inline never sees the `form_with` and the
        // `form.x` calls together — which is the NoMethodError campfire's
        // room page died on.
        let wrapper = super::super::FormWrapperHelper {
            params: vec![Symbol::from("room")],
            call: Expr::new(
                Span::default(),
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("form_with"),
                    args: vec![Expr::new(
                        Span::default(),
                        ExprNode::Hash {
                            entries: vec![(lit_sym(Symbol::from("url")), var("room"))],
                            kwargs: true,
                        },
                    )],
                    block: None,
                    parenthesized: true,
                },
            ),
        };
        let mut ctx = test_ctx();
        ctx.form_wrappers = std::rc::Rc::new(
            [("composer_form_tag".to_string(), wrapper)].into_iter().collect(),
        );
        // The call site passes a template local, which inside an
        // ERB-ingested body parses as a zero-arg Send rather than a Var.
        let local_read = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("the_room"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        let call = block_helper_call_with("composer_form_tag", vec![local_read]);
        let emitted = walk_body(&buf_append(call), &ctx)
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            !emitted.contains("composer_form_tag"),
            "the wrapper call is replaced, not kept:\n{emitted}"
        );
        assert!(
            emitted.contains("<form action="),
            "and the form_with it wraps expands inline:\n{emitted}"
        );
        assert!(
            emitted.contains("the_room"),
            "with the call site's argument substituted for the parameter:\n{emitted}"
        );
        assert!(emitted.contains("inner"), "block body walked inline:\n{emitted}");
    }

    #[test]
    fn a_form_wrapper_declines_an_argument_it_cannot_move() {
        // Substituting a parameter means moving its argument into the
        // wrapped call. A read is free to move; a call is not — it could
        // run a different number of times, or in a different order. Such
        // a site keeps its shape and stays a loud failure.
        let wrapper = super::super::FormWrapperHelper {
            params: vec![Symbol::from("room")],
            call: Expr::new(
                Span::default(),
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("form_with"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: true,
                },
            ),
        };
        let mut ctx = test_ctx();
        ctx.form_wrappers = std::rc::Rc::new(
            [("composer_form_tag".to_string(), wrapper)].into_iter().collect(),
        );
        // `find_room(1)` — an argument-taking call, not a bare read.
        let impure = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("find_room"),
                args: vec![str_lit("1")],
                block: None,
                parenthesized: true,
            },
        );
        let call = block_helper_call_with("composer_form_tag", vec![impure]);
        let emitted = walk_body(&buf_append(call), &ctx)
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            emitted.contains("composer_form_tag"),
            "the site keeps its source shape:\n{emitted}"
        );
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

