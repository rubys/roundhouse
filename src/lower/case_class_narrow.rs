//! `case <read> when <Model>` — inside that arm the read IS a `<Model>`.
//!
//! Ruby's `Model === x` is `x.is_a?(Model)`, so an arm's body sees the
//! scrutinee narrowed. The analyzer narrows locals, not an expression
//! like `ma.item`, and a POLYMORPHIC reader types as nothing in
//! particular — which left lobsters' mod-activity log building
//! `item_path(ma.item)` inside `when ModMail` (no such route; spinel
//! refused the call). Here each re-read of the scrutinee inside a
//! `when <Model>` arm becomes a `Cast` to that model: the value is
//! unchanged (a class-targeted Cast emits as the bare read), and its
//! type now says which record it is, so the record-URL lowering names
//! `mod_mail_path` and the route-param pass projects the slug.
//!
//! Views only, where a template switches on a polymorphic record to
//! pick its markup. The scrutinee must be a pure read (a name or a
//! zero-arg reader chain) so "the same expression" is decidable and
//! re-reading it is free; the arm's pattern must be exactly one
//! constant that names a model.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Pattern};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub fn apply_case_class_narrowing(app: &mut App) {
    let models: std::collections::HashSet<String> =
        app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
    for view in &mut app.views {
        rewrite(&mut view.body, &models);
    }
}

fn rewrite(expr: &mut Expr, models: &std::collections::HashSet<String>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, models));
    let ExprNode::Case { scrutinee, arms } = &mut *expr.node else { return };
    if !super::case_lambda::is_pure_read(scrutinee)
        || !matches!(&*scrutinee.node, ExprNode::Send { recv: Some(_), .. })
    {
        return;
    }
    for arm in arms.iter_mut() {
        let Pattern::Expr { expr: pat } = &arm.pattern else { continue };
        let ExprNode::Const { path } = &*pat.node else { continue };
        let name = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
        if !models.contains(&name) {
            continue;
        }
        let ty = Ty::Class { id: ClassId(Symbol::from(name.as_str())), args: vec![] };
        substitute(&mut arm.body, scrutinee, &ty);
    }
}

fn substitute(e: &mut Expr, scrutinee: &Expr, ty: &Ty) {
    if same_read(e, scrutinee) {
        let span = e.span;
        let value = e.clone();
        *e = Expr::new(span, ExprNode::Cast { value, target_ty: ty.clone() });
        e.ty = Some(ty.clone());
        return;
    }
    e.node.for_each_child_mut(&mut |c| substitute(c, scrutinee, ty));
}

/// Two pure reads that read the same thing: the same name, or the same
/// zero-arg reader on the same receiver.
fn same_read(a: &Expr, b: &Expr) -> bool {
    match (&*a.node, &*b.node) {
        (ExprNode::Var { name: x, .. }, ExprNode::Var { name: y, .. }) => x == y,
        (ExprNode::Ivar { name: x }, ExprNode::Ivar { name: y }) => x == y,
        (ExprNode::SelfRef, ExprNode::SelfRef) => true,
        (ExprNode::Const { path: x }, ExprNode::Const { path: y }) => x == y,
        (
            ExprNode::Send { recv: rx, method: mx, args: ax, block: None, .. },
            ExprNode::Send { recv: ry, method: my, args: ay, block: None, .. },
        ) => {
            mx == my
                && ax.is_empty()
                && ay.is_empty()
                && match (rx, ry) {
                    (Some(x), Some(y)) => same_read(x, y),
                    (None, None) => true,
                    _ => false,
                }
        }
        _ => false,
    }
}
