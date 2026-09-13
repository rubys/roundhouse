//! `record.save(validate: false)` / `record.save!(validate: false)` →
//! `record.save_after_validation`.
//!
//! Rails' `validate: false` skips validations and their callbacks and
//! runs the save callbacks — which is exactly the post-validation half
//! of the runtime's `save`, already extracted as
//! `save_after_validation` for `update_attribute` and the has_many
//! `grant_to` expansion to enter. The kwarg is the same entry point
//! spelled at the call site.
//!
//! The runtime's `save` and `save!` take no arguments, and that is
//! right for every other caller: a kwarg on a zero-arity method is an
//! ArgumentError under CRuby and, on spinel, silently dropped
//! (matz/spinel#4436) — which VALIDATED the row the caller was planting
//! precisely because it would not validate. campfire's
//! push_subscriptions test does that to seed a legacy row
//! (`legacy.save!(validate: false)`) and then asserts the re-POST is
//! rejected; the seed itself was rejected first.
//!
//! The bang form loses nothing: `save!(validate: false)` cannot raise
//! `RecordInvalid` (nothing validated), and the runtime's
//! `save_after_validation` has no abortable callbacks to turn into
//! `RecordNotSaved`. A `validate: true` — Rails' default, written out —
//! is left as the plain call it means. Any other kwarg is not this
//! pass's to read.
//!
//! Shape-directed, not type-directed: `save(validate: false)` names no
//! method but ActiveRecord's, so the receiver's type adds nothing the
//! spelling has not already said.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub fn apply_save_without_validation_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    super::for_each_test_body(app, &mut rewrite);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv: Some(_), method, args, block: None, .. } = &mut *expr.node else {
        return;
    };
    if !matches!(method.as_str(), "save" | "save!") {
        return;
    }
    let [only] = args.as_slice() else { return };
    let ExprNode::Hash { entries, kwargs: true } = &*only.node else { return };
    let [(k, v)] = entries.as_slice() else { return };
    let key_is_validate = matches!(&*k.node,
        ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "validate");
    let value_is_false = matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { value: false } });
    if !(key_is_validate && value_is_false) {
        return;
    }
    *method = Symbol::from("save_after_validation");
    args.clear();
}
