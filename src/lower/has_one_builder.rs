//! `owner.create_<assoc>!(k: v)` → `Target.create!(fk: owner.id, k: v)`,
//! for a `has_one :assoc`, and the `create_`/`build_` forms beside it.
//!
//! Rails generates four methods per `has_one` — `build_x`, `create_x`,
//! `create_x!`, `reload_x` — on top of the reader and writer that
//! `model_to_library::associations` already synthesizes. campfire calls
//! exactly one of them, `user.create_webhook!(url: …)` in
//! `User::Bot.create_bot!`, and it was one of the ten strict-emit
//! errors: the emitted tree CALLED it and nothing defined it, so typing
//! it without lowering it would have been a method that types and then
//! raises.
//!
//! A CALL-SITE REWRITE, not a synthesized method, for the reason
//! `destroy_by` states next door: a method defined on `Base` is a method
//! on every model in every target, and the attributes hash is a
//! different shape at every call site, so each strict target would have
//! to give that parameter a type. The kwargs stay at the call site,
//! where they are literal, and become `create!`'s own hash — which is
//! exactly the shape the AR catalog already types. It also sidesteps
//! kwarg FORWARDING, which the strict targets do not support: nothing is
//! forwarded, the arguments are inlined into the constructor call.
//!
//! BOTH RECEIVER FORMS. campfire writes each once:
//! `user.create_webhook!(url: …)` in `User::Bot.create_bot!`, and a bare
//! `create_webhook!(url: url)` in `User::Bot#update_webhook_url!`. The
//! explicit form resolves its owner from the receiver's stamped type;
//! the implicit one cannot, so it is resolved structurally — a model's
//! own body owns itself, and a CONCERN's body is owned by the model that
//! includes it, but only when exactly one model does. With two includers
//! the same bare name could mean two different targets, and guessing
//! would emit a row against the wrong table; that stays a gap.
//!
//! Scope is `has_one` only. `belongs_to`'s builders write the foreign key
//! back onto the RECEIVER (`user.create_account!` sets `user.account_id`)
//! rather than onto the new record, which is a different rewrite with an
//! assignment in it; it lands when a fixture demands it, the way HABTM
//! does in `associations`. `reload_<assoc>` is likewise absent — the
//! reader re-queries on every call here, so a reload has nothing to
//! invalidate, and inventing it would imply a cache that does not exist.

use crate::app::App;
use crate::dialect::Association;
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;
use std::collections::HashMap;

/// `has_one` declarations by owner: assoc name → (target, foreign key).
type Builders = HashMap<ClassId, HashMap<Symbol, (ClassId, Symbol)>>;

pub fn apply_has_one_builder_lowering(app: &mut App) {
    let mut table: Builders = HashMap::new();
    for model in &app.models {
        for assoc in model.associations() {
            // Polymorphic `has_one ..., as:` needs the interface TYPE
            // column set too. Left out rather than half-done: writing
            // only the id column would build a row no reader can find.
            if let Association::HasOne { name, target, foreign_key, as_interface: None, .. } =
                assoc
            {
                table
                    .entry(model.name.clone())
                    .or_default()
                    .insert(name.clone(), (target.clone(), foreign_key.clone()));
            }
        }
    }
    if table.is_empty() {
        return;
    }

    // Concern module → the single model that includes it, when there is
    // exactly one. `None` for a module two models share: see the header.
    let mut sole_includer: HashMap<ClassId, Option<ClassId>> = HashMap::new();
    for model in &app.models {
        for m in crate::analyze::model_includes(model) {
            sole_includer
                .entry(m)
                .and_modify(|e| *e = None)
                .or_insert_with(|| Some(model.name.clone()));
        }
    }

    // A model's own body: implicit self is the model.
    for i in 0..app.models.len() {
        let owner = app.models[i].name.clone();
        let mut body: Vec<_> = std::mem::take(&mut app.models[i].body);
        for item in &mut body {
            for e in model_item_exprs(item) {
                rewrite(e, &table, Some(&owner));
            }
        }
        app.models[i].body = body;
    }

    // A concern's body: implicit self is its sole includer, if any.
    for i in 0..app.library_classes.len() {
        let owner = sole_includer
            .get(&app.library_classes[i].name)
            .and_then(|o| o.clone());
        for method in &mut app.library_classes[i].methods {
            rewrite(&mut method.body, &table, owner.as_ref());
        }
    }

    // Everything else — controllers, views, callbacks: an implicit self
    // there is not a model, so only the explicit-receiver form applies.
    super::for_each_hook_body(app, &mut |e| rewrite(e, &table, None));
    for view in &mut app.views {
        rewrite(&mut view.body, &table, None);
    }
}

