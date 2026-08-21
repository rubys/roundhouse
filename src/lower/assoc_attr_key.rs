//! `record.update!(creator: user)` → `record.update!(creator_id: user.id)`.
//!
//! Rails routes a mass-assignment hash through the model's writers, so
//! an ASSOCIATION NAME is as assignable as a column. The synthesized
//! `update` / `update!` enumerate columns and virtual writers and
//! deliberately claim association names without emitting anything for
//! them (see `model_to_library::schema`'s note): an attrs Hash is
//! `HashMap<String, serde_json::Value>` on every strict target, and a
//! model instance cannot be a `serde_json::Value`, so reading
//! `attrs[:creator].id` out of one is a compile error rather than a
//! feature.
//!
//! That note priced the cost at nil. It was not nil — it was a SILENT
//! DROP. campfire's `rooms_controller_test` writes
//! `rooms(:designers).update! creator: users(:jz)` and then expects the
//! new creator to be able to destroy the room; the assignment vanished,
//! `can_administer?` stayed false, and the test read as a permissions
//! failure with nothing pointing at mass assignment.
//!
//! The fix belongs at the CALL SITE, which is the one place the record
//! is still a typed expression rather than a hash value — exactly the
//! treatment `where(user: user)` already gets from
//! `scope_chain::lower_relation_args`, and for exactly the same reason.
//! The key becomes the foreign-key column and the value becomes its
//! `id`, so what reaches the attrs Hash is an Integer.
//!
//! KEYED ON THE NAME, NOT ON A TYPE, under the uniqueness rule this
//! codebase holds association names to everywhere else
//! (`has_many_by_name`, `sole_scope_owner`, `owner_model_from_name`).
//! MEASURED, not assumed: the receivers at these sites analyze to
//! `None` / `Untyped` / an open `Ty::Var` — a test body's
//! `rooms(:designers)` carries no stamped type at all — so a
//! type-directed gate fires on nothing. What IS knowable is that
//! `creator` names a `belongs_to` whose foreign key is `creator_id` on
//! every model that declares it; two models disagreeing on the fk, or a
//! COLUMN anywhere in the app sharing the name, both decline.
//!
//! A LITERAL `nil` value declines too, with residue: Rails writes NULL
//! and `<nil>.id` would raise. A non-literal that happens to be nil at
//! runtime raises rather than silently dropping, which is the trade
//! this whole pass is making.

use std::collections::HashMap;

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::Association;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

/// The mass-assignment entries that take an attrs hash on a RECORD.
/// `create!` / `new` are NOT here: `synth_initialize` already reads an
/// association key through a Cast, and rust's constructor path strips
/// the statement that would otherwise not compile.
const ASSIGNERS: &[&str] = &["update", "update!", "assign_attributes"];

/// belongs_to name -> its foreign-key column, when every model
/// declaring the name agrees on it. A disagreement removes the entry.
type BelongsTo = HashMap<Symbol, Symbol>;

pub fn apply_assoc_attr_key_lowering(app: &mut App) -> Vec<Diagnostic> {
    let mut table: BelongsTo = HashMap::new();
    let mut conflicted: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
    for m in &app.models {
        for a in m.associations() {
            if let Association::BelongsTo { name, foreign_key, .. } = a {
                match table.get(name) {
                    Some(existing) if existing != foreign_key => {
                        conflicted.insert(name.clone());
                    }
                    Some(_) => {}
                    None => {
                        table.insert(name.clone(), foreign_key.clone());
                    }
                }
            }
        }
    }
    // A name that is also a COLUMN somewhere is not unambiguously an
    // association key — the column loop in the synthesized `update`
    // already claims it, and rewriting would send the value through the
    // wrong slot.
    for t in app.schema.tables.values() {
        for c in &t.columns {
            conflicted.insert(c.name.clone());
        }
    }
    for n in &conflicted {
        table.remove(n);
    }
    if table.is_empty() {
        return Vec::new();
    }
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |b| rewrite(b, &table, &mut diags));
    super::for_each_test_body(app, &mut |b| rewrite(b, &table, &mut diags));
    for view in &mut app.views {
        rewrite(&mut view.body, &table, &mut diags);
    }
    diags
}

fn residue(expr: &Expr, assoc: &str, reason: &str) -> Diagnostic {
    crate::lower::residue_diagnostic(
        "assoc_attr_key",
        "association mass-assignment",
        expr.span,
        reason,
        format!(
            "`{assoc}:` in a mass-assignment hash is left as-is ({reason}) — the \
             synthesized `update` writes columns, so the key is DROPPED; pass \
             `{assoc}_id:` or give the value a resolvable model type"
        ),
    )
}

fn rewrite(expr: &mut Expr, table: &BelongsTo, diags: &mut Vec<Diagnostic>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, table, diags));
    let matches_shape = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(_), method, args, block: None, .. }
            if ASSIGNERS.contains(&method.as_str())
                && args.len() == 1
                && matches!(&*args[0].node, ExprNode::Hash { .. })
    );
    if !matches_shape {
        return;
    }
    let span = expr.span;
    let ExprNode::Send { args, .. } = &mut *expr.node else { unreachable!() };
    let ExprNode::Hash { entries, .. } = &mut *args[0].node else { unreachable!() };
    for (k, v) in entries.iter_mut() {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
            continue;
        };
        let Some(fk) = table.get(key) else { continue };
        if matches!(&*v.node, ExprNode::Lit { value: Literal::Nil }) {
            diags.push(residue(&*k, key.as_str(), "an explicit nil writes NULL, not an id"));
            continue;
        }
        let mut id_read = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(v.clone()),
                method: Symbol::from("id"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        id_read.ty = Some(Ty::Int);
        *v = id_read;
        let mut new_key =
            Expr::new(k.span, ExprNode::Lit { value: Literal::Sym { value: fk.clone() } });
        new_key.ty = Some(Ty::Sym);
        *k = new_key;
    }
}
