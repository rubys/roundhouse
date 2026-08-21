//! `content.attachables.grep(User)` → the records those attachments
//! point at, as one query.
//!
//! Rails' `Content#attachables` dereferences every attachment node's
//! signed GlobalID; this runtime answered `[]` because there is no
//! GlobalID registry to turn a name back into a class, and building one
//! means reflection or per-model registration at load.
//!
//! NEITHER IS NEEDED, because the only shape that consumes the list
//! names its class AT THE CALL SITE. `grep(User)` is Ruby's
//! `select { |x| User === x }`, so the caller has already told the
//! compiler which model it wants — and that turns an unresolvable
//! name-to-class lookup into a literal:
//!
//! ```text
//! body.attachables.grep(User)
//!   → User.where(id: body.attachable_ids("User")).to_a
//! ```
//!
//! `attachable_ids` (runtime/ruby/action_text.rb) verifies each node's
//! sgid, keeps the ones minted for that model name, and answers their
//! ids in document order without repeats — so a trailing `.uniq` in the
//! source is a no-op rather than a wrong answer (it would be: `uniq` on
//! records compares by object identity here, and two reads of one row
//! are two objects).
//!
//! A stale sgid — a record deleted since the message was written —
//! drops out of the `where` rather than raising, which is the behaviour
//! `attachables` had when it answered `[]` and is what Rails' own
//! MissingAttachable stands in for.
//!
//! Only the `grep(<Const>)` shape rewrites. A bare `attachables` keeps
//! its documented `[]`, because without a named class there is nothing
//! to key the query on — that half still wants a registry.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

pub fn apply_attachables_grep_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    super::for_each_test_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    // `<content>.attachables.grep(<Const>)` — zero-arg, block-free on
    // the inner hop, one Const argument on the outer.
    let parts = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "grep" && args.len() == 1 =>
        {
            let ExprNode::Const { path } = &*args[0].node else { return };
            match &*r.node {
                ExprNode::Send { recv: Some(content), method: m, args: a, block: None, .. }
                    if m.as_str() == "attachables" && a.is_empty() =>
                {
                    Some((content.clone(), path.clone()))
                }
                _ => None,
            }
        }
        _ => None,
    };
    let Some((content, path)) = parts else { return };
    let span = expr.span;
    let class_name = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
    let mut name_lit = Expr::new(
        span,
        ExprNode::Lit { value: Literal::Str { value: class_name } },
    );
    name_lit.ty = Some(Ty::Str);
    let mut ids = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(content),
            method: Symbol::from("attachable_ids"),
            args: vec![name_lit],
            block: None,
            parenthesized: true,
        },
    );
    ids.ty = Some(Ty::Array { elem: Box::new(Ty::Int) });
    let cond = Expr::new(
        span,
        ExprNode::Hash {
            entries: vec![(
                Expr::new(
                    span,
                    ExprNode::Lit { value: Literal::Sym { value: Symbol::from("id") } },
                ),
                ids,
            )],
            kwargs: true,
        },
    );
    let where_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path })),
            method: Symbol::from("where"),
            args: vec![cond],
            block: None,
            parenthesized: true,
        },
    );
    *expr.node = ExprNode::Send {
        recv: Some(where_call),
        method: Symbol::from("to_a"),
        args: vec![],
        block: None,
        parenthesized: false,
    };
    let _ = Span::synthetic();
}
