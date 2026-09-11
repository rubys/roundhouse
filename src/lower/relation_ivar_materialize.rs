//! A controller ivar assigned a Relation in one branch and an Array in
//! another is materialised on the Relation branch: `@messages =
//! Message.none` → `@messages = Message.none.to_a`.
//!
//! Rails does not care: a Relation and an Array answer the same
//! `count`/`each`/`empty?` a view asks, and `Relation#to_ary` papers
//! over the rest. A strict target does. campfire's searches controller
//! writes
//!
//! ```ruby
//! if query.present?
//!   @messages = Current.user.reachable_messages.search(query).last(100)
//! else
//!   @messages = Message.none
//! end
//! ```
//!
//! and the view lowerer types the `messages` parameter `Array[Message]`
//! from the ivar's NAME (`view_to_library::ivar_ty`). The emitted
//! program then declared one type and passed another, and spinel —
//! whose `--rbs` contract is that a signature is trusted — unboxed the
//! `Message.none` branch's Relation into the Array slot's neighbour and
//! read a table name out of an Array header: `SELECT COUNT(*) AS n FROM
//! 4393318048`, the search 500 on the compiled binary.
//!
//! The declaration is the one to keep. A view consumes a collection,
//! the Array is what every target emits for one, and the alternative —
//! widening the parameter to the union — puts a poly dispatch under
//! every use in the view for the sake of one `none`. So the Relation
//! branch is loaded at the assignment instead, which is exactly what the
//! view's first `each` would have done. `Relation#to_a` is declared
//! `Array[untyped]` on the runtime; the rewritten site is stamped
//! `Array[<model>]` so the ivar's union collapses to one shape.
//!
//! Gate: an ivar whose assignments across the controller's own bodies
//! join to BOTH an `Array` and a `Relation` (a nil is fine, it rides
//! along either). An ivar that is only ever a Relation is left alone —
//! that is a different, working contract (spinel types the parameter
//! from the call site), and rewriting it would materialise every
//! `@bots = Current.account.bots` in the app for no reader that needs
//! it.

use std::collections::HashMap;

use crate::app::App;
use crate::dialect::ControllerBodyItem;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_relation_ivar_materialize(app: &mut App) {
    for controller in &mut app.controllers {
        let mut ivars: HashMap<Symbol, Ty> = HashMap::new();
        for item in &controller.body {
            if let ControllerBodyItem::Action { action, .. } = item {
                crate::analyze::extract_ivar_assignments(&action.body, &mut ivars);
            }
        }
        let mixed: Vec<Symbol> = ivars
            .iter()
            .filter(|(_, ty)| is_array_and_relation(ty))
            .map(|(name, _)| name.clone())
            .collect();
        if mixed.is_empty() {
            continue;
        }
        for item in &mut controller.body {
            if let ControllerBodyItem::Action { action, .. } = item {
                rewrite(&mut action.body, &mixed);
            }
        }
    }
}

/// Both shapes present among the variants, whatever else rides along.
fn is_array_and_relation(ty: &Ty) -> bool {
    let Ty::Union { variants } = ty else { return false };
    variants.iter().any(|v| matches!(v, Ty::Array { .. }))
        && variants.iter().any(|v| matches!(v, Ty::Relation { .. }))
}

fn rewrite(expr: &mut Expr, mixed: &[Symbol]) {
    expr.node.for_each_child_mut(&mut |child| rewrite(child, mixed));
    let span = expr.span;
    if let ExprNode::Assign { target: LValue::Ivar { name }, value } = &mut *expr.node {
        if !mixed.contains(name) {
            return;
        }
        let Some(Ty::Relation { of }) = value.ty.as_ref() else { return };
        let elem = Ty::Class { id: of.clone(), args: vec![] };
        let inner = std::mem::replace(value, Expr::new(span, ExprNode::Lit { value: Literal::Nil }));
        let mut materialised = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(inner),
                method: Symbol::from("to_a"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        materialised.ty = Some(Ty::Array { elem: Box::new(elem) });
        *value = materialised;
        // The assignment's own type follows its value.
        expr.ty = value.ty.clone();
    }
}
