//! Bare instance-side `transaction { … }` in a MODEL body →
//! `ActiveRecord::Base.transaction { … }` (lobsters'
//! HatRequest#approve_by_user_for_reason! wraps its mutations bare).
//! Rails delegates the instance form to the class; our runtime's
//! transaction is flat and connection-global (BEGIN/COMMIT on the one
//! adapter), so grounding straight to Base is semantics-preserving —
//! and it avoids adding an instance `transaction` to the shared
//! runtime, where a same-named class/instance RBS pair collides in
//! the name-keyed signature matcher (runtime_src). Models only:
//! nothing else in the corpus calls bare `transaction`, and a helper
//! module's bare send should stay honest residue.
//!
//! "A model body" includes the method bodies inside an ASSOCIATION
//! EXTENSION block (`has_many :memberships do def revise(…) … end
//! end`) — campfire's `Room#memberships.revise` wraps its grant/revoke
//! pair in a bare `transaction`. Those bodies hang off the association
//! rather than off `model.body`, so the walk below has to reach them
//! explicitly; without that they compiled to a bare send and raised
//! NoMethodError at the first call, while the identical spelling one
//! method over worked.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_transaction_grounding(app: &mut App) {
    for model in &mut app.models {
        for item in &mut model.body {
            match item {
                crate::dialect::ModelBodyItem::Method { method, .. } => {
                    rewrite(&mut method.body);
                }
                crate::dialect::ModelBodyItem::Association {
                    assoc: crate::dialect::Association::HasMany { extension, .. },
                    ..
                } => {
                    for method in extension.iter_mut() {
                        rewrite(&mut method.body);
                    }
                }
                _ => {}
            }
        }
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    if let ExprNode::Send { recv, method, args, block, .. } = &mut *expr.node {
        // Bare `transaction do` and `self.transaction do` (lobsters
        // spells it with the explicit receiver).
        let instance_recv = match recv {
            None => true,
            Some(r) => matches!(&*r.node, ExprNode::SelfRef),
        };
        if instance_recv
            && method.as_str() == "transaction"
            && args.is_empty()
            && block.is_some()
        {
            *recv = Some(Expr::new(
                expr.span,
                ExprNode::Const {
                    path: vec![Symbol::from("ActiveRecord"), Symbol::from("Base")],
                },
            ));
        }
    }
}
