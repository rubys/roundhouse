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
//! "A model body" includes two families that do not live in
//! `model.body`, and each one was found only after the previous fix
//! shipped:
//!
//! * ASSOCIATION EXTENSION blocks (`has_many :memberships do def
//!   revise(…) … end end`) — campfire's `Room#memberships.revise`
//!   wraps its grant/revoke pair in a bare `transaction`. Those hang
//!   off the association.
//! * MODEL CONCERNS (`app/models/user/bannable.rb`, emitted as
//!   `module User::Bannable` inside `class User`) — their methods run
//!   on model instances exactly like the ones in `user.rb`, and
//!   campfire spells `transaction do` in three of them
//!   (`Bannable#ban`, `#unban`, `Bot#regenerate_key`). They live in
//!   `app.library_classes`, so `user.rb`'s own `deactivate` grounded
//!   while the concern one file over raised NoMethodError.
//!
//! A concern is recognized by its NAMESPACE naming a model
//! (`User::Bannable`), which is also how it is emitted — nested inside
//! `class User`. A plain helper module is not a model concern and
//! keeps its bare send as honest residue, per the paragraph above.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_transaction_grounding(app: &mut App) {
    // `for_each_model_body` is the shared walk over the three families
    // this pass needed one at a time (model methods, association
    // extensions, model concerns) — see its note for why it exists.
    super::for_each_model_body(app, &mut rewrite);
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
