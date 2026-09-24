//! `system(cmd, …, exception: true)` → `system(cmd, …) || raise(…)`.
//!
//! The keyword makes a failed command raise instead of answering
//! false/nil. Spinel's `system` takes no options Hash ("use
//! Process.spawn"), and this one option is expressible without one:
//! the call's falsy answer IS the failure. CRuby's message names the
//! exit status; a spawn failure raises `Errno::ENOENT` there and a
//! RuntimeError here — the one divergence, and only in the message
//! class of a command that could not run. lobsters' ResticJob backs up
//! the database this way.
//!
//! Only the lone `exception: true` hash is claimed; any other option
//! (`chdir:`, `out:`) keeps the call as written, so the refusal stays
//! visible for the shapes this does not model.

use crate::app::App;
use crate::expr::{BoolOpKind, BoolOpSurface, Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;

pub fn apply_system_exception_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "system" || args.len() < 2 {
        return;
    }
    let Some(last) = args.last() else { return };
    let ExprNode::Hash { entries, kwargs: true } = &*last.node else { return };
    let [(k, v)] = entries.as_slice() else { return };
    let is_exception_true = matches!(&*k.node,
            ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "exception")
        && matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { value: true } });
    if !is_exception_true {
        return;
    }
    let span = expr.span;
    let cmd_args: Vec<Expr> = args[..args.len() - 1].to_vec();
    let mut call = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("system"),
            args: cmd_args.clone(),
            block: None,
            parenthesized: true,
        },
    );
    call.ty = expr.ty.clone();
    let message = Expr::new(
        span,
        ExprNode::StringInterp {
            parts: vec![
                InterpPart::Text { value: "Command failed: ".to_string() },
                InterpPart::Expr { expr: cmd_args[0].clone() },
            ],
        },
    );
    let raise = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("raise"),
            args: vec![message],
            block: None,
            parenthesized: true,
        },
    );
    let ty = expr.ty.clone();
    *expr = Expr::new(
        span,
        ExprNode::BoolOp { op: BoolOpKind::Or, surface: BoolOpSurface::Symbol, left: call, right: raise },
    );
    expr.ty = ty;
}
