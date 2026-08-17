//! `where(role: :bot)` → `where(role: 2)`.
//!
//! Rails' `enum :role, %i[member administrator bot]` lets every query
//! and assignment name the LABEL where the column stores an integer;
//! ActiveRecord translates at runtime. The declaration itself expands at
//! ingest into scopes, predicates and bang writers that already carry
//! the stored value — but a HAND-WRITTEN label (`User.new(role:
//! :administrator)`, `where.not(role: :bot)`, `update! status:
//! :deactivated`) had nothing to translate it, so the symbol was
//! assigned or compared verbatim. `User.new(role: :administrator)
//! .administrator?` answered false, because the predicate the same
//! declaration generated compares against `1`.
//!
//! `Model::enums` exists for exactly this and had no consumer: its own
//! doc comment says it "is the only thing that can tell a hand-written
//! `where(role: :bot)` which integer `:bot` means". This is that
//! consumer.
//!
//! **Keyed by COLUMN, not by receiver.** Resolving the model at every
//! call site means typing `active.where(role: :bot)`'s receiver, and
//! the mapping is a property of the column anyway. The narrowing that
//! makes this safe is threefold: the call has to be one of
//! ActiveRecord's attribute-taking methods (so an ARIA `role: :button`
//! in a view helper is untouched), the key has to name a column some
//! model declares an enum on, and the VALUE has to be one of that
//! enum's labels. A column two models map DIFFERENTLY is dropped
//! entirely rather than guessed at.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};

/// ActiveRecord methods that take column=>value attribute hashes. A
/// symbol under any other call is somebody else's keyword.
const ATTR_METHODS: &[&str] = &[
    "new", "create", "create!", "build", "first_or_create", "first_or_create!",
    "first_or_initialize", "where", "not", "rewhere", "find_by", "find_by!",
    "find_or_create_by", "find_or_create_by!", "find_or_initialize_by",
    "exists?", "update", "update!", "update_all", "update_attribute",
    "update_columns", "assign_attributes", "where_scope",
    // `user_params.merge(role: :administrator)` — campfire builds the
    // attribute hash and hands it straight to `User.new`.
    "merge",
];

type EnumMap = std::collections::HashMap<String, std::collections::HashMap<String, Literal>>;

pub fn apply_enum_symbol_lowering(app: &mut App) {
    let map = enum_columns(app);
    if map.is_empty() {
        return;
    }
    super::for_each_hook_body(app, &mut |e| rewrite(e, &map));
    for view in &mut app.views {
        rewrite(&mut view.body, &map);
    }
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, &map);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body, &map);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body, &map);
        }
    }
}

/// column → (label → stored value), for every enum column the app
/// declares CONSISTENTLY. A column two models map differently is not in
/// the result: the rewrite is keyed by column name, so an inconsistent
/// mapping has no single right answer and guessing one would write the
/// wrong integer into a real query.
fn enum_columns(app: &App) -> EnumMap {
    let mut out: EnumMap = Default::default();
    let mut conflicted: std::collections::HashSet<String> = Default::default();
    for model in &app.models {
        for (column, labels) in &model.enums {
            let col = column.as_str().to_string();
            if conflicted.contains(&col) {
                continue;
            }
            let mapping: std::collections::HashMap<String, Literal> =
                labels.iter().cloned().collect();
            match out.get(&col) {
                Some(existing) if !same_mapping(existing, &mapping) => {
                    out.remove(&col);
                    conflicted.insert(col);
                }
                Some(_) => {}
                None => {
                    out.insert(col, mapping);
                }
            }
        }
    }
    out
}

fn same_mapping(
    a: &std::collections::HashMap<String, Literal>,
    b: &std::collections::HashMap<String, Literal>,
) -> bool {
    a.len() == b.len()
        && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| literal_eq(v, w)))
}

fn literal_eq(a: &Literal, b: &Literal) -> bool {
    match (a, b) {
        (Literal::Int { value: x }, Literal::Int { value: y }) => x == y,
        (Literal::Str { value: x }, Literal::Str { value: y }) => x == y,
        (Literal::Sym { value: x }, Literal::Sym { value: y }) => x == y,
        _ => false,
    }
}

fn rewrite(expr: &mut Expr, map: &EnumMap) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, map));
    let ExprNode::Send { method, args, .. } = &mut *expr.node else {
        return;
    };
    if !ATTR_METHODS.contains(&method.as_str()) {
        return;
    }
    for arg in args.iter_mut() {
        let ExprNode::Hash { entries, .. } = &mut *arg.node else { continue };
        for (key, value) in entries.iter_mut() {
            let ExprNode::Lit { value: Literal::Sym { value: col } } = &*key.node else {
                continue;
            };
            let Some(mapping) = map.get(col.as_str()) else { continue };
            let ExprNode::Lit { value: Literal::Sym { value: label } } = &*value.node else {
                continue;
            };
            let Some(stored) = mapping.get(label.as_str()) else { continue };
            *value = crate::lower::typing::with_ty(
                Expr::new(value.span, ExprNode::Lit { value: stored.clone() }),
                match stored {
                    Literal::Int { .. } => crate::ty::Ty::Int,
                    _ => crate::ty::Ty::Str,
                },
            );
        }
    }
}
