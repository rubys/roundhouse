//! `Pathname(path)` → `Pathname.new(path)`.
//!
//! Kernel's conversion FUNCTION, not a method on anything. CRuby mixes
//! it into Object when `pathname` is required, and it is defined as
//! exactly `Pathname.new(path)` — the only difference being that an
//! argument which is already a Pathname comes back identical rather
//! than as an equal copy, which no call site can tell apart (Pathname
//! is a string with methods; `==` and `hash` are the string's).
//!
//! Spinel's bundled `pathname` says so itself, in the package header:
//! the constructor "cannot be spelled in Spinel yet (a toplevel method
//! named after a class collides with the class's own symbol) -- use
//! Pathname.new". So this is not a workaround for a missing library —
//! the library is there, the CLASS-side spelling is the one it offers,
//! and it is the spelling every strict target can resolve without a
//! toplevel-function surface ([[feedback_runtime_must_be_statically_resolvable]]).
//!
//! campfire's `CableHelper.script_aware_action_cable_meta_tag` builds
//! the Action Cable URL with `Pathname(request.script_name) +
//! Pathname(mount_path)`, and the layout renders it into the `<head>`
//! of every page — so the bare call stopped the spinel build on a line
//! that runs on every request.
//!
//! Only the one-argument form is rewritten, which is the only form
//! Kernel defines.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub fn apply_pathname_ctor_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);

    let ExprNode::Send { recv: None, method, args, block: None, .. } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "Pathname" || args.len() != 1 {
        return;
    }
    let span = expr.span;
    let mut konst = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("Pathname")] });
    konst.ty = Some(Ty::Class { id: ClassId(Symbol::from("Pathname")), args: vec![] });
    *method = Symbol::from("new");
    let ExprNode::Send { recv, .. } = &mut *expr.node else { return };
    *recv = Some(konst);
    // The result type is left exactly as analyze left it. Pathname is
    // not in the class registry, so stamping the instance here would
    // put a receiver on the ledger that nothing can answer methods for
    // — a dispatch failure invented by a rewrite, which is the shape
    // [[project_campfire_emit_ledger_zero]] spent six commits removing.
}
