//! `Rooms::Open.count` — an STI SUBCLASS standing at a query root.
//!
//! Rails' single-table inheritance gives a subclass its own default
//! scope: `Rooms::Open.count` counts the rooms whose `type` column says
//! `"Rooms::Open"`, not every row in `rooms`.
//!
//! An STI subclass is not ingested as a Model (it declares no table of
//! its own), so every one of those call sites resolved by plain Ruby
//! inheritance to the BASE class's method and answered the whole table.
//! campfire has seven rooms across three subclasses, so
//! `Rooms::Open.count` answered 7 where Rails answers 2 — a silently
//! wrong number, and the reason `user_test`'s membership-grant assertion
//! could pass while nothing was scoped: it compares that expression
//! against itself.
//!
//! The rewrite is a call-site one, into vocabulary every target already
//! speaks:
//!
//! ```text
//! Rooms::Open.pluck(:id)  ->  Room.where(type: "Rooms::Open").pluck(:id)
//! Rooms::Open.all         ->  Room.where(type: "Rooms::Open")
//! ```
//!
//! Synthesizing per-subclass class methods was the alternative and is
//! worse twice over: the surface is the whole Relation API, and each
//! method would have to land on every target.
//!
//! **Two halves.** The RELATION surface filters; the CONSTRUCTORS
//! (`new`/`create!`/…) STAMP, folding `type: "Rooms::Open"` into the
//! attribute hash — without it `Rooms::Open.create!` wrote a row
//! belonging to no subclass, which the read half above then correctly
//! could not see. A hand-written class method on the subclass
//! (`Rooms::Direct.find_or_create_for`) is the subclass's own; and
//! `find` is left alone because the arel pass inlines `<Model>.find` to
//! a primary-key SELECT — re-rooting it would move that work onto the
//! Relation path for no scoping gain a primary key can use.

use std::collections::HashMap;

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};

/// The Relation entry points a subclass inherits and must scope. `all`
/// is handled separately (it IS the scope, so the call disappears).
const STI_RELATION_METHODS: &[&str] = &[
    "all", "count", "pluck", "ids", "where", "find_by", "find_by!", "first", "last", "exists?",
    "any?", "none?", "empty?", "order", "limit", "offset", "joins", "includes", "distinct",
    "group", "maximum", "minimum", "destroy_all", "delete_all", "update_all",
];

/// Construction on a subclass, which has to STAMP the type column
/// rather than filter on it. `Rooms::Open.create!(name: …)` wrote a row
/// with an empty `type`, so it belonged to no subclass at all — and the
/// read half above made that visible, because a correctly-scoped
/// `Rooms::Open.count` then cannot see the row it just created.
const STI_CONSTRUCTORS: &[&str] = &["new", "create", "create!", "build"];

/// Rails' inheritance column. Not configurable here — `inheritance_column`
/// is a per-model override no corpus app writes, and guessing at one
/// would filter on a column that isn't there.
const INHERITANCE_COLUMN: &str = "type";

pub fn apply_sti_scope_lowering(app: &mut App) {
    let bases = sti_bases(app);
    if bases.is_empty() {
        return;
    }
    super::for_each_hook_body(app, &mut |e| rewrite(e, &bases));
    for view in &mut app.views {
        rewrite(&mut view.body, &bases);
    }
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, &bases);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body, &bases);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body, &bases);
        }
    }
}

