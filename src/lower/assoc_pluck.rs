//! `pluck` on rows the pipeline already materialized.
//!
//! `pluck(:col)` is a Relation terminal: Rails turns it into
//! `SELECT col FROM …` and answers an Array of that column's values.
//! Two roots reach that treatment here — a model CONST
//! (`Rooms::Open.pluck(:id)`, seeded onto a Relation by
//! `lower::scope_chain`) and a Const-rooted chain
//! (`Room.where(…).pluck(:id)`, folded to SQL by `lower::arel`).
//!
//! AN ASSOCIATION READER IS NEITHER, and campfire has one:
//!
//! ```ruby
//! room.memberships.pluck(:user_id).each { |id| ... }   # Message::Broadcasts
//! ```
//!
//! `Room#memberships` is a lowered reader that answers a hydrated
//! `Array[Membership]`, and `Array` has no `pluck` — `undefined method
//! 'pluck' for an instance of Array`, a 500 on every `POST
//! /rooms/:id/messages` in a served app. (The broadcast is not lost: the
//! `after_create_commit` that fans the turbo-stream out runs before this
//! line, so the message reaches subscribers and the request that made it
//! then fails. The suite never sees it — under the test adapter the job
//! that reaches this line is enqueued rather than run.)
//!
//! So: expand it into the projection it is.
//!
//! ```ruby
//! room.memberships.map { |__pluck| __pluck.user_id }
//! ```
//!
//! WHAT THIS IS NOT. It is not the SQL that Rails writes. Lifting an
//! association-rooted chain into a `SELECT user_id FROM memberships
//! WHERE room_id = ?` is the association-proxy work that
//! `project_relation_specialization_design_call` already scopes, and
//! this pass is deliberately not a down payment on it — a half-built
//! chain root that handles `pluck` and nothing else is worse than none,
//! because the next terminal to arrive looks supported until it is not.
//!
//! WHAT IT COSTS is therefore honest and small: the reader hydrates
//! every row of the association, which is what it ALREADY does today —
//! the projection is the only new work. Rails reads one column; we read
//! the row and then take the column. Ledgered.
//!
//! THE GUARD IS THE ASSOCIATION KIND, and it took two wrong answers to
//! get there. A plain `has_many` reader hydrates and answers an Array;
//! a `has_many :through` answers an `ActiveRecord::Relation` over a
//! joins chain, which HAS `pluck` and whose `pluck` is a single-column
//! SELECT. Both are spelled `owner.name`.
//!
//! * NOT the chain's innermost receiver. `Current.user.rooms.directs`
//!   bottoms out at a `Const` and is nothing to do with arel.
//! * NOT `Ty::Array`, which looks exactly like the discriminator and is
//!   not: the analyzer types a `:through` reader `Array[Room]` as well,
//!   because the type is its approximation of "a collection" and
//!   conflates precisely the two shapes this pass must separate.
//!   Gating on it rewrote a Relation's SELECT into a whole-row
//!   hydrate — the opposite of the fix, and a test caught it.
//!
//! So: rewrite when the reader's name is declared by a plain
//! `has_many`/`has_one` on EVERY model that declares one. A name that
//! is plain on one model and `:through` on another is left alone rather
//! than guessed at. A stated `Relation` or model `Class` on the
//! receiver short-circuits first, and a `Const` root stays a backstop.
//!
//! It leaves the cases where the receiver is a SCOPE on an association
//! (`room.users.without(x).pluck(:name)`) exactly as they are. Those
//! need to know what the scope answers, which is the association-proxy
//! question again.
//!
//! ONLY `pluck`. `ids` has the same shape and no failing call site;
//! adding it would be a second untested arm, and the pass that grows one
//! for a case nothing exercises is the pass that acquires its next bug.

