//! ERB erubi-trim lowering for view bodies.
//!
//! Rails renders ERB templates via `erubi` with its default `trim`
//! option on. That option strips the leading horizontal whitespace
//! AND trailing newline of any `<% ... %>` non-output tag — the
//! line containing such a tag disappears from the rendered output
//! if it was whitespace-only around the tag.
//!
//! Roundhouse's ERB compiler preserves the original source bytes
//! verbatim so `emit::ruby` can round-trip a view file byte-for-
//! byte. The trim therefore happens at lowering time instead: a
//! post-ingest IR→IR pass produces a body whose text-append
//! statements already reflect what Rails would render.
//!
//! Scope:
//!   * Every `<% %>` tag inside the view gets its line's leading
//!     indent trimmed from the preceding text-append and its
//!     trailing newline trimmed from the following text-append.
//!   * Branch edges (first/last text of each `if`/`else` body)
//!     lose the same indent/newline pair, matching the `<% if %>`
//!     / `<% else %>` / `<% end %>` tag's trim behavior on their
//!     own lines.
//!   * The entire view's last text-append, if it contains only
//!     whitespace, is dropped — that's the trailing `\n` after
//!     the file's final `<% end %>` that Rails' trim consumes.
//!
//! All IR nodes other than `Seq` / `If` / `Send`-with-block /
//! `Lambda` / `Assign` pass through unchanged; those are the only
//! shapes that carry nested view-body Seqs in the compiled ERB
//! shape.

use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::Symbol;

/// Top-level entry point. Applies trim rules recursively to any
/// nested branch / block body and returns a fresh `Expr`. Call
/// this once per view at the top of each target's view-emit
/// function; the returned body is what the emitter should walk.
pub fn trim_view(body: &Expr) -> Expr {
    let body = strip_trailing_newline_text_append(body);
    trim_recursive(&body)
}

fn trim_recursive(expr: &Expr) -> Expr {
    match &*expr.node {
        ExprNode::Seq { exprs } => {
            // Apply the erubi trim around non-output stmts at
            // this Seq level, then recurse into each resulting
            // child so nested If / each / form_with bodies get
            // the same treatment.
            let trimmed = erubi_trim_seq(exprs);
            let new_exprs: Vec<Expr> =
                trimmed.into_iter().map(|e| trim_recursive(&e)).collect();
            Expr::new(expr.span.clone(), ExprNode::Seq { exprs: new_exprs })
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            // erubi trim consumes the `\n` after the opening
            // `<% if/else %>` tag AND the leading indent of the
            // closing `<% else/end %>` tag — so the first/last
            // text in each branch is edge-trimmed, then the body
            // recurses normally.
            let then_trimmed = trim_recursive(&trim_branch_edges(then_branch));
            let else_has_content = !matches!(
                &*else_branch.node,
                ExprNode::Lit { value: Literal::Nil }
            );
            let else_trimmed = if else_has_content {
                trim_recursive(&trim_branch_edges(else_branch))
            } else {
                else_branch.clone()
            };
            Expr::new(
                expr.span.clone(),
                ExprNode::If {
                    cond: cond.clone(),
                    then_branch: then_trimmed,
                    else_branch: else_trimmed,
                },
            )
        }
        ExprNode::Send { recv, method, args, block, parenthesized } => {
            // Recurse into every sub-expression of a Send so nested
            // form_with / each / content_for block bodies are
            // reached even when they sit a few levels deep (e.g.
            // `_buf + form_with(...) { ... }` is an outer `+` Send
            // whose args[0] is the inner Send that carries the
            // block). Missing any one of these would leave some
            // view-body Seq untrimmed and let per-target whitespace
            // divergences creep back.
            let new_recv = recv.as_ref().map(trim_recursive);
            let new_args: Vec<Expr> = args.iter().map(trim_recursive).collect();
            let new_block = block.as_ref().map(trim_recursive);
            Expr::new(
                expr.span.clone(),
                ExprNode::Send {
                    recv: new_recv,
                    method: method.clone(),
                    args: new_args,
                    block: new_block,
                    parenthesized: *parenthesized,
                },
            )
        }
        ExprNode::Lambda { params, block_param, body, block_style } => Expr::new(
            expr.span.clone(),
            ExprNode::Lambda {
                params: params.clone(),
                block_param: block_param.clone(),
                body: trim_recursive(body),
                block_style: *block_style,
            },
        ),
        // The `_buf = _buf + X` shape — X can itself be a
        // compound expression (form_with call with block); recurse
        // through the value side.
        ExprNode::Assign { target, value } => Expr::new(
            expr.span.clone(),
            ExprNode::Assign {
                target: target.clone(),
                value: trim_recursive(value),
            },
        ),
        _ => expr.clone(),
    }
}

