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
        name_span: crate::span::Span::synthetic(),
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

/// Each attachable model paired with the partial its
/// `to_attachable_partial_path` names — `(User, "users/mention")` for
/// campfire — for `project::apply_content_layout` to write
/// `Content.render_attachment`'s arms from. In model order.
///
/// The method is looked for on the model itself and then on every
/// library class the model includes, transitively (campfire defines it
/// in `User::Mentionable`, the same concern that carries the marker),
/// and only a body that is one String literal counts: that is the
/// shape Rails' `to_attachable_partial_path` contract expects (a path,
/// fixed per class), and the only one a generator can spell as a
/// constant. A model with the marker and no such method falls back,
/// as Rails does, to `to_partial_path` — `users/user` — which is not a
/// partial an app has for an attachment, so it gets no arm and its
/// node keeps an empty inner.
pub fn attachable_partials(app: &App) -> Vec<(ClassId, String)> {
    let models = attachable_models(app);
    let mut out = Vec::new();
    for m in &app.models {
        if !models.contains(&m.name) {
            continue;
        }
        if let Some(path) = find_on_model_or_concerns(app, m, partial_path_literal) {
            out.push((m.name.clone(), path));
        }
    }
    out
}

/// The first `pick` over the model's own methods, then over every
/// library class the model includes, transitively — the include chain
/// walked breadth first, matching on the include's last segment as
/// `attachable_models` does for the marker (a bare `include
/// Mentionable` inside `class User` names `User::Mentionable`).
fn find_on_model_or_concerns<T>(
    app: &App,
    m: &Model,
    pick: impl Fn(&MethodDef) -> Option<T>,
) -> Option<T> {
    if let Some(found) = m.methods().find_map(&pick) {
        return Some(found);
    }
    let mut queue: Vec<Symbol> =
        crate::analyze::model_includes(m).into_iter().map(|c| c.0).collect();
    let mut seen: BTreeSet<Symbol> = BTreeSet::new();
    while let Some(inc) = queue.pop() {
        if !seen.insert(inc.clone()) {
            continue;
        }
        let last = inc.as_str().rsplit("::").next().unwrap_or("");
        for lc in &app.library_classes {
            let lc_last = lc.name.0.as_str().rsplit("::").next().unwrap_or("");
            if lc.name.0 != inc && lc_last != last {
                continue;
            }
            if let Some(found) = lc.methods.iter().find_map(&pick) {
                return Some(found);
            }
            queue.extend(lc.includes.iter().map(|i| i.0.clone()));
        }
    }
    None
}

/// Whether `m` is the instance method Rails' `Attachment#to_plain_text`
/// asks an attachable for.
fn plain_text_representation(m: &MethodDef) -> Option<()> {
    (m.name.as_str() == "attachable_plain_text_representation"
        && m.receiver == MethodReceiver::Instance)
        .then_some(())
}

fn partial_path_literal(m: &MethodDef) -> Option<String> {
    if m.name.as_str() != "to_attachable_partial_path" || m.receiver != MethodReceiver::Instance {
        return None;
    }
    match &*m.body.node {
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        _ => None,
    }
}

/// One class that names the partial Action Text renders for its
/// attachment nodes, and how the framework hands it over. The
/// call site is `ActionText::ContentHelper#render_action_text_attachment`:
/// `render(partial: attachment.to_attachable_partial_path, object:
/// attachment, as: attachment.model_name.element)` — so the partial's
/// LOCAL is the class's `model_name.element` (`user` for `User`,
/// `opengraph_embed` for `ActionText::Attachment::OpengraphEmbed`), not
/// its directory's singular, and what it holds is the Attachment
/// delegating to the record. This is the fact the view lowering, the
/// analyzer and the render seam each need one piece of.
#[derive(Debug, Clone)]
pub struct AttachablePartial {
    /// The class: a model that mixes `ActionText::Attachable` in, or a
    /// library class that answers `to_partial_path` and is dispatched
    /// by content type.
    pub class: ClassId,
    /// The partial as the class spells it: `users/mention`.
    pub partial: String,
    /// `model_name.element`: the last segment of the class name, snake
    /// cased.
    pub local: String,
    /// `attachable_content_type`'s literal, for a class the node names
    /// by content type rather than by sgid (campfire's
    /// `OpengraphEmbed`, `application/vnd.actiontext.opengraph-embed`);
    /// such a class builds itself from the node through its own
    /// `from_node`.
    pub content_type: Option<String>,
    /// Whether the class (or, for a model, a concern it includes)
    /// defines `attachable_plain_text_representation(caption)` — the
    /// hook `Attachment#to_plain_text` prefers to the caption. campfire's
    /// `User::Mentionable` answers `"@#{name}"` and its `OpengraphEmbed`
    /// `""`; `Content.attachment_plain_text` gets an arm for each.
    pub plain_text: bool,
}