use crate::app::App;
use crate::expr::{BlockStyle, Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub fn apply_assoc_pluck_lowering(app: &mut App) {
    let materialized = materialized_assoc_names(app);
    super::for_each_hook_body(app, &mut |e| rewrite(e, &materialized));
}

/// Association names whose reader answers a MATERIALIZED Array on every
/// model that declares one.
///
/// A plain `has_many` / `has_one` lowers to a reader that runs the
/// query and hydrates; a `has_many :through` (and HABTM) lowers to an
/// `ActiveRecord::Relation` over a joins chain. The name is the only
/// handle available when the receiver has no type, so a name that is
/// plain on one model and `:through` on another is EXCLUDED rather than
/// guessed at — being wrong in the Relation direction costs a
/// single-column SELECT, and this pass is not worth that.
fn materialized_assoc_names(app: &App) -> std::collections::HashSet<Symbol> {
    use crate::lower::model_associations::AssocKind;
    let mut plain = std::collections::HashSet::new();
    let mut relational = std::collections::HashSet::new();
    for edge in crate::lower::model_associations::compute_association_graph(app) {
        // `through` is carried as a FIELD as well as a kind, and the
        // graph does not always spell the kind `HasManyThrough` — a
        // `has_many :rooms, through: :memberships` arrives as `HasMany`
        // with `through: Some(:memberships)`. Reading the kind alone
        // put it in the plain set and rewrote a Relation's `pluck`.
        if edge.through.is_some() {
            relational.insert(edge.name.clone());
            continue;
        }
        match edge.kind {
            AssocKind::HasMany | AssocKind::HasOne => {
                plain.insert(edge.name.clone());
            }
            AssocKind::HasManyThrough | AssocKind::HasAndBelongsToMany => {
                relational.insert(edge.name.clone());
            }
            AssocKind::BelongsTo => {}
        }
    }
    plain.retain(|n| !relational.contains(n));
    plain
}

fn rewrite(expr: &mut Expr, materialized: &std::collections::HashSet<Symbol>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, materialized));

    let ExprNode::Send { recv: Some(recv), method, args, block, .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "pluck" || block.is_some() || args.is_empty() {
        return;
    }
    // Symbol arguments only. `pluck(:"table.col")` and the string form
    // name a column through SQL rather than through a reader, and
    // nothing answers those on a materialized row.
    let mut cols: Vec<Symbol> = Vec::new();
    for a in args {
        let ExprNode::Lit { value: Literal::Sym { value } } = &*a.node else { return };
        if value.as_str().contains('.') {
            return;
        }
        cols.push(value.clone());
    }
    // THE RECEIVER'S TYPE IS THE DISCRIMINATOR, and it has to be,
    // because the two association shapes are not syntactically
    // different. A plain `has_many` reader hydrates and answers an
    // Array (`Room#memberships`); a `has_many :through` answers an
    // `ActiveRecord::Relation` (`User#rooms`, a joins chain). Both are
    // spelled `owner.name`. A Relation ALREADY has `pluck`, and its
    // `pluck` is a single-column SELECT — rewriting that one would
    // replace a projection with a whole-row hydrate.
    //
    // UNKNOWN TYPES ARE REWRITTEN, and that is safe rather than
    // optimistic: `map` is Enumerable, so it answers on an Array and on
    // a Relation both. The worst an unknown costs is a Relation losing
    // its single-column SELECT — where the alternative, leaving it, is
    // `undefined method 'pluck' for an instance of Array` on every
    // Array-valued one. campfire's `room.memberships` is exactly this
    // case: the site lives in a concern, where `room` has no type.
    // `Ty::Array` LOOKS like the discriminator and is not. The analyzer
    // types a `has_many :through` reader `Array[Room]` too, where the
    // emitted reader is an `ActiveRecord::Relation` over a joins chain —
    // the type is its approximation of "a collection", and it conflates
    // exactly the two shapes this pass has to tell apart. Gating on it
    // rewrote a Relation's single-column SELECT into a whole-row
    // hydrate, which is the opposite of the fix.
    //
    // So the ASSOCIATION KIND decides, because that is what actually
    // determines which reader gets emitted. A stated Relation or model
    // Class still short-circuits — those are certain, and cheap to
    // check.
    if matches!(
        &recv.ty,
        Some(crate::ty::Ty::Relation { .. }) | Some(crate::ty::Ty::Class { .. })
    ) {
        return;
    }
    let ExprNode::Send { method: reader, .. } = &*recv.node else { return };
    if !materialized.contains(reader) {
        return;
    }
    // A Const root is `Model.where(…).pluck(:id)`, arel's inlined
    // SELECT. Belt and braces: a model constant does not answer an
    // association name, so this should not trigger — and if it ever
    // does, the SELECT wins.
    if const_rooted(recv) {
        return;
    }

    let span = expr.span;
    let var = Symbol::from("__pluck");
    let read = |col: &Symbol| {
        Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(
                    span,
                    ExprNode::Var { id: crate::ident::VarId(0), name: var.clone() },
                )),
                method: col.clone(),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        )
    };
    // Rails answers an Array of VALUES for one column and an Array of
    // Arrays for several, and a caller that indexes the result is
    // reading one or the other. The single-column form is the whole of
    // the corpus; the multi-column arm is here because getting it wrong
    // would be silent.
    let body = if cols.len() == 1 {
        read(&cols[0])
    } else {
        Expr::new(
            span,
            ExprNode::Array {
                elements: cols.iter().map(read).collect(),
                style: Default::default(),
            },
        )
    };

    let block = Expr::new(
        span,
        ExprNode::Lambda {
            params: vec![var],
            rest_param: None,
            block_param: None,
            body,
            block_style: BlockStyle::Brace,
        },
    );
    let recv = recv.clone();
    *expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("map"),
            args: vec![],
            block: Some(block),
            parenthesized: false,
        },
    );
}

/// Does this chain bottom out at a model constant?
///
/// `Room.where(x).order(y)` does; `room.memberships` does not. Walks
/// only the RECEIVER spine — an argument that happens to name a class
/// is not what the chain is rooted on.
fn const_rooted(expr: &Expr) -> bool {
    match &*expr.node {
        ExprNode::Const { .. } => true,
        ExprNode::Send { recv: Some(r), .. } => const_rooted(r),
        _ => false,
    }
}
