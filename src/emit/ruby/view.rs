//! `app/views/**/*.erb` emission. The view body is a compiled-ERB
//! `Expr` tree (`_buf = "" ; _buf = _buf + ...`); reconstruction walks
//! that shape and rebuilds the original ERB syntax.

use std::path::PathBuf;

use super::super::EmittedFile;
use super::expr::{emit_do_block, emit_expr, emit_send_base};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;

pub(super) fn emit_view(view: &crate::dialect::View) -> EmittedFile {
    let path = PathBuf::from(format!(
        "app/views/{}.{}.erb",
        view.name, view.format
    ));
    let content = reconstruct_erb(&view.body);
    EmittedFile { path, content }
}

/// Walk a view body whose structure is:
///   _buf = ""
///   _buf = _buf + "text"           # text chunk
///   _buf = _buf + (expr).to_s      # <%= expr %>
///   <other ruby statement>         # <% code %> (control flow)
///   _buf                           # epilogue
/// and reconstruct the corresponding ERB source.
pub fn reconstruct_erb(body: &Expr) -> String {
    let mut out = String::new();
    let stmts: &[Expr] = match &*body.node {
        ExprNode::Seq { exprs } => exprs,
        // Single-statement body — shouldn't happen for compiled ERB but
        // fall through gracefully.
        _ => {
            out.push_str(&emit_buf_stmt(body));
            return out;
        }
    };
    for stmt in stmts {
        out.push_str(&emit_buf_stmt(stmt));
    }
    out
}

fn emit_buf_stmt(stmt: &Expr) -> String {
    match &*stmt.node {
        // Prologue: `_buf = ""` — swallow.
        ExprNode::Assign {
            target: LValue::Var { name, .. },
            value,
        } if name.as_str() == "_buf" => {
            if let ExprNode::Lit { value: Literal::Str { value: s } } = &*value.node {
                if s.is_empty() {
                    return String::new();
                }
            }
            // `_buf = _buf + X` — the working shape.
            if let ExprNode::Send {
                recv: Some(recv),
                method,
                args,
                ..
            } = &*value.node
            {
                if method.as_str() == "+" && args.len() == 1 {
                    if let ExprNode::Var { name: rn, .. } = &*recv.node {
                        if rn.as_str() == "_buf" {
                            return emit_buf_append(&args[0]);
                        }
                    }
                }
            }
            // Unrecognized `_buf = ...` — fall through as code.
            format!("<% {} %>", emit_expr(stmt))
        }
        // Epilogue: bare `_buf` read at end.
        ExprNode::Var { name, .. } if name.as_str() == "_buf" => String::new(),
        // Control flow: `recv.method(args) do |params| body end` inside a
        // template body. Emit the opening as `<% recv.method(args) do |p| %>`,
        // reconstruct the block body template-style, close with `<% end %>`.
        ExprNode::Send {
            recv,
            method,
            args,
            block: Some(block),
            parenthesized,
        } => emit_template_block_send(
            recv.as_ref(),
            method,
            args,
            block,
            *parenthesized,
        ),
        // Conditional: `<% if cond %> then-template <% else %> else-template <% end %>`.
        // A missing else clause is represented by `Lit(Nil)`; when we see it,
        // omit the `<% else %>` segment.
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = emit_expr(cond);
            let then_s = reconstruct_erb(then_branch);
            if matches!(
                &*else_branch.node,
                ExprNode::Lit { value: Literal::Nil }
            ) {
                format!("<% if {} %>{}<% end %>", cond_s, then_s)
            } else {
                let else_s = reconstruct_erb(else_branch);
                format!("<% if {} %>{}<% else %>{}<% end %>", cond_s, then_s, else_s)
            }
        }
        // Anything else is a raw control statement.
        _ => format!("<% {} %>", emit_expr(stmt)),
    }
}

fn emit_template_block_send(
    recv: Option<&Expr>,
    method: &Symbol,
    args: &[Expr],
    block: &Expr,
    parenthesized: bool,
) -> String {
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        // Unexpected block shape — fall back to raw code emission.
        return format!(
            "<% {} %>",
            emit_do_block(&emit_send_base(recv, method, args, parenthesized), block)
        );
    };
    let base = emit_send_base(recv, method, args, parenthesized);
    let params_clause = if params.is_empty() {
        "do".to_string()
    } else {
        let ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
        format!("do |{}|", ps.join(", "))
    };
    let inner = reconstruct_erb(body);
    format!("<% {} {} %>{}<% end %>", base, params_clause, inner)
}

/// Emit the argument of `_buf = _buf + ARG` either as a text chunk or
/// as a `<%= expr %>` output interpolation.
fn emit_buf_append(arg: &Expr) -> String {
    // Text chunk: the argument is a string literal.
    if let ExprNode::Lit { value: Literal::Str { value: s } } = &*arg.node {
        return s.clone();
    }
    // Output interpolation: strip the `(expr).to_s` wrapper the compiler
    // added. If somebody wrote `<%= x.to_s %>` explicitly, unwrap once
    // and accept the loss of the explicit `.to_s` — round-trip is stable
    // on the second pass regardless.
    let inner = unwrap_to_s(arg);
    // Output-block case: `<%= recv.method(args) do |p| %>body<% end %>`.
    // The inner expression is a Send with an attached block; the block
    // body is itself a compiled ERB template we can reconstruct.
    if let ExprNode::Send {
        recv,
        method,
        args,
        block: Some(block),
        parenthesized,
    } = &*inner.node
    {
        if let ExprNode::Lambda { params, body, .. } = &*block.node {
            let base = emit_send_base(recv.as_ref(), method, args, *parenthesized);
            let params_clause = if params.is_empty() {
                "do".to_string()
            } else {
                let ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
                format!("do |{}|", ps.join(", "))
            };
            let inner_erb = reconstruct_erb(body);
            return format!("<%= {} {} %>{}<% end %>", base, params_clause, inner_erb);
        }
    }
    format!("<%= {} %>", emit_expr(inner))
}

fn unwrap_to_s(expr: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(inner), method, args, .. } = &*expr.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return inner;
        }
    }
    expr
}
