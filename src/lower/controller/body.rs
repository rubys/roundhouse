//! Action-body normalization — the pre-emit pipeline that reshapes
//! an action's body `Expr` so every target emitter can walk it
//! without per-target special cases:
//!
//!   1. Inline applicable `before_action` callback bodies
//!      (`actions::resolve_before_actions`).
//!   2. Flatten `respond_to { format.html {…} format.json {…} }` into
//!      an `if request_format == :json; …; else …; end` dispatch
//!      (when both branches use the `render :sym` shape); fall back
//!      to html-only when either branch has a more complex shape
//!      (`unwrap_respond_to`).
//!   3. Append a synthetic `render :<action>` when the body has no
//!      explicit response terminal (`synthesize_implicit_render`).
//!
//! Per-target ivar/params rewrites run AFTER this pipeline.

use crate::dialect::Controller;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::span::Span;

use super::actions::resolve_before_actions;
use super::util::{is_format_binding, unwrap_lambda};

/// Flatten every `respond_to do |format| ... end` block in `expr`
/// into just its HTML branch — the legacy behavior used by the
/// per-target paths (`normalize_action_body`) that don't yet know
/// how to emit the format dispatch. Group 1 emitters (Ruby /
/// Crystal / TS, via `lower_controllers_with_arel_and_views`) call
/// `unwrap_respond_to_with_format_dispatch` instead, which
/// preserves the json branch as a `request_format == :json`
/// conditional.
pub fn unwrap_respond_to(expr: &Expr) -> Expr {
    unwrap_respond_to_inner(expr, /*with_format_dispatch=*/ false, /*breadth=*/ false)
}

/// Format-dispatching variant of `unwrap_respond_to`.
///
/// When both the html and json branches use the simple `render :sym`
/// shape, the respond_to becomes an `if request_format == :json` /
/// `else` dispatch with the json branch's render carrying a `format:
/// :json` kwarg (consumed downstream by `rewrite_render_to_views` to
/// route to the `<sym>_json` view and tag `content_type:
/// "application/json"`). For all other json-branch shapes — inline
/// `render json: <expr>`, `head :no_content`, redirects in error
/// branches — we fall back to html-only flattening so the HTTP-HTML
/// paths every emitter targets stay lossless.
///
/// `breadth: true` (the CRuby emit path, whose overlay runtime can
/// answer the extra surface) widens the dispatch: `format.json`
/// branches of ANY shape are preserved (inline `render json: <expr>`
/// normalizes downstream to a `JsonRender.encode` body render), and
/// `format.rss` branches are preserved under a `request_format ==
/// :rss` arm (lobsters' /rss private feed). Non-breadth callers keep
/// the narrow behavior so their emit stays byte-identical.
///
/// Handles both scaffold shapes:
///   - Simple:    `respond_to { format.html { a }; format.json { b } }` → `if c; b' else a end`
///   - Branched:  `respond_to { if c; format.html { a1 }; format.json { b1 }
///                              else;  format.html { a2 }; format.json { b2 } end }`
///                 → `if c; <a1+b1 dispatch> else <a2+b2 dispatch> end`
///
/// Walks recursively — nested `respond_to` calls (rare) flatten
/// bottom-up, and non-respond_to sub-expressions pass through their
/// structural variants so anything already at the top level is
/// preserved.
pub fn unwrap_respond_to_with_format_dispatch(expr: &Expr, breadth: bool) -> Expr {
    unwrap_respond_to_inner(expr, /*with_format_dispatch=*/ true, breadth)
}

