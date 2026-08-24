//! `Model.destroy_by(col: value)` → `Model.where(col: value).destroy_all`,
//! and `delete_by` → `delete_all` beside it.
//!
//! That rewrite is Rails' own definition of the method, which is a
//! one-liner over `where`; nothing about it is specific to a target.
//! Writing it here rather than as a `Base` runtime method is the
//! difference between one shared lowering and nine primitives: a method
//! defined on `Base` is a method on EVERY model in EVERY target, and
//! the last attempt at that reddened two CI jobs because the strict
//! targets have to give the conditions parameter a type, and a
//! conditions hash is a different shape at every call site.
//!
//! Runs beside `exists_conditions`, which splits `exists?` the same way
//! and for the same reason, and shares its two outcomes: literal
//! conditions get folded into one statement by the arel rewrite, a
//! runtime value falls to the Relation.
//!
//! Rails accepts the id/positional forms of `exists?` but `destroy_by`
//! takes conditions only, so the kwargs guard here is about what the
//! synthesized `where` can carry, not about telling two methods apart.
//! The other shapes `where` accepts (a String fragment, a positional
//! hash) keep the Relation seed `scope_chain::CLASS_ROOT_TERMINALS`
//! gives them, and so does the association-receiver form
//! (`room.memberships.destroy_by user: users`) — an association read is
//! Array-typed, not Relation-typed, so it never reaches this pass.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_destroy_by_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);

    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    let terminal = match method.as_str() {
        "destroy_by" => "destroy_all",
        "delete_by" => "delete_all",
        _ => return,
    };
    if args.len() != 1 {
        return;
    }
    let ExprNode::Hash { kwargs: true, entries } = &*args[0].node else {
        return;
    };
    if entries.is_empty() {
        return;
    }
    // The synthesized `where` hop is never seen by analyze, so its type
    // has to be written here or the hop reads out as an unstamped send
    // on a typed receiver — `send_dispatch_failed` naming `where`,
    // which is the one method every model has. Both a class receiver
    // and an already-built relation land on the same relation type.
    let relation_ty = match recv.ty.as_ref() {
        Some(Ty::Class { id, .. }) => Ty::Relation { of: id.clone() },
        Some(Ty::Relation { of }) => Ty::Relation { of: of.clone() },
        _ => return,
    };
    // Rails hands back the destroyed records from `destroy_all` and the
    // affected-row count from `delete_all`; the two `Relation` catalog
    // entries say exactly that (`ArrayOfSelf` / `Int`), and the stamps
    // below repeat them rather than inventing a third answer.
    let result_ty = match (terminal, &relation_ty) {
        ("destroy_all", Ty::Relation { of }) => Ty::Array {
            elem: Box::new(Ty::Class { id: of.clone(), args: vec![] }),
        },
        _ => Ty::Int,
    };

    let span = expr.span;
    let mut where_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv.clone()),
            method: Symbol::from("where"),
            args: vec![args[0].clone()],
            block: None,
            parenthesized: true,
        },
    );
    where_call.ty = Some(relation_ty);
    *expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(where_call),
            method: Symbol::from(terminal),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    expr.ty = Some(result_ty);
}
