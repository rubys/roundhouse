//! ActiveSupport's core_ext reopens, grounded to a runtime function
//! instead of a reopen.
//!
//! Rails ships `index_by`, `many?`, `to_sentence` and `sole` on
//! `Enumerable`/`Array`, and `squish` on `String`, by reopening the
//! builtin — which is a shape
//! only the CRuby overlay can host: the transpiled runtimes cannot
//! reopen a builtin, and spinel AOT cannot dispatch a user-defined
//! method on one. The runtime already answers `index_by` on
//! `ActiveRecord::Relation` (relation.rb), and the twin for everything
//! else is one module function taking the collection as an argument —
//! the same rule `active_support_ext.rb` states for `blank?`: the
//! receiver is evaluated exactly once, so a receiver with effects
//! grounds too, and no `respond_to?` is needed.
//!
//! campfire builds `Sound::INDEX = BUILTIN.index_by(&:name)` in a CLASS
//! BODY, so an ungrounded call is not a late NoMethodError on some
//! route — it fires while `app/models.rb` is being required and the
//! tree does not boot.
//!
//! WHAT IT DOES NOT REWRITE: a receiver the analyzer typed as a
//! Relation. That one has a real method with a real RBS signature, and
//! routing it through the module function would trade a typed call for
//! an untyped one to fix nothing.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_enumerable_ext_grounding(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
    // Test bodies too — untyped at this point (they are typed in
    // `test_module_to_library`), so only the untyped-gated methods
    // ground there; `many?`/`to_sentence` want a typed Array receiver
    // and stay as written, as they always did in a test.
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body);
        }
    }
}

/// The same grounding over one TYPED body, for
/// `test_module_to_library` to run once a test's body has types — a
/// `<<~HTML.squish` in a test is a String only after that pass, and
/// this module's own walk runs before it. Sibling of
/// `array_ordinal::rewrite_body`, called from the same place.
pub(crate) fn rewrite_body(expr: &mut Expr) {
    rewrite(expr);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let span = expr.span;
    let ExprNode::Send { recv, method, args, block, parenthesized } = &mut *expr.node else {
        return;
    };
    // `index_by` takes the block and `many?` refuses one — the bare
    // call is the form Rails' counter-and-`any?` body reduces to a
    // length test, and the block form counts MATCHES instead, which is
    // a different question no corpus app asks.
    let wants_block = match method.as_str() {
        "index_by" => true,
        "many?" | "to_sentence" | "sole" | "squish" => false,
        _ => return,
    };
    if !args.is_empty() || block.is_some() != wants_block {
        return;
    }
    let Some(receiver) = recv.as_ref() else { return };
    if is_relation(receiver.ty.as_ref()) {
        return;
    }
    // `many?` names an `Array` parameter, so only an Array receiver
    // goes. `index_by` keeps the wider gate it has always had (its
    // parameter is untyped, and an untyped receiver is the case the
    // header explains). A Hash or String receiver here stays visible
    // rather than becoming a call whose argument does not fit.
    if matches!(method.as_str(), "many?" | "to_sentence")
        && !matches!(receiver.ty.as_ref(), Some(Ty::Array { .. }))
    {
        return;
    }
    // `squish` names a `String` parameter, so only a String receiver
    // goes — and it is the only one of these whose name a model could
    // plausibly define itself, which is the second reason to gate on
    // the analyzer's answer rather than on the spelling.
    if method.as_str() == "squish" && !matches!(receiver.ty.as_ref(), Some(Ty::Str)) {
        return;
    }
    let receiver = recv.take().expect("checked above");
    *recv = Some(Expr::new(
        span,
        ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
    ));
    args.push(receiver);
    *parenthesized = true;
}

/// `Ty::Relation` under any element type, and through a nullable union
/// — the shape a scope chain leaves behind.
fn is_relation(ty: Option<&Ty>) -> bool {
    match ty {
        Some(Ty::Relation { .. }) => true,
        Some(Ty::Union { variants }) => variants.iter().any(|v| is_relation(Some(v))),
        _ => false,
    }
}