/// Every class in the app that names an attachment partial by literal —
/// `to_attachable_partial_path` first, `to_partial_path` otherwise, the
/// order Rails' `render_action_text_attachment` resolves them in.
/// Models come through the include chain (`attachable_partials`);
/// library classes are read directly. Deterministic: models in app
/// order, then library classes in app order.
pub fn attachable_partial_bindings(app: &App) -> Vec<AttachablePartial> {
    let mut out = Vec::new();
    for (class, partial) in attachable_partials(app) {
        let plain_text = app
            .models
            .iter()
            .find(|m| m.name == class)
            .is_some_and(|m| find_on_model_or_concerns(app, m, plain_text_representation).is_some());
        out.push(AttachablePartial {
            local: element_of(&class),
            class,
            partial,
            content_type: None,
            plain_text,
        });
    }
    for lc in &app.library_classes {
        if lc.is_module || out.iter().any(|b| b.class == lc.name) {
            continue;
        }
        let partial = lc
            .methods
            .iter()
            .find_map(partial_path_literal)
            .or_else(|| lc.methods.iter().find_map(|m| literal_of(m, "to_partial_path")));
        let Some(partial) = partial else { continue };
        let content_type = lc
            .methods
            .iter()
            .find_map(|m| literal_or_constant_of(m, "attachable_content_type", &lc.constants));
        // Only a class the framework could hand a node to: one the
        // sgid names (a model, handled above) or one that builds
        // itself from the node by content type. A library class with
        // a `to_partial_path` and neither is some other partial owner.
        if content_type.is_none() {
            continue;
        }
        let has_from_node = lc
            .methods
            .iter()
            .any(|m| m.name.as_str() == "from_node" && m.receiver == MethodReceiver::Class);
        if !has_from_node {
            continue;
        }
        out.push(AttachablePartial {
            local: element_of(&lc.name),
            class: lc.name.clone(),
            partial,
            content_type,
            plain_text: lc.methods.iter().any(|m| plain_text_representation(m).is_some()),
        });
    }
    out
}

/// `model_name.element` for a class: the last segment, snake cased —
/// `ActionText::Attachment::OpengraphEmbed` → `opengraph_embed`.
fn element_of(class: &ClassId) -> String {
    let last = class.0.as_str().rsplit("::").next().unwrap_or("");
    crate::naming::safe_local(&crate::naming::snake_case(last))
}

fn literal_of(m: &MethodDef, name: &str) -> Option<String> {
    literal_or_constant_of(m, name, &[])
}

/// A body that is one String literal, or one bare constant the class
/// declares as one — campfire's `attachable_content_type` answers
/// `OPENGRAPH_EMBED_CONTENT_TYPE`, declared two lines up.
fn literal_or_constant_of(m: &MethodDef, name: &str, constants: &[(Symbol, Expr)]) -> Option<String> {
    if m.name.as_str() != name || m.receiver != MethodReceiver::Instance {
        return None;
    }
    match &*m.body.node {
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        ExprNode::Const { path } if path.len() == 1 => constants.iter().find_map(|(n, e)| {
            if *n != path[0] {
                return None;
            }
            match &*e.node {
                ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                _ => None,
            }
        }),
        _ => None,
    }
}

