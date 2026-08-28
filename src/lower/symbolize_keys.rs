//! ActiveSupport `Hash#symbolize_keys` grounding: on a receiver the
//! analyzer stamped `Hash[Symbol, _]`, the call is the IDENTITY, so it
//! becomes the receiver.
//!
//! Like `blank?` and `parameterize`, `symbolize_keys` is a core_ext
//! reopen only the CRuby overlay can host — every transpiled runtime
//! either cannot reopen a builtin or, on spinel AOT, has no method to
//! dispatch at all. campfire's `WebPush::Notification`:
//!
//! ```text
//! { subject: "mailto:…" }.merge Rails.configuration.x.vapid.symbolize_keys
//! ```
//!
//! The config read now answers a symbol-keyed Hash (the lifted group
//! reader, `ingest::app`), so the conversion has nothing to do — and
//! spinel was passing the unresolved call's poly result into
//! `sp_SymPolyHash_merge`'s `sp_SymPolyHash *`.
//!
//! ONLY WHEN THE KEYS ARE ALREADY SYMBOLS. A `Hash[String, _]` receiver
//! keeps its dynamic call: converting it is real work — a new hash with
//! every key interned — and inventing that here would be a rewrite
//! nobody has priced, not a grounding. The residue is honest: CRuby
//! serves it through the overlay, and a strict target reports it.
//!
//! Receiver effects are not a concern the way they are in
//! `lower::blank`: the receiver is used exactly once, in the same
//! position, so evaluation order and count are unchanged.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ty::Ty;

pub fn apply_symbolize_keys_grounding(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let replacement = match &mut *expr.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "symbolize_keys"
                && args.is_empty()
                && matches!(r.ty.as_ref(), Some(Ty::Hash { key, .. }) if **key == Ty::Sym) =>
        {
            Some(r.clone())
        }
        _ => None,
    };
    if let Some(r) = replacement {
        *expr = r;
    }
}