fn unwrap_respond_to_inner(expr: &Expr, with_format_dispatch: bool, breadth: bool) -> Expr {
    // Top-level `respond_to` with a block — replace the whole Send
    // with its flattened body. This short-circuits the structural
    // recursion so we don't re-enter the respond_to's Send/Lambda
    // children via the generic path.
    if let ExprNode::Send { recv: None, method, block: Some(block), .. } = &*expr.node {
        if method.as_str() == "respond_to" {
            let lambda_body = unwrap_lambda(block);
            return flatten_respond_to_body(lambda_body, with_format_dispatch, breadth);
        }
    }
    let recurse = |e: &Expr| unwrap_respond_to_inner(e, with_format_dispatch, breadth);
    let new_node = match &*expr.node {
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(&recurse).collect(),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: recurse(cond),
            then_branch: recurse(then_branch),
            else_branch: recurse(else_branch),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(&recurse),
            method: method.clone(),
            args: args.iter().map(&recurse).collect(),
            block: block.as_ref().map(&recurse),
            parenthesized: *parenthesized,
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: recurse(left),
            right: recurse(right),
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: recurse(body),
            block_style: *block_style,
        },
        ExprNode::Assign { target, value } => {
            let new_target = match target {
                LValue::Attr { recv, name } => LValue::Attr {
                    recv: recurse(recv),
                    name: name.clone(),
                },
                LValue::Index { recv, index } => LValue::Index {
                    recv: recurse(recv),
                    index: recurse(index),
                },
                other => other.clone(),
            };
            ExprNode::Assign {
                target: new_target,
                value: recurse(value),
            }
        }
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(&recurse).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, kwargs } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (recurse(k), recurse(v)))
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
        hint: expr.hint,
        decisions: expr.decisions,
    }
}

/// Flatten the immediate body of a `respond_to` block. Recognized
/// shapes at this level are `Seq` (the `format.html/.json` pair) and
/// `If` (conditional branching to different format pairs); anything
/// else is handled via `flatten_format_pair_or_drop` directly.
///
/// `with_format_dispatch=false` keeps just the html branch (legacy
/// behavior, used by Group 2 emit paths). `true` emits the
/// `if request_format == :json; …; else; …; end` shape.
fn flatten_respond_to_body(body: &Expr, with_format_dispatch: bool, breadth: bool) -> Expr {
    let recurse_outer = |e: &Expr| unwrap_respond_to_inner(e, with_format_dispatch, breadth);
    match &*body.node {
        ExprNode::Seq { exprs } => {
            let mut html: Option<Expr> = None;
            let mut json: Option<Expr> = None;
            let mut rss: Option<Expr> = None;
            let mut other: Vec<Expr> = Vec::new();
            for e in exprs {
                match classify_format_stmt(e) {
                    Some((fmt, branch_body)) if fmt.as_str() == "html" => {
                        html = Some(recurse_outer(&branch_body));
                    }
                    Some((fmt, branch_body)) if fmt.as_str() == "json" => {
                        // Narrow mode preserves only simple `render :sym
                        // [, kwargs]` shapes — others fall through and
                        // effectively drop (the html branch alone covers
                        // the response). Breadth mode (CRuby) keeps ANY
                        // body: inline `render json: <expr>` normalizes
                        // downstream. Group 2 emit doesn't carry the
                        // dispatch (its emitters don't recognize
                        // `request_format`), so we drop unconditionally
                        // when `with_format_dispatch=false`.
                        if with_format_dispatch
                            && (breadth || is_simple_render_sym(&branch_body))
                        {
                            json = Some(recurse_outer(&branch_body));
                        }
                    }
                    Some((fmt, branch_body)) if fmt.as_str() == "rss" && breadth => {
                        rss = Some(recurse_outer(&branch_body));
                    }
                    Some(_) => {} // unknown format (e.g. format.xml) — drop
                    None => other.push(recurse_outer(e)),
                }
            }
            build_format_dispatch(html, json, rss, other, body.span)
        }
        ExprNode::If { cond, then_branch, else_branch } => Expr::new(
            body.span,
            ExprNode::If {
                cond: recurse_outer(cond),
                then_branch: flatten_respond_to_body(then_branch, with_format_dispatch, breadth),
                else_branch: flatten_respond_to_body(else_branch, with_format_dispatch, breadth),
            },
        ),
        // A single expression at respond_to-body scope — either a
        // lone `format.html`/`format.json`, or some unrelated shape
        // the pass leaves to the generic walker.
        _ => match classify_format_stmt(body) {
            Some((fmt, branch_body)) if fmt.as_str() == "html" => recurse_outer(&branch_body),
            Some((fmt, branch_body))
                if fmt.as_str() == "json"
                    && with_format_dispatch
                    && (breadth || is_simple_render_sym(&branch_body)) =>
            {
                build_format_dispatch(
                    None,
                    Some(recurse_outer(&branch_body)),
                    None,
                    Vec::new(),
                    body.span,
                )
            }
            Some((fmt, branch_body)) if fmt.as_str() == "rss" && breadth => {
                build_format_dispatch(
                    None,
                    None,
                    Some(recurse_outer(&branch_body)),
                    Vec::new(),
                    body.span,
                )
            }
            Some(_) => Expr::new(body.span, ExprNode::Seq { exprs: vec![] }),
            None => recurse_outer(body),
        },
    }
}

