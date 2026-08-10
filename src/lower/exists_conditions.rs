//! `Model.exists?(col: value)` → `Model.where(col: value).exists?`.
//!
//! Rails' `exists?` is three methods behind one name: no argument asks
//! whether the table has any row, an id asks for that row, and a
//! conditions hash is `where(conditions).exists?`. The runtime models
//! the id form only — `Base.exists?(id)` hands straight to
//! `_adapter_exists_by_id?` — so a conditions hash arrived at
//! `Db.escape_int` and died there (`undefined method 'to_i' for an
//! instance of Hash`).
//!
//! Splitting the name here rather than making the runtime method
//! polymorphic keeps the primitive monomorphic: an `is_a?(Hash)` branch
//! inside `exists?` would give it an untyped parameter, and every
//! strict target would then carry a runtime type test for a question
//! answered at compile time by the shape of the call.
//!
//! Runs before the arel rewrite, so the two possible outcomes are both
//! improvements on today:
//!
//!   * literal conditions — arel folds the whole `where(…).exists?`
//!     chain into one `SELECT 1 … LIMIT 1`, as it already does for a
//!     `Model.where(…)` written out longhand.
//!   * a runtime value (a method parameter, a call) — arel deliberately
//!     declines to inline those, since Rails decides per value whether
//!     it means `=`, `IN` or `IS NULL`, and the chain falls to the
//!     Relation. Which is exactly what the arel builder's own comment
//!     says should happen; it just had nowhere correct to land.
//!
//! The kwargs guard is what keeps the id form (and `File.exists?(path)`,
//! whose argument is positional) out of the rewrite.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_exists_conditions_lowering(app: &mut App) {
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
    if method.as_str() != "exists?" || args.len() != 1 {
        return;
    }
    if !matches!(&*recv.node, ExprNode::Const { .. }) {
        return;
    }
    // Only the conditions form: a bare keyword hash. An id is
    // positional, and so is the path `File.exists?` takes.
    let ExprNode::Hash { kwargs: true, entries } = &*args[0].node else {
        return;
    };
    if entries.is_empty() {
        return;
    }

    let span = expr.span;
    let conditions = args[0].clone();
    let where_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv.clone()),
            method: Symbol::from("where"),
            args: vec![conditions],
            block: None,
            parenthesized: true,
        },
    );
    *expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(where_call),
            method: Symbol::from("exists?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    expr.ty = Some(crate::ty::Ty::Bool);
}