// ── Internal helpers ──────────────────────────────────────────

/// Produce a new statement list where text-chunks adjacent to
/// non-output ERB statements have their whitespace trimmed:
///   - Text after a `<% %>` tag: leading `\n` stripped.
///   - Text before a `<% %>` tag: trailing horizontal whitespace
///     on the tag's own line stripped (back to the last `\n`).
fn erubi_trim_seq(stmts: &[Expr]) -> Vec<Expr> {
    let mut out: Vec<Expr> = stmts.to_vec();
    for i in 0..out.len() {
        if !is_non_output_erb_stmt(&out[i]) {
            continue;
        }
        if i > 0 {
            if let Some(trimmed) = trim_trailing_line_indent_of_text_append(&out[i - 1]) {
                out[i - 1] = trimmed;
            }
        }
        if i + 1 < out.len() {
            if let Some(trimmed) = trim_leading_newline_of_text_append(&out[i + 1]) {
                out[i + 1] = trimmed;
            }
        }
    }
    out
}

/// A non-output ERB statement — the IR shapes that `<% ... %>`
/// (not `<%= ... %>`) tags compile to. Output-side `_buf = _buf +
/// …` appends are NOT non-output.
fn is_non_output_erb_stmt(stmt: &Expr) -> bool {
    match &*stmt.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, .. }
            if name.as_str() == "_buf" =>
        {
            false
        }
        ExprNode::If { .. } => true,
        ExprNode::Send { recv: None, .. } => true,
        _ => false,
    }
}

/// Trim the first/last text chunks of an if/else branch to match
/// erubi's leading-whitespace / trailing-newline trim on the
/// `<% if/else/end %>` tag lines.
pub fn trim_branch_edges(body: &Expr) -> Expr {
    let body = trim_leading_newline_of_first_text(&body.clone());
    trim_trailing_line_indent_of_last_text(&body)
}

fn trim_leading_newline_of_first_text(body: &Expr) -> Expr {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            if let Some(trimmed) = trim_leading_newline_of_text_append(&exprs[0]) {
                let mut new_exprs = exprs.clone();
                new_exprs[0] = trimmed;
                Expr::new(body.span.clone(), ExprNode::Seq { exprs: new_exprs })
            } else {
                body.clone()
            }
        }
        _ => trim_leading_newline_of_text_append(body).unwrap_or_else(|| body.clone()),
    }
}

fn trim_trailing_line_indent_of_last_text(body: &Expr) -> Expr {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let last = exprs.len() - 1;
            if let Some(trimmed) = trim_trailing_line_indent_of_text_append(&exprs[last]) {
                let mut new_exprs = exprs.clone();
                new_exprs[last] = trimmed;
                Expr::new(body.span.clone(), ExprNode::Seq { exprs: new_exprs })
            } else {
                body.clone()
            }
        }
        _ => trim_trailing_line_indent_of_text_append(body).unwrap_or_else(|| body.clone()),
    }
}

/// Drop the very last whitespace-only text-append of a view body
/// (the `\n` Rails' erubi trim consumes after the file's closing
/// `<% end %>`). The compiled ERB always ends with a bare `_buf`
/// epilogue; look at the statement just before that when deciding
/// what to drop.
fn strip_trailing_newline_text_append(body: &Expr) -> Expr {
    let ExprNode::Seq { exprs } = &*body.node else {
        return body.clone();
    };
    if exprs.is_empty() {
        return body.clone();
    }
    let last_idx = exprs.len() - 1;
    let target_idx = if matches!(
        &*exprs[last_idx].node,
        ExprNode::Var { name, .. } if name.as_str() == "_buf"
    ) {
        if last_idx == 0 {
            return body.clone();
        }
        last_idx - 1
    } else {
        last_idx
    };
    if !is_whitespace_only_text_append(&exprs[target_idx]) {
        return body.clone();
    }
    let mut new_exprs = exprs.clone();
    new_exprs.remove(target_idx);
    Expr::new(body.span.clone(), ExprNode::Seq { exprs: new_exprs })
}