/// Pull `(format_name, block_body)` out of a `format.<x> { body }`
/// Send. Returns `None` for any statement that isn't a format
/// binding. The bare-form `format.html` (no block) returns an empty
/// Seq body so callers can treat block-form and bare-form uniformly.
fn classify_format_stmt(e: &Expr) -> Option<(Symbol, Expr)> {
    if let ExprNode::Send { recv: Some(recv), method, block, .. } = &*e.node {
        if is_format_binding(recv) {
            let body = match block.as_ref() {
                Some(b) => unwrap_lambda(b).clone(),
                None => Expr::new(e.span, ExprNode::Seq { exprs: vec![] }),
            };
            return Some((method.clone(), body));
        }
    }
    None
}

/// True when `body` is a shape the json dispatch supports today:
/// either `render :sym [, kwargs]` (simple view-template) or
/// `head :sym` (status-only terminal). Inline renders (`render
/// json: <expr>`) and redirects in error branches don't qualify
/// and the json branch gets dropped (html alone covers the
/// response).
fn is_simple_render_sym(body: &Expr) -> bool {
    match &*body.node {
        ExprNode::Send { recv: None, method, args, .. }
            if method.as_str() == "render" && !args.is_empty() =>
        {
            matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { .. } })
        }
        ExprNode::Send { recv: None, method, args, .. }
            if method.as_str() == "head" && !args.is_empty() =>
        {
            matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { .. } })
                || matches!(&*args[0].node, ExprNode::Lit { value: Literal::Int { .. } })
        }
        _ => false,
    }
}

/// Build the `if request_format == :json; <json>; else; <html>; end`
/// dispatch. Drop branches that are `None`: a missing json branch
/// falls through to html on both paths; a missing html branch
/// (uncommon — the source action defined only `format.json`) uses
/// the same empty Seq on the else.
fn build_format_dispatch(
    html: Option<Expr>,
    json: Option<Expr>,
    rss: Option<Expr>,
    other: Vec<Expr>,
    span: Span,
) -> Expr {
    // Innermost-first: the html branch is the else-default, an rss arm
    // (breadth mode only) wraps it, and the json arm wraps outermost —
    // so the no-rss shape stays byte-identical to the legacy two-way
    // dispatch.
    let mut dispatch = match (&html, &json, &rss) {
        (Some(h), None, None) => h.clone(),
        (None, None, None) => Expr::new(span, ExprNode::Seq { exprs: vec![] }),
        _ => html.unwrap_or_else(|| Expr::new(span, ExprNode::Seq { exprs: vec![] })),
    };
    if let Some(r) = rss {
        dispatch = Expr::new(
            span,
            ExprNode::If {
                cond: request_format_eq(span, "rss"),
                then_branch: r,
                else_branch: dispatch,
            },
        );
    }
    if let Some(j) = json {
        let json_body = mark_render_format(&j, "json");
        dispatch = Expr::new(
            span,
            ExprNode::If {
                cond: request_format_eq(span, "json"),
                then_branch: json_body,
                else_branch: dispatch,
            },
        );
    }
    if other.is_empty() {
        dispatch
    } else {
        let mut all = other;
        all.push(dispatch);
        Expr::new(span, ExprNode::Seq { exprs: all })
    }
}

/// `request_format == :<fmt>` — the predicate every dispatched action
/// branches on. `request_format` is a Base accessor populated by the
/// CGI driver from a path-suffix sniff (`.json` → `:json`). Emitted
/// with an explicit `self` receiver so Group 2 emitters (Elixir,
/// Python, Go, Rust) that distinguish methods from locals at the
/// emit layer route it to the accessor rather than to a bare
/// variable lookup.
fn request_format_eq(span: Span, fmt: &str) -> Expr {
    let recv = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::SelfRef)),
            method: Symbol::from("request_format"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let fmt_sym = Expr::new(
        span,
        ExprNode::Lit {
            value: Literal::Sym {
                value: Symbol::from(fmt),
            },
        },
    );
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("=="),
            args: vec![fmt_sym],
            block: None,
            parenthesized: false,
        },
    )
}

