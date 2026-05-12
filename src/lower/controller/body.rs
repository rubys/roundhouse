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
    unwrap_respond_to_inner(expr, /*with_format_dispatch=*/ false)
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
pub fn unwrap_respond_to_with_format_dispatch(expr: &Expr) -> Expr {
    unwrap_respond_to_inner(expr, /*with_format_dispatch=*/ true)
}

fn unwrap_respond_to_inner(expr: &Expr, with_format_dispatch: bool) -> Expr {
    // Top-level `respond_to` with a block — replace the whole Send
    // with its flattened body. This short-circuits the structural
    // recursion so we don't re-enter the respond_to's Send/Lambda
    // children via the generic path.
    if let ExprNode::Send { recv: None, method, block: Some(block), .. } = &*expr.node {
        if method.as_str() == "respond_to" {
            let lambda_body = unwrap_lambda(block);
            return flatten_respond_to_body(lambda_body, with_format_dispatch);
        }
    }
    let recurse = |e: &Expr| unwrap_respond_to_inner(e, with_format_dispatch);
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
fn flatten_respond_to_body(body: &Expr, with_format_dispatch: bool) -> Expr {
    let recurse_outer = |e: &Expr| unwrap_respond_to_inner(e, with_format_dispatch);
    match &*body.node {
        ExprNode::Seq { exprs } => {
            let mut html: Option<Expr> = None;
            let mut json: Option<Expr> = None;
            let mut other: Vec<Expr> = Vec::new();
            for e in exprs {
                match classify_format_stmt(e) {
                    Some((fmt, branch_body)) if fmt.as_str() == "html" => {
                        html = Some(recurse_outer(&branch_body));
                    }
                    Some((fmt, branch_body)) if fmt.as_str() == "json" => {
                        // Only preserve simple `render :sym [, kwargs]`
                        // shapes — others fall through and effectively
                        // drop (the html branch alone covers the
                        // response). Group 2 emit doesn't carry the
                        // dispatch (its emitters don't recognize
                        // `request_format`), so we drop unconditionally
                        // when `with_format_dispatch=false`.
                        if with_format_dispatch && is_simple_render_sym(&branch_body) {
                            json = Some(recurse_outer(&branch_body));
                        }
                    }
                    Some(_) => {} // unknown format (e.g. format.xml) — drop
                    None => other.push(recurse_outer(e)),
                }
            }
            build_format_dispatch(html, json, other, body.span)
        }
        ExprNode::If { cond, then_branch, else_branch } => Expr::new(
            body.span,
            ExprNode::If {
                cond: recurse_outer(cond),
                then_branch: flatten_respond_to_body(then_branch, with_format_dispatch),
                else_branch: flatten_respond_to_body(else_branch, with_format_dispatch),
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
                    && is_simple_render_sym(&branch_body) =>
            {
                build_format_dispatch(None, Some(recurse_outer(&branch_body)), Vec::new(), body.span)
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

/// True when `body` is exactly `render :sym [, kwargs]` — the simple
/// view-template shape the json dispatch supports today. Inline
/// renders (`render json: <expr>`), `head :no_content`, and
/// redirects in error branches don't qualify and the json branch
/// gets dropped (html alone covers the response).
fn is_simple_render_sym(body: &Expr) -> bool {
    match &*body.node {
        ExprNode::Send { recv: None, method, args, .. }
            if method.as_str() == "render" && !args.is_empty() =>
        {
            matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { .. } })
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
    other: Vec<Expr>,
    span: Span,
) -> Expr {
    let dispatch = match (html, json) {
        (Some(h), None) => h,
        (None, None) => Expr::new(span, ExprNode::Seq { exprs: vec![] }),
        (h, Some(j)) => {
            let html_body =
                h.unwrap_or_else(|| Expr::new(span, ExprNode::Seq { exprs: vec![] }));
            let json_body = mark_render_format(&j, "json");
            Expr::new(
                span,
                ExprNode::If {
                    cond: request_format_eq(span, "json"),
                    then_branch: json_body,
                    else_branch: html_body,
                },
            )
        }
    };
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

/// Walk `body` and add `format: :<fmt>` to every top-level Send
/// `render(<sym>, …)`. The kwarg flows downstream into
/// `rewrite_render_to_views`, which strips it and uses it to choose
/// the `<sym>_json` view + tag the outer render with
/// `content_type: "application/json"`.
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
    append_statement(body, terminal)
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
