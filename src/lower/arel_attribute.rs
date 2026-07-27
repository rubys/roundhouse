//! `Model.arel_table[:col]` → `Model.arel_table.attribute(:col)`.
//!
//! The Arel shim's qualified-column read is spelled `[]` in Rails, and
//! `Arel::Table` used to live on the CRuby overlay where that was
//! harmless. It now lives in the shared ruby-family runtime so spinel
//! can compile lobsters' `Tag.arel_table[:id]` — and an indexer def on
//! a small immutable class is a landmine there.
//!
//! Spinel gives `Arel::Table` a by-value representation, so
//! `Table#[]` compiles to `sp_Table__lb_rb(sp_Table self, …)`. Every
//! *poly* `[]` site in the whole program then gets an arm for it and
//! hands that by-value parameter a `sp_Table *` — `passing 'sp_Table *'
//! to parameter of incompatible type 'sp_Table'`, at each unrelated
//! `x[k]` in the tree. One class defining `[]` breaks indexing
//! everywhere; the framework router/view_helpers/ac_base suites caught
//! it in twelve places at once.
//!
//! So the runtime spells the reader `attribute` — a name no poly
//! dispatcher is crowded with — and the Rails spelling is lowered to it
//! here. Same move the overlay's own header predicted when it wrote
//! that another target would want "`attribute(col)` plus an emit-side
//! index lowering".
//!
//! Keyed on the receiver's stamped type, so an `Arel::Table` is the
//! only thing rewritten and every other `[]` in the app is untouched.
//! Views are walked: the type is what selects, not the syntax, so
//! there is no per-target view-cond vocabulary to miss the way the
//! blank pass's rewrite would.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_arel_attribute_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

/// Is this the Arel table shim? Matched on the last path segment, the
/// same resolution `Ty::Class` receivers get in the blank pass.
fn is_arel_table(ty: Option<&Ty>) -> bool {
    let Some(Ty::Class { id, .. }) = ty else { return false };
    let raw = id.0.as_str();
    raw.rsplit("::").next().unwrap_or(raw) == "Table"
        && (raw == "Table" || raw.starts_with("Arel::"))
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *expr.node
    else {
        return;
    };
    if method.as_str() != "[]" || args.len() != 1 || !is_arel_table(r.ty.as_ref()) {
        return;
    }
    *method = Symbol::from("attribute");
}
