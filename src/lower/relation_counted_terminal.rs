//! Shared lowering: `first(n)` / `last(n)` on a receiver the analyzer
//! typed as a Relation become the runtime's `first_n` / `last_n`.
//!
//! The runtime splits the counted forms from the bare ones because one
//! method cannot carry both return types on a strict target: `last`
//! answers a record or nil, `last(n)` an Array (see
//! `scope_chain::counted_terminal`, which renames the chains it can
//! prove relations syntactically, on the Ruby emit path). A typed
//! receiver needs no syntax to prove it: campfire's `Page.load` takes
//! its relation as a parameter, and once `send_dispatch` grounds its
//! `public_send(direction, size)` the arms read
//! `relation.skip_preloading!.last(size)`, which reached the zero-arg
//! `last` and raised ArgumentError.
//!
//! Only `Ty::Relation` counts. `Ty::Array` receivers keep `first(n)`:
//! Array#first(n) is the same method Ruby means, and the analyzer types
//! some scope results as Arrays, so a rename there could corrupt a real
//! Array. A block form is Enumerable#detect and is left alone.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_relation_counted_terminals(app: &mut App) {
    super::for_each_hook_body(app, &mut |body| rewrite(body));
}

fn rewrite(e: &mut Expr) {
    e.node.for_each_child_mut(&mut |c| rewrite(c));
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *e.node {
        if args.len() == 1 && matches!(r.ty, Some(Ty::Relation { .. })) {
            let renamed = match method.as_str() {
                "first" => "first_n",
                "last" => "last_n",
                _ => return,
            };
            *method = Symbol::from(renamed);
        }
    }
}
