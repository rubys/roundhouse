//! `capture { concat(a); concat(b) if c; … }` → an inline accumulator
//! (lobsters' users_helper builds its stories/comments summary lines
//! this way). Rails' CaptureHelper pushes a buffer, runs the block,
//! and returns what `concat` appended; the block runs exactly once and
//! sequentially, so the whole dance lowers to
//!
//!   _cap = ""
//!   _cap = _cap + (a).to_s
//!   _cap = _cap + (b).to_s if c
//!   _cap
//!
//! as a Seq-in-value-position — no buffer stack, no Thread.current
//! (the CRuby overlay's capture becomes a dead fallback for shapes
//! this pass doesn't claim). Plain `+` concat, not `<<`: the
//! strict-lane lesson — `str <<` is outside the proven emitter
//! surface. Only bare `concat(x)` sends inside the block rewrite;
//! nested captures rewrite innermost-first via the recursion (no
//! corpus nesting; the shared `_cap` name would shadow if one
//! appears).

use crate::app::App;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::{Symbol, VarId};

pub fn apply_capture_inline(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let is_capture = matches!(
        &*expr.node,
        ExprNode::Send { recv: None, method, args, block: Some(block), .. }
            if method.as_str() == "capture"
                && args.is_empty()
                && matches!(&*block.node, ExprNode::Lambda { .. })
    );
    if !is_capture {
        return;
    }
    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { block: Some(block), .. } = node else { unreachable!() };
    let ExprNode::Lambda { body, .. } = &*block.node else { unreachable!() };
    let cap = Symbol::from("_cap");
    let mut body = body.clone();
    let appended = rewrite_concats(&mut body, &cap);
    let read = Expr::new(span, ExprNode::Var { id: VarId(0), name: cap.clone() });
    let assign_cap = |value: Expr| {
        Expr::new(
            span,
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: cap.clone() },
                value,
            },
        )
    };
    // NOTHING APPENDED — so the block's RETURN VALUE is the capture.
    // Rails' `capture` yields into a fresh buffer and answers the
    // buffer only when the block put something there; an empty buffer
    // falls back to what the block returned. Every block written in a
    // HELPER module is that second shape — campfire's
    // `link_to … do image_tag(…) + tag.span(…) end` builds its markup
    // with `+` and returns it — and initializing a buffer nothing
    // writes to made the whole anchor render EMPTY: `<a class="btn"
    // href="/"></a>`, well-formed and missing its icon and its label.
    if !appended {
        let mut stmts = match &*body.node {
            ExprNode::Seq { exprs } if !exprs.is_empty() => exprs.clone(),
            _ => vec![body.clone()],
        };
        let value = stmts.pop().expect("non-empty");
        stmts.push(assign_cap(to_s_call(value, span)));
        stmts.push(read);
        *expr.node = ExprNode::Seq { exprs: stmts };
        return;
    }
    let init = assign_cap(Expr::new(
        span,
        ExprNode::Lit { value: crate::expr::Literal::Str { value: String::new() } },
    ));
    *expr.node = ExprNode::Seq { exprs: vec![init, body, read] };
}

fn to_s_call(e: Expr, span: crate::span::Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(e),
            method: Symbol::from("to_s"),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    )
}

/// Answers whether any `concat` was rewritten — the caller needs that
/// to decide between the buffer and the block's return value.
fn rewrite_concats(e: &mut Expr, cap: &Symbol) -> bool {
    let mut found = false;
    e.node.for_each_child_mut(&mut |c| {
        if rewrite_concats(c, cap) {
            found = true;
        }
    });
    let is_concat = matches!(
        &*e.node,
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "concat" && args.len() == 1
    );
    if !is_concat {
        return found;
    }
    let span = e.span;
    let node = std::mem::replace(&mut *e.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { args, .. } = node else { unreachable!() };
    let arg = args.into_iter().next().unwrap();
    let to_s = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(arg),
            method: Symbol::from("to_s"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let cap_ref = Expr::new(span, ExprNode::Var { id: VarId(0), name: cap.clone() });
    let concat = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(cap_ref),
            method: Symbol::from("+"),
            args: vec![to_s],
            block: None,
            parenthesized: false,
        },
    );
    *e.node = ExprNode::Assign {
        target: LValue::Var { id: VarId(0), name: cap.clone() },
        value: concat,
    };
    true
}