/// Walk `body` and tag terminals with format-aware kwargs:
///   - `render(<sym>, …)` gets a `format: :<fmt>` marker; the kwarg
///     flows into `rewrite_render_to_views`, which strips it and
///     uses it to route to the `<sym>_<fmt>` view + tag the outer
///     render with `content_type: "<mime>"`.
///   - `head(<sym>, …)` gets a `content_type: "<mime>"` kwarg
///     directly. head doesn't go through view rewriting (its body
///     is empty regardless of format), so the lowerer plants the
///     MIME marker here rather than via the render-rewrite path.
fn mark_render_format(body: &Expr, fmt: &str) -> Expr {
    let new_node = match &*body.node {
        ExprNode::Send {
            recv: None,
            method,
            args,
            block,
            parenthesized,
        } if method.as_str() == "render" && !args.is_empty() => {
            if matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { .. } }) {
                let new_args = add_format_kwarg(args, fmt, body.span);
                ExprNode::Send {
                    recv: None,
                    method: method.clone(),
                    args: new_args,
                    block: block.clone(),
                    parenthesized: *parenthesized,
                }
            } else {
                return body.clone();
            }
        }
        ExprNode::Send {
            recv: None,
            method,
            args,
            block,
            parenthesized,
        } if method.as_str() == "head" && !args.is_empty() => {
            let new_args = add_content_type_kwarg(args, mime_for_format(fmt), body.span);
            ExprNode::Send {
                recv: None,
                method: method.clone(),
                args: new_args,
                block: block.clone(),
                parenthesized: *parenthesized,
            }
        }
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(|e| mark_render_format(e, fmt)).collect(),
        },
        ExprNode::If {
            cond,
            then_branch,
            else_branch,
        } => ExprNode::If {
            cond: cond.clone(),
            then_branch: mark_render_format(then_branch, fmt),
            else_branch: mark_render_format(else_branch, fmt),
        },
        _ => return body.clone(),
    };
    Expr::new(body.span, new_node)
}

/// Map a Rails format symbol to its canonical MIME string.
fn mime_for_format(fmt: &str) -> &'static str {
    match fmt {
        "json" => "application/json",
        _ => "text/html; charset=utf-8",
    }
}

/// Append (or merge into) a trailing kwarg-Hash carrying
/// `content_type: "<mime>"`. Used to tag head call sites in the
/// json branch — render call sites take the format-kwarg path so
/// the view-rewrite layer can decide the MIME.
fn add_content_type_kwarg(args: &[Expr], mime: &str, span: Span) -> Vec<Expr> {
    let pair = (
        Expr::new(
            span,
            ExprNode::Lit {
                value: Literal::Sym {
                    value: Symbol::from("content_type"),
                },
            },
        ),
        Expr::new(
            span,
            ExprNode::Lit {
                value: Literal::Str {
                    value: mime.to_string(),
                },
            },
        ),
    );
    let mut out = args.to_vec();
    if let Some(last) = out.last_mut() {
        if let ExprNode::Hash { entries, kwargs: true } = &*last.node {
            let mut new_entries = entries.clone();
            new_entries.push(pair);
            *last = Expr::new(
                last.span,
                ExprNode::Hash {
                    entries: new_entries,
                    kwargs: true,
                },
            );
            return out;
        }
    }
    out.push(Expr::new(
        span,
        ExprNode::Hash {
            entries: vec![pair],
            kwargs: true,
        },
    ));
    out
}

/// Append (or merge into) a trailing kwarg-Hash carrying `format:
/// :<fmt>`. Render call args have the shape `[symbol, ...kwarg_hash?]`
/// — if a trailing Hash already exists we merge `format:` into it;
/// otherwise we append a new kwarg Hash.
fn add_format_kwarg(args: &[Expr], fmt: &str, span: Span) -> Vec<Expr> {
    let fmt_pair = (
        Expr::new(
            span,
            ExprNode::Lit {
                value: Literal::Sym {
                    value: Symbol::from("format"),
                },
            },
        ),
        Expr::new(
            span,
            ExprNode::Lit {
                value: Literal::Sym {
                    value: Symbol::from(fmt),
                },
            },
        ),
    );
    let mut out = args.to_vec();
    if let Some(last) = out.last_mut() {
        if let ExprNode::Hash { entries, kwargs: true } = &*last.node {
            let mut new_entries = entries.clone();
            new_entries.push(fmt_pair);
            *last = Expr::new(
                last.span,
                ExprNode::Hash {
                    entries: new_entries,
                    kwargs: true,
                },
            );
            return out;
        }
    }
    out.push(Expr::new(
        span,
        ExprNode::Hash {
            entries: vec![fmt_pair],
            kwargs: true,
        },
    ));
    out
}

