//! `ActionText::Attachable` — the sgid round trip.
//!
//! Rails signs a GlobalID into every `<action-text-attachment>` node and
//! reads it back to reach the record: `@user.attachable_sgid` mints one,
//! `Content#attachables` dereferences them. This runtime had NEITHER
//! half, and `Content#attachables` said so by answering `[]` — a
//! divergence stated at its definition and in
//! docs/pipeline/runtime.md, whose cost was that campfire's
//! `Message#mentionees` saw no mentions on any message.
//!
//! Both halves are here now, and neither needs reflection:
//!
//! * the MINT is a per-model method this pass synthesizes, with the
//!   model name baked in at compile time — the same reason
//!   `lower::signed_id` combines its purpose at the call site rather
//!   than reading `self.class.name`;
//! * the DEREFERENCE never needs a name-to-class map, because the only
//!   shape that consumes it names its class at the call site:
//!   `attachables.grep(User)`. [`super::attachables_grep`] rewrites
//!   that to a query keyed on the ids the sgids carry, so there is no
//!   GlobalID registry to populate and no `const_get`.
//!
//! WHO GETS THE MINT. Rails puts it on `ActionText::Attachable`, so a
//! model that does not mix that in should not answer — this resolves
//! the include transitively, because campfire declares it one level
//! down (`User` includes `Mentionable`, which includes
//! `ActionText::Attachable`).
//!
//! Ruby-family only, like `runtime/ruby/action_text.rb` itself: the
//! signing lives in `MessageVerifier`, whose PBKDF2/HMAC is in
//! `MessageDigest`, which only the ruby-family trees ship.

use std::collections::BTreeSet;

use crate::app::App;
use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, Model};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

const MARKER: &str = "ActionText::Attachable";

/// Models that mix in `ActionText::Attachable`, directly or through a
/// concern module (to a fixpoint — a concern may include another).
pub fn attachable_models(app: &App) -> BTreeSet<ClassId> {
    // Concern modules that carry the marker, transitively.
    let mut carriers: BTreeSet<Symbol> = BTreeSet::new();
    loop {
        let mut changed = false;
        for lc in &app.library_classes {
            if carriers.contains(&lc.name.0) {
                continue;
            }
            let carries = lc.includes.iter().any(|i| {
                i.0.as_str() == MARKER || carriers.contains(&i.0) || {
                    // A bare `include Mentionable` inside `class User`
                    // names `User::Mentionable`; match on the last
                    // segment, which is the only spelling the include
                    // site carries.
                    let last = i.0.as_str().rsplit("::").next().unwrap_or("");
                    carriers.iter().any(|c| c.as_str().rsplit("::").next() == Some(last))
                }
            });
            if carries {
                carriers.insert(lc.name.0.clone());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut out = BTreeSet::new();
    for m in &app.models {
        let hit = crate::analyze::model_includes(m).into_iter().any(|inc| {
            let s = inc.0.as_str();
            if s == MARKER {
                return true;
            }
            let qualified = format!("{}::{}", m.name.0.as_str(), s);
            carriers.iter().any(|c| c.as_str() == s || c.as_str() == qualified)
        });
        if hit {
            out.insert(m.name.clone());
        }
    }
    out
}

/// `def attachable_sgid; ActionText::SignedGlobalId.generate("User", @id); end`
///
/// The model NAME is a literal, not `self.class.name`: this is the
/// compile-time fact the pass holds, and baking it keeps the runtime
/// free of reflection (`lower::signed_id` states the same rule for the
/// purpose string it combines).
pub(crate) fn push_attachable_sgid(
    methods: &mut Vec<MethodDef>,
    model: &Model,
    attachable: &BTreeSet<ClassId>,
) {
    if !attachable.contains(&model.name) {
        return;
    }
    let name = Symbol::from("attachable_sgid");
    if methods.iter().any(|m| m.name == name && m.receiver == MethodReceiver::Instance) {
        return;
    }
    let mut model_lit = Expr::new(
        Span::synthetic(),
        ExprNode::Lit {
            value: Literal::Str { value: model.name.0.as_str().to_string() },
        },
    );
    model_lit.ty = Some(Ty::Str);
    let mut id_read = Expr::new(Span::synthetic(), ExprNode::Ivar { name: Symbol::from("id") });
    id_read.ty = Some(Ty::Int);
    let mut body = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Const {
                    path: vec![Symbol::from("ActionText"), Symbol::from("SignedGlobalId")],
                },
            )),
            method: Symbol::from("generate"),
            args: vec![model_lit, id_read],
            block: None,
            parenthesized: true,
        },
    );
    body.ty = Some(Ty::Str);
    methods.push(MethodDef {
        name,
        receiver: MethodReceiver::Instance,
        params: vec![],
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    });
}