fn is_whitespace_only_text_append(stmt: &Expr) -> bool {
    let ExprNode::Assign {
        target: LValue::Var { name, .. },
        value,
    } = &*stmt.node
    else {
        return false;
    };
    if name.as_str() != "_buf" {
        return false;
    }
    let ExprNode::Send { recv: Some(r), method, args, .. } = &*value.node else {
        return false;
    };
    if method.as_str() != "+" || args.len() != 1 {
        return false;
    }
    let recv_is_buf = matches!(
        &*r.node,
        ExprNode::Var { name, .. } if name.as_str() == "_buf"
    );
    let text_ws = matches!(
        &*args[0].node,
        ExprNode::Lit { value: Literal::Str { value: s } }
            if s.bytes().all(|b| b == b'\n' || b == b' ' || b == b'\t'),
    );
    recv_is_buf && text_ws
}

fn trim_leading_newline_of_text_append(stmt: &Expr) -> Option<Expr> {
    let (name, recv, method, args, value_span, stmt_span) = extract_text_append(stmt)?;
    let ExprNode::Lit {
        value: Literal::Str { value: text },
    } = &*args[0].node
    else {
        return None;
    };
    let stripped = text.strip_prefix('\n')?;
    Some(rebuild_text_append(
        &name, recv, method, &args[0], stripped, value_span, stmt_span,
    ))
}

fn trim_trailing_line_indent_of_text_append(stmt: &Expr) -> Option<Expr> {
    let (name, recv, method, args, value_span, stmt_span) = extract_text_append(stmt)?;
    let ExprNode::Lit {
        value: Literal::Str { value: text },
    } = &*args[0].node
    else {
        return None;
    };
    let last_nl = text.rfind('\n')?;
    let tail = &text[last_nl + 1..];
    if tail.is_empty() || !tail.bytes().all(|b| b == b' ' || b == b'\t') {
        return None;
    }
    let new_text = text[..=last_nl].to_string();
    Some(rebuild_text_append(
        &name, recv, method, &args[0], &new_text, value_span, stmt_span,
    ))
}

type TextAppendRefs<'a> = (
    Symbol,
    &'a Expr,
    &'a Symbol,
    &'a Vec<Expr>,
    &'a crate::span::Span,
    &'a crate::span::Span,
);

fn extract_text_append(stmt: &Expr) -> Option<TextAppendRefs<'_>> {
    let ExprNode::Assign {
        target: LValue::Var { name, .. },
        value,
    } = &*stmt.node
    else {
        return None;
    };
    if name.as_str() != "_buf" {
        return None;
    }
    let ExprNode::Send { recv: Some(recv), method, args, .. } = &*value.node else {
        return None;
    };
    if method.as_str() != "+" || args.len() != 1 {
        return None;
    }
    let ExprNode::Var { name: rn, .. } = &*recv.node else {
        return None;
    };
    if rn.as_str() != "_buf" {
        return None;
    }
    if !matches!(&*args[0].node, ExprNode::Lit { value: Literal::Str { .. } }) {
        return None;
    }
    Some((name.clone(), recv, method, args, &value.span, &stmt.span))
}

fn rebuild_text_append(
    name: &Symbol,
    recv: &Expr,
    method: &Symbol,
    old_text_arg: &Expr,
    new_text: &str,
    value_span: &crate::span::Span,
    stmt_span: &crate::span::Span,
) -> Expr {
    let new_text_arg = Expr::new(
        old_text_arg.span.clone(),
        ExprNode::Lit {
            value: Literal::Str {
                value: new_text.to_string(),
            },
        },
    );
    let new_rhs = Expr::new(
        value_span.clone(),
        ExprNode::Send {
            recv: Some(recv.clone()),
            method: method.clone(),
            args: vec![new_text_arg],
            block: None,
            parenthesized: false,
        },
    );
    Expr::new(
        stmt_span.clone(),
        ExprNode::Assign {
            target: LValue::Var {
                id: crate::ident::VarId(0),
                name: name.clone(),
            },
            value: new_rhs,
        },
    )
}