/// The expressions a model body item carries, for the owner-aware walk.
fn model_item_exprs(item: &mut crate::dialect::ModelBodyItem) -> Vec<&mut Expr> {
    use crate::dialect::ModelBodyItem;
    match item {
        ModelBodyItem::Method { method, .. } => vec![&mut method.body],
        ModelBodyItem::Scope { scope, .. } => vec![&mut scope.body],
        ModelBodyItem::Unknown { expr, .. } => vec![expr],
        _ => vec![],
    }
}

fn rewrite(expr: &mut Expr, table: &Builders, self_owner: Option<&ClassId>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, table, self_owner));

    let ExprNode::Send { recv, method, args, block: None, .. } = &*expr.node else {
        return;
    };
    // The owner is the receiver's model type where there is a receiver —
    // `Ty::Class` is how the analyzer spells an instance — and the
    // enclosing model where the call is on implicit self.
    let owner = match recv {
        Some(r) => match r.ty.as_ref() {
            Some(Ty::Class { id, .. }) => id,
            _ => return,
        },
        None => match self_owner {
            Some(o) => o,
            None => return,
        },
    };
    let Some(by_name) = table.get(owner) else { return };

    // `create_x!` before `create_x`: the bang name also starts with the
    // non-bang prefix, so testing the shorter one first would strip the
    // `!` and build the wrong constructor.
    let name = method.as_str();
    let (assoc, ctor) = if let Some(rest) = name.strip_prefix("create_").and_then(|r| r.strip_suffix('!')) {
        (rest, "create!")
    } else if let Some(rest) = name.strip_prefix("create_") {
        (rest, "create")
    } else if let Some(rest) = name.strip_prefix("build_") {
        (rest, "new")
    } else {
        return;
    };
    let Some((target, foreign_key)) = by_name.get(&Symbol::from(assoc)) else { return };

    // Rails allows the no-argument form (`user.create_webhook!`); the
    // foreign key alone is still a complete row to attempt.
    let mut entries = vec![(
        crate::lower::typing::lit_sym(foreign_key.clone()),
        owner_id(recv.as_ref()),
    )];
    match args.len() {
        0 => {}
        1 => {
            let ExprNode::Hash { kwargs: true, entries: given } = &*args[0].node else {
                return;
            };
            entries.extend(given.iter().cloned());
        }
        _ => return,
    }

    let span = expr.span;
    let mut attrs = Expr::new(span, ExprNode::Hash { entries, kwargs: true });
    attrs.ty = Some(Ty::Untyped);
    *expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(class_const(target)),
            method: Symbol::from(ctor),
            args: vec![attrs],
            block: None,
            parenthesized: true,
        },
    );
    // The synthesized hop is never seen by analyze, so its type is
    // written here — same reason `destroy_by` stamps its `where`. All
    // three constructors answer an instance of the target; `create`
    // (non-bang) answers an unsaved-but-present record when validation
    // fails, never nil, so there is no Nil arm on any of them.
    expr.ty = Some(Ty::Class { id: target.clone(), args: vec![] });
}

/// The owner's primary key. With an explicit receiver that is
/// `<recv>.id`, read at the call site; on implicit self it is `@id`,
/// which is what the synthesized readers next door use — they are cut
/// INTO the model, and so is the body this call sits in.
fn owner_id(recv: Option<&Expr>) -> Expr {
    let mut e = match recv {
        Some(r) => Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(r.clone()),
                method: Symbol::from("id"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        ),
        None => Expr::new(Span::synthetic(), ExprNode::Ivar { name: Symbol::from("id") }),
    };
    e.ty = Some(Ty::Int);
    e
}

fn class_const(id: &ClassId) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: id.0.as_str().split("::").map(Symbol::from).collect() },
    )
}
