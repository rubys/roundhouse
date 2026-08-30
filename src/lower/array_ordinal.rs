//! ActiveSupport's `Array#second` … `#fifth` → an index read.
//!
//! Rails ships these by reopening `Array`, which is a shape only the
//! CRuby overlay can host: the transpiled runtimes cannot reopen a
//! builtin, and spinel AOT cannot dispatch a user-defined method on
//! one. Each is defined as exactly one index read — activesupport's
//! `array/access.rb` is `def second; self[1]; end` — so the rewrite is
//! the definition, not an approximation, and the receiver is evaluated
//! once either way.
//!
//! campfire's `RoomMessagesChannel.guarded_stream?` is
//! `stream_name.to_s.split(":", 2).second == STREAM_SUFFIX`, and it is
//! the FIRST thing `RoomStreamsAreAuthorized` calls on a cable
//! subscribe. Un-lowered it reached the spinel binary as a dynamic
//! dispatch and answered `undefined method 'second' for an instance of
//! Array` — which, on a target where an unhandled error in a request
//! ends the process, took the server down on the first subscribe
//! rather than failing one subscription.
//!
//! ONLY A TYPED ARRAY RECEIVER. `second` also exists on
//! `ActiveRecord::Relation`, where it is `offset(1).first` — a query,
//! not an index read — and the runtime answers that one with a real
//! method and a real signature. An untyped receiver is left alone for
//! the same reason: a rewrite is only sound if the receiver is known
//! to be the Array whose definition is being inlined.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

/// `(name, index)` — activesupport's `array/access.rb`, verbatim.
/// `first` and `last` are Ruby's own and need no rewrite.
const ORDINALS: [(&str, i64); 4] =
    [("second", 1), ("third", 2), ("fourth", 3), ("fifth", 4)];

pub fn apply_array_ordinal_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);

    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &mut *expr.node else {
        return;
    };
    if !args.is_empty() {
        return;
    }
    let Some((_, index)) = ORDINALS.iter().find(|(name, _)| *name == method.as_str()) else {
        return;
    };
    if !matches!(recv.ty, Some(Ty::Array { .. })) {
        return;
    }
    let span = expr.span;
    let mut idx = Expr::new(span, ExprNode::Lit { value: Literal::Int { value: *index } });
    idx.ty = Some(Ty::Int);
    *method = Symbol::from("[]");
    let ExprNode::Send { args, .. } = &mut *expr.node else { return };
    args.push(idx);
}