/// STI subclass → its base model. A subclass is a library class whose
/// `parent` chain reaches a Model, and that Model's table has to carry
/// the inheritance column — without it there is nothing to filter on
/// and the class is plain Ruby inheritance, not STI.
fn sti_bases(app: &App) -> HashMap<ClassId, ClassId> {
    let mut out = HashMap::new();
    let model_named = |id: &ClassId| app.models.iter().find(|m| &m.name == id);
    for lc in &app.library_classes {
        let mut cursor = lc.parent.clone();
        let mut hops = 0;
        while let Some(parent) = cursor {
            if hops > 8 {
                break;
            }
            if let Some(base) = model_named(&parent) {
                let has_type = app
                    .schema
                    .tables
                    .get(&base.table.0)
                    .is_some_and(|t| {
                        t.columns.iter().any(|c| c.name.as_str() == INHERITANCE_COLUMN)
                    });
                if has_type {
                    out.insert(lc.name.clone(), base.name.clone());
                }
                break;
            }
            cursor = app
                .library_classes
                .iter()
                .find(|other| other.name == parent)
                .and_then(|other| other.parent.clone());
            hops += 1;
        }
    }
    out
}

fn rewrite(expr: &mut Expr, bases: &HashMap<ClassId, ClassId>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, bases));
    let ExprNode::Send { recv: Some(r), method, .. } = &*expr.node else { return };
    let constructing = STI_CONSTRUCTORS.contains(&method.as_str());
    if !constructing && !STI_RELATION_METHODS.contains(&method.as_str()) {
        return;
    }
    let ExprNode::Const { path } = &*r.node else { return };
    let named = ClassId(Symbol::from(
        path.iter()
            .map(|s| s.as_str().trim_start_matches("::"))
            .collect::<Vec<_>>()
            .join("::"),
    ));
    let Some(_) = bases.get(&named) else { return };
    if constructing {
        stamp_type(expr, &named);
        return;
    }
    let base = bases.get(&named).expect("checked above");
    let span = expr.span;
    let scoped = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const {
                    path: base.0.as_str().split("::").map(Symbol::from).collect(),
                },
            )),
            method: Symbol::from("where"),
            args: vec![Expr::new(
                span,
                ExprNode::Hash {
                    entries: vec![(
                        Expr::new(
                            span,
                            ExprNode::Lit {
                                value: Literal::Sym { value: Symbol::from(INHERITANCE_COLUMN) },
                            },
                        ),
                        Expr::new(
                            span,
                            ExprNode::Lit {
                                value: Literal::Str { value: named.0.as_str().to_string() },
                            },
                        ),
                    )],
                    kwargs: true,
                },
            )],
            block: None,
            parenthesized: true,
        },
    );
    // `.all` IS the scope, so the call disappears into it. Everything
    // else rides the scoped relation.
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { method, args, block, parenthesized, .. } = node else { unreachable!() };
    if method.as_str() == "all" && args.is_empty() && block.is_none() {
        *expr = scoped;
        return;
    }
    *expr = Expr::new(
        span,
        ExprNode::Send { recv: Some(scoped), method, args, block, parenthesized },
    );
}

/// Fold `type: "<Sub>"` into a constructor's attribute hash.
///
/// The RECEIVER stays the subclass: Ruby's inheritance constructs a
/// `Rooms::Open`, which is what campfire's `Room#open?` (`is_a?
/// (Rooms::Open)`) asks about — rerooting on the base would answer that
/// wrongly while fixing the column. Only the column is missing.
///
/// An existing `type:` wins; an argument that is not a literal Hash
/// (a params object, a variable) is left alone, because there is no
/// literal to fold into and building a `merge` here would hand the
/// constructor a shape it does not take.
fn stamp_type(expr: &mut Expr, sub: &ClassId) {
    let span = expr.span;
    let ExprNode::Send { args, .. } = &mut *expr.node else { return };
    let Some(attrs) = args.last_mut() else { return };
    let ExprNode::Hash { entries, .. } = &mut *attrs.node else { return };
    let already = entries.iter().any(|(k, _)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
            if value.as_str() == INHERITANCE_COLUMN)
    });
    if already {
        return;
    }
    entries.push((
        Expr::new(
            span,
            ExprNode::Lit { value: Literal::Sym { value: Symbol::from(INHERITANCE_COLUMN) } },
        ),
        Expr::new(
            span,
            ExprNode::Lit { value: Literal::Str { value: sub.0.as_str().to_string() } },
        ),
    ));
}
