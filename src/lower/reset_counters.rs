//! `Model.reset_counters(id, :assoc, …)` — recount each named has_many
//! into its counter-cache column:
//!
//!   Model.where(id: id).update_all(assoc_count: Target.where(fk: id).count)
//!
//! Rails resolves the column through the inverse `belongs_to
//! counter_cache:`; the default it computes is `<has_many name>_count`,
//! and the rewrite fires only when the owner's table HAS that column,
//! so a custom-named cache is left alone (and still refused) rather
//! than recounted into the wrong place. The id is read twice, so it
//! must be a pure read. lobsters' Origin recounts its stories this way
//! after create.

use crate::app::App;
use crate::dialect::{Association, Model};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

pub fn apply_reset_counters_lowering(app: &mut App) {
    let models = app.models.clone();
    let schema = app.schema.clone();
    super::for_each_hook_body(app, &mut |body| rewrite(body, &models, &schema));
}

fn rewrite(expr: &mut Expr, models: &[Model], schema: &crate::schema::Schema) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, models, schema));
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "reset_counters" || args.len() < 2 {
        return;
    }
    let ExprNode::Const { path } = &*recv.node else { return };
    let Some(owner) = models.iter().find(|m| m.name.0.as_str() == path_str(path)) else { return };
    let Some(table) = schema.tables.get(&owner.table.0) else { return };
    let id = &args[0];
    if !super::case_lambda::is_pure_read(id) {
        return;
    }
    let span = expr.span;
    let mut updates: Vec<(Expr, Expr)> = Vec::new();
    for a in &args[1..] {
        let ExprNode::Lit { value: Literal::Sym { value: assoc } } = &*a.node else { return };
        let Some((target, fk)) = owner.associations().find_map(|asc| match asc {
            Association::HasMany { name, target, foreign_key, through: None, as_interface: None, .. }
                if name == assoc =>
            {
                Some((target.clone(), foreign_key.clone()))
            }
            _ => None,
        }) else {
            return;
        };
        let column = format!("{}_count", assoc.as_str());
        if !table.columns.iter().any(|c| c.name.as_str() == column) {
            return;
        }
        let count = typed(
            send(where_eq(&target, &fk, id, span), "count", vec![], span),
            Ty::Int,
        );
        updates.push((sym(&column, span), count));
    }
    let owner_rel = where_eq(&owner.name, &Symbol::from("id"), id, span);
    let hash = Expr::new(span, ExprNode::Hash { entries: updates, kwargs: true });
    *expr = typed(send(owner_rel, "update_all", vec![hash], span), Ty::Int);
}

fn path_str(path: &[Symbol]) -> String {
    path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::")
}

fn typed(mut e: Expr, ty: Ty) -> Expr {
    e.ty = Some(ty);
    e
}

fn sym(name: &str, span: Span) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(name) } })
}

fn send(recv: Expr, method: &str, args: Vec<Expr>, span: Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

/// `Model.where(key: value)`, typed as the model's relation.
fn where_eq(model: &ClassId, key: &Symbol, value: &Expr, span: Span) -> Expr {
    let class = Expr::new(
        span,
        ExprNode::Const { path: model.0.as_str().split("::").map(Symbol::from).collect() },
    );
    let cond = Expr::new(
        span,
        ExprNode::Hash { entries: vec![(sym(key.as_str(), span), value.clone())], kwargs: true },
    );
    typed(send(class, "where", vec![cond], span), Ty::Relation { of: model.clone() })
}