/// Append a synthesized `render :<action_name>` Send to `body` when
/// `body` has no top-level render / redirect_to / head terminal.
/// Encodes the Rails convention that an action falling off the end
/// renders its eponymous view.
///
/// When `has_json_variant` is true, the synthesized render expands
/// to a format dispatch — `if request_format == :json; render
/// :<action>, format: :json; else; render :<action>; end` — so
/// requests with a stripped `.json` suffix render the
/// `<action>.json.jbuilder` template. Without a json variant the
/// dispatch would reference an undefined `<action>_json` view at
/// emit time, so we only synthesize it when the variant exists.
///
/// Target-neutral — every emitter walking the result sees an explicit
/// terminal that `classify_controller_send` resolves to `Render`.
/// Before this pass, each scaffold template synthesized the terminal
/// ad-hoc at emit time; after, the walker path needs no special case.
pub fn synthesize_implicit_render(body: &Expr, action_name: &str, has_json_variant: bool) -> Expr {
    if has_toplevel_terminal(body) {
        return body.clone();
    }
    let render = render_symbol_send(action_name, body.span);
    let terminal = if has_json_variant {
        let json_render = render_symbol_send(action_name, body.span);
        let json_branch = mark_render_format(&json_render, "json");
        Expr::new(
            body.span,
            ExprNode::If {
                cond: request_format_eq(body.span, "json"),
                then_branch: json_branch,
                else_branch: render,
            },
        )
    } else {
        render
    };
    // A body with SOME response terminal that isn't guaranteed at top
    // level (a render inside begin/rescue, or behind a condition the
    // detector can't prove) may already have responded by the time the
    // synthesized default runs — Rails' own default-render check is
    // `performed?`, not syntax. Guard the synthesized render the same
    // way. Bodies with no terminal at all (the common case — every
    // conventional index/show) keep the bare unguarded shape.
    let terminal = if contains_terminal(body) {
        Expr::new(
            body.span,
            ExprNode::If {
                cond: Expr::new(
                    body.span,
                    ExprNode::Send {
                        recv: None,
                        method: Symbol::from("performed?"),
                        args: Vec::new(),
                        block: None,
                        parenthesized: false,
                    },
                ),
                then_branch: Expr::new(body.span, ExprNode::Lit { value: Literal::Nil }),
                else_branch: terminal,
            },
        )
    } else {
        terminal
    };
    append_statement(body, terminal)
}

/// True when a response terminal (`render` / `redirect_to` / `head` /
/// `respond_to`-with-block) appears ANYWHERE in the body — the signal
/// that the synthesized default render needs a `performed?` guard (see
/// `synthesize_implicit_render`). Deliberately broader than
/// `has_toplevel_terminal`: that one proves a response always happens;
/// this one detects that a response MIGHT already have happened.
fn contains_terminal(body: &Expr) -> bool {
    fn walk(e: &Expr, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: None, method, block, .. } = &*e.node {
            if matches!(method.as_str(), "render" | "redirect_to" | "head")
                || (method.as_str() == "respond_to" && block.is_some())
            {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, found));
    }
    let mut found = false;
    walk(body, &mut found);
    found
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
    synthesize_implicit_render(&flattened, action_name, /*has_json_variant=*/ false)
}

/// True when `body` is an empty `Seq` or a `nil` literal — the two
/// shapes every walker needs to recognize so `if cond; A; end` with
/// no else-branch doesn't emit a spurious empty `else { }` block.
pub fn is_empty_body(body: &Expr) -> bool {
    matches!(&*body.node, ExprNode::Seq { exprs } if exprs.is_empty())
        || matches!(&*body.node, ExprNode::Lit { value: Literal::Nil })
}