/// The view name (`action_text/attachables/_opengraph_embed`) a
/// partial path maps to.
pub fn partial_view_name(partial: &str) -> String {
    match partial.rsplit_once('/') {
        Some((dir, stem)) => format!("{dir}/_{stem}"),
        None => format!("_{partial}"),
    }
}

/// `attachment=` and `caption` on a content-type attachable — the
/// delegation Rails' Attachment does for the record, mirrored the other
/// way. Rails renders the partial with the ATTACHMENT as its local
/// (`delegate_missing_to :attachable`), so `opengraph_embed.caption`
/// there is `Attachment#caption`, the node's caption, on a class that
/// never defines it. The emitted partial takes the RECORD — a typed
/// local — so the record carries the node it was built from in a slot
/// the render seam sets (`embed.attachment = attachment`) and answers
/// the Attachment's readers through it. Only what campfire's partial
/// reads (`caption`); a class that defines its own keeps it.
///
/// Types are stamped rather than inferred: this runs after analysis,
/// and a node without a type is an error on the strict emit.
pub(crate) fn push_attachment_delegation(methods: &mut Vec<MethodDef>, class: &ClassId) {
    let attachment_ty = Ty::Class { id: ClassId(Symbol::from("ActionText::Attachment")), args: vec![] };
    let slot_ty = Ty::Union { variants: vec![attachment_ty.clone(), Ty::Nil] };
    let caption_ty = Ty::Union { variants: vec![Ty::Str, Ty::Nil] };
    let has_writer =
        methods.iter().any(|m| m.name.as_str() == "attachment=" && m.receiver == MethodReceiver::Instance);
    let has_caption =
        methods.iter().any(|m| m.name.as_str() == "caption" && m.receiver == MethodReceiver::Instance);
    let sp = Span::synthetic();
    let typed = |node: ExprNode, ty: Ty| {
        let mut e = Expr::new(sp, node);
        e.ty = Some(ty);
        e
    };
    if !has_writer {
        // def attachment=(value); @attachment = value; end
        let value = typed(
            ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("value") },
            attachment_ty.clone(),
        );
        let body = typed(
            ExprNode::Assign {
                target: crate::expr::LValue::Ivar { name: Symbol::from("attachment") },
                value,
            },
            attachment_ty.clone(),
        );
        methods.push(MethodDef {
            name_span: sp,
            name: Symbol::from("attachment="),
            receiver: MethodReceiver::Instance,
            params: vec![crate::dialect::Param {
                name: Symbol::from("value"),
                default: None,
                keyword: false,
                rest: false,
                from_keyword: false,
                from_kwrest: false,
            }],
            body,
            signature: None,
            effects: EffectSet::default(),
            enclosing_class: Some(class.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: true,
            block_param: None,
        });
    }
    if !has_caption {
        // def caption; a = @attachment; a.nil? ? nil : a.caption; end
        let read = typed(ExprNode::Ivar { name: Symbol::from("attachment") }, slot_ty.clone());
        let a_var = |ty: Ty| typed(ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("a") }, ty);
        let assign = typed(
            ExprNode::Assign { target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: Symbol::from("a") }, value: read },
            slot_ty.clone(),
        );
        let is_nil = typed(
            ExprNode::Send { recv: Some(a_var(slot_ty.clone())), method: Symbol::from("nil?"), args: vec![], block: None, parenthesized: false },
            Ty::Bool,
        );
        let call = typed(
            ExprNode::Send { recv: Some(a_var(attachment_ty.clone())), method: Symbol::from("caption"), args: vec![], block: None, parenthesized: false },
            caption_ty.clone(),
        );
        let nil = typed(ExprNode::Lit { value: Literal::Nil }, Ty::Nil);
        let branch = typed(ExprNode::If { cond: is_nil, then_branch: nil, else_branch: call }, caption_ty.clone());
        let body = typed(ExprNode::Seq { exprs: vec![assign, branch] }, caption_ty.clone());
        methods.push(MethodDef {
            name_span: sp,
            name: Symbol::from("caption"),
            receiver: MethodReceiver::Instance,
            params: vec![],
            body,
            signature: None,
            effects: EffectSet::default(),
            enclosing_class: Some(class.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        });
    }
}
