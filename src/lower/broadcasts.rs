//! Target-neutral lowering of Turbo Streams broadcast declarations.
//!
//! Walks a model's body looking for two shapes:
//!
//!   1. `broadcasts_to ->(record) { "stream" }, inserts_by: :prepend`
//!      — fires on every save (replace / prepend / append) and every
//!      destroy (remove). The lambda param is preserved as
//!      `self_param` so each emitter can rewrite it to its own
//!      `self` / `this` / `record` convention when rendering.
//!   2. `after_create_commit { assoc.broadcast_replace_to("stream") }`
//!      and `after_destroy_commit { ... }` — fires on a parent
//!      record found via a belongs_to association. The association
//!      is resolved here so emitters can render the guarded lookup
//!      (`<Target>::find(self.<fk>)` and friends) without
//!      re-walking the body.
//!
//! Expressions inside the declarations (channel name, target
//! override, broadcast args) stay as dialect-level `Expr` nodes —
//! rendering to target syntax is the emitter's job. The
//! `rescue nil` modifier on `after_*_commit` blocks (used by the
//! blog fixture during seeding) is peeled here so emitters see just
//! the underlying broadcast call.

use crate::dialect::{Association, Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::naming::pluralize_snake;

/// Lowered broadcast declarations for a single model, split by
/// persist hook. An empty result means the model has no broadcasts
/// and emitters should skip the Broadcaster impl + save/destroy
/// hook points entirely.
#[derive(Default, Debug)]
pub struct LoweredBroadcasts {
    pub save: Vec<LoweredBroadcast>,
    pub destroy: Vec<LoweredBroadcast>,
}

impl LoweredBroadcasts {
    pub fn is_empty(&self) -> bool {
        self.save.is_empty() && self.destroy.is_empty()
    }
}

/// One broadcast call to emit. `channel` and `target` are kept as
/// `Expr` nodes — emitters are responsible for rendering them,
/// including any self-param rewrite driven by `self_param`.
#[derive(Debug)]
pub struct LoweredBroadcast {
    pub action: BroadcastAction,
    pub channel: Expr,
    pub target: Option<Expr>,
    /// Name of the `broadcasts_to` lambda param, when present. The
    /// channel (and target) expressions may reference it via
    /// `param.field`; emitters rewrite bare occurrences to the
    /// target's `self` equivalent.
    pub self_param: Option<Symbol>,
    /// Set when the broadcast fires on a parent record via a
    /// belongs_to association (after_*_commit blocks). Emitters
    /// guard the call with `<Target>::find(self.<fk>)` so missing
    /// parents silently skip instead of panicking.
    pub on_association: Option<LoweredAssocRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BroadcastAction {
    Replace,
    Prepend,
    Append,
    Remove,
}

impl BroadcastAction {
    /// Snake-case suffix matching `broadcast_<action>_to` in Rails,
    /// railcar's runtime, and both TS/Rust cable helpers.
    pub fn as_snake(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::Prepend => "prepend",
            Self::Append => "append",
            Self::Remove => "remove",
        }
    }
}

#[derive(Clone, Debug)]
pub struct LoweredAssocRef {
    /// Association name as written in source (`article`).
    pub name: Symbol,
    /// Target model class (`Article`).
    pub target_class: Symbol,
    /// Target model's table name (`articles`). Pre-computed here so
    /// emitters don't each carry a pluralizer.
    pub target_table: String,
    /// Foreign-key column on the owning model (`article_id`).
    pub foreign_key: Symbol,
}

/// Lower all broadcast declarations on a model.
pub fn lower_broadcasts(model: &Model) -> LoweredBroadcasts {
    let mut out = LoweredBroadcasts::default();
    let belongs_tos = collect_belongs_to(model);

    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else {
            continue;
        };
        let ExprNode::Send {
            recv: None,
            method,
            args,
            block,
            ..
        } = &*expr.node
        else {
            continue;
        };
        match method.as_str() {
            "broadcasts_to" => collect_broadcasts_to(&mut out, args),
            // Both `after_create_commit` and `after_save_commit`
            // fire at persist time. We register them on the save
            // bucket; Rails' create-vs-update distinction isn't
            // worth extra codegen for blog-shaped apps (re-broadcasting
            // on update is idempotent).
            "after_create_commit" | "after_save_commit" => {
                if let Some(b) = block {
                    collect_commit_block(&mut out.save, b, &belongs_tos);
                }
            }
            "after_destroy_commit" => {
                if let Some(b) = block {
                    collect_commit_block(&mut out.destroy, b, &belongs_tos);
                }
            }
            _ => {}
        }
    }
    out
}

/// Does the tree this app emits reach the LIVE broadcast path — the one
/// that ends at a WebSocket transport?
///
/// `lower_broadcasts` answers a narrower question: does a model DECLARE
/// broadcasts with the `broadcasts_to` / `after_*_commit` macro family.
/// That was the whole story while the corpus was blog-shaped, so both
/// consumers of "does this app need /cable" asked it directly — and
/// campfire, which writes `broadcast_append_to` in an ordinary concern
/// METHOD (rewritten by `broadcast_calls`, a pass no macro walk visits),
/// shipped `Broadcasts.append` calls in a tree with no `/cable`
/// endpoint, no transport registered, and no websocket-driver gem. The
/// emitted app looked complete and was silently one-way: the POST wrote
/// the row, and nothing ever reached a second tab.
///
/// Asking the LOWERED BODIES keeps the two questions from drifting apart
/// again — whatever pass puts a `Broadcasts.<action>` send in a body,
/// this sees it. Bodies live in exactly the two homes
/// `apply_broadcast_calls_lowering` rewrites: a model's own methods and
/// a library class (the concern module emitted beside it).
///
/// `turbo_stream_fragment` is deliberately NOT counted. A
/// `.turbo_stream.erb` template composes markup for an HTTP response
/// body and never touches a transport, so an app with turbo_stream views
/// and no broadcasts still ships cable-free.
pub fn app_broadcasts_live(app: &crate::app::App) -> bool {
    if app.models.iter().any(|m| !lower_broadcasts(m).is_empty()) {
        return true;
    }
    let model_bodies = app.models.iter().flat_map(|m| {
        m.body.iter().filter_map(|item| match item {
            ModelBodyItem::Method { method, .. } => Some(&method.body),
            _ => None,
        })
    });
    let library_bodies = app
        .library_classes
        .iter()
        .flat_map(|lc| lc.methods.iter().map(|m| &m.body));
    model_bodies.chain(library_bodies).any(is_live_broadcast_call)
}

/// True when `expr` — or anything under it — is a send to the
/// `Broadcasts` module naming one of the four live actions.
fn is_live_broadcast_call(expr: &Expr) -> bool {
    if let ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node {
        let names_broadcasts = matches!(
            &*recv.node,
            ExprNode::Const { path } if path.last().is_some_and(|s| {
                // A rooted reference (`::Broadcasts`, which
                // `apply_view_constant_rooting` may have stamped) names
                // the same module.
                matches!(s.as_str(), "Broadcasts" | "::Broadcasts")
            })
        );
        if names_broadcasts
            && matches!(method.as_str(), "append" | "prepend" | "replace" | "remove")
        {
            return true;
        }
    }
    let mut found = false;
    expr.node.for_each_child(&mut |child| {
        found = found || is_live_broadcast_call(child);
    });
    found
}

/// Pick belongs_to associations off the model body so commit blocks
/// can resolve a bare-method receiver (`article.broadcast_…`) to
/// its target class and foreign key.
fn collect_belongs_to(model: &Model) -> Vec<LoweredAssocRef> {
    let mut out = Vec::new();
    for item in &model.body {
        let ModelBodyItem::Association { assoc, .. } = item else {
            continue;
        };
        if let Association::BelongsTo {
            name,
            target,
            foreign_key,
            ..
        } = assoc
        {
            let target_class = target.0.clone();
            let target_table = pluralize_snake(target_class.as_str());
            out.push(LoweredAssocRef {
                name: name.clone(),
                target_class,
                target_table,
                foreign_key: foreign_key.clone(),
            });
        }
    }
    out
}

fn collect_broadcasts_to(out: &mut LoweredBroadcasts, args: &[Expr]) {
    let Some(stream_arg) = args.first() else {
        return;
    };
    // Accept both lambda and bare-string forms for the channel.
    let (channel, self_param) = match &*stream_arg.node {
        ExprNode::Lambda { body, params, .. } => {
            (body.clone(), params.first().cloned())
        }
        ExprNode::Lit { value: Literal::Str { .. } } => (stream_arg.clone(), None),
        _ => return,
    };

    // Options hash: `inserts_by:` controls the save-time action;
    // `target:` overrides the default DOM target string. Anything
    // else is quietly ignored — future options land as needed.
    //
    // `inserts_by` defaults to `:append` — matches Rails' turbo-rails
    // `broadcasts_to(stream, inserts_by: :append, …)` signature.
    // Explicit `:replace` stays replace; anything unrecognized falls
    // back to append so unknown-option typos don't silently change
    // semantics in a visible way.
    let mut action = BroadcastAction::Append;
    let mut target: Option<Expr> = None;
    if let Some(opts) = args.get(1) {
        if let ExprNode::Hash { entries, .. } = &*opts.node {
            for (k, v) in entries {
                let Some(key) = hash_sym_key(k) else { continue };
                match key.as_str() {
                    "inserts_by" => {
                        if let ExprNode::Lit {
                            value: Literal::Sym { value },
                        } = &*v.node
                        {
                            action = match value.as_str() {
                                "prepend" => BroadcastAction::Prepend,
                                "replace" => BroadcastAction::Replace,
                                _ => BroadcastAction::Append,
                            };
                        }
                    }
                    "target" => target = Some(v.clone()),
                    _ => {}
                }
            }
        }
    }

    out.save.push(LoweredBroadcast {
        action,
        channel: channel.clone(),
        target: target.clone(),
        self_param: self_param.clone(),
        on_association: None,
    });
    out.destroy.push(LoweredBroadcast {
        action: BroadcastAction::Remove,
        channel,
        target,
        self_param,
        on_association: None,
    });
}

/// Parse one `after_{create,destroy}_commit { … }` block. Unwraps
/// `rescue nil` (blog fixture uses it for seeding safety), matches
/// `assoc.broadcast_*_to(channel[, target])`, and resolves the
/// receiver against the model's belongs_to set.
fn collect_commit_block(
    out: &mut Vec<LoweredBroadcast>,
    block: &Expr,
    assocs: &[LoweredAssocRef],
) {
    let body = match &*block.node {
        ExprNode::Lambda { body, .. } => body,
        _ => return,
    };
    let inner = match &*body.node {
        ExprNode::RescueModifier { expr, .. } => expr,
        _ => body,
    };
    let ExprNode::Send {
        recv: Some(recv),
        method,
        args,
        ..
    } = &*inner.node
    else {
        return;
    };
    let action = match method.as_str() {
        "broadcast_replace_to" => BroadcastAction::Replace,
        "broadcast_prepend_to" => BroadcastAction::Prepend,
        "broadcast_append_to" => BroadcastAction::Append,
        "broadcast_remove_to" => BroadcastAction::Remove,
        _ => return,
    };
    // Receiver is the association. Prism parses `article.foo` inside
    // a block as `Send{recv:Send{method:"article",args:[]}, method:"foo"}`
    // — that's the common case. An explicit local (Var) also works,
    // though the fixture uses bare idents.
    let assoc_name: Symbol = match &*recv.node {
        ExprNode::Var { name, .. } => name.clone(),
        ExprNode::Send {
            recv: None,
            method,
            args,
            ..
        } if args.is_empty() => method.clone(),
        _ => return,
    };
    let Some(assoc) = assocs.iter().find(|a| a.name == assoc_name) else {
        return;
    };

    let Some(channel) = args.first().cloned() else {
        return;
    };
    let target = args.get(1).cloned();

    out.push(LoweredBroadcast {
        action,
        channel,
        target,
        self_param: None,
        on_association: Some(assoc.clone()),
    });
}

fn hash_sym_key(k: &Expr) -> Option<Symbol> {
    match &*k.node {
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => Some(value.clone()),
        _ => None,
    }
}

/// One element of a turbo stream name — `turbo_stream_from room,
/// :messages` and `broadcast_append_to room, :messages` each describe
/// their stream as a list of these.
#[derive(Clone, Debug)]
pub enum Streamable {
    /// A literal segment: `:messages`, `"articles"`.
    Literal(String),
    /// A record, contributing `<singular>_<id>` with the id read at
    /// run time.
    Record { singular: String, id: Expr },
}

/// Spell a turbo stream name from its streamables — THE convention,
/// and the reason it lives here rather than in either caller: the
/// subscribe side (`turbo_stream_from` in a view) and the publish side
/// (`broadcast_*_to` on a model) must agree exactly or the message goes
/// to a stream nobody is listening on, silently.
///
/// A RECORD CONTRIBUTES ITS GLOBALID PARAM, matching turbo-rails 2.0.16:
///
/// ```ruby
/// streamables.then { |s| s.try(:to_gid_param) || s.to_param }  # joined by ":"
/// ```
///
/// This used to be `"room_#{room.id}"` — a dom_id-shaped name, with a
/// comment saying "we have no GlobalID". Cheaper, and it worked for as
/// long as both ends of the wire were ours. It stops working the moment
/// the APP's own channel code reads the name back: campfire's
/// `RoomMessagesChannel.subscribable_room` does
/// `GlobalID::Locator.locate gid_param, only: Room`, and `room_1` is not
/// something that resolves. The same rule the `/cable` handshake follows
/// — run the app's code, and hand it inputs in the shape it parses.
///
/// A LITERAL still contributes its own text (`:messages` → `messages`),
/// which is `Symbol#to_param`, so an all-literal name is unchanged and
/// the blog fixture's byte-for-byte e2e pin is untouched.
///
/// The `:` join mirrors Rails' own. The gid is minted by a runtime call
/// rather than interpolated here, so minting has one spelling for the
/// subscribe side, the publish side, and eventually the channel that
/// reads it back.
///
/// TARGET REACH, stated: `GlobalID.param` lives in `runtime/ruby/
/// rails.rb`, which is ruby-family only — it is in no per-target
/// transpile table. A RECORD streamable therefore needs a `GlobalID`
/// twin on any strict target that meets one. None does today: the blog
/// fixture every target builds writes `turbo_stream_from "articles"`
/// and `"article_#{@article.id}_comments"`, both literals, which take
/// the unchanged all-literal path; campfire, the only corpus app that
/// passes a record, emits ruby and spinel. A fixture is what should
/// force those twins into existence rather than eight speculative
/// ports of a function nothing calls.
/// `def to_gid_param; GlobalID.param("Room", @id); end` on every model.
///
/// Rails puts this on every `GlobalID::Identification` includer, which
/// is every Active Record model, and CODE IN THE WILD CALLS IT — not
/// just ours. campfire's own `test/test_helpers/turbo_test_helper.rb`
/// builds the expected stream name with
/// `streamble.try(:to_gid_param) || streamble`, so a model without it
/// takes that helper down (`undefined method 'to_gid_param' for an
/// instance of Rooms::Closed`) and every broadcast assertion with it.
///
/// EVERY MODEL, not the ones this app happens to stream. A "which
/// models qualify" analysis would be wrong the moment a new call site
/// appears, and the call sites are not all ours to see — the app's test
/// helpers are app code.
///
/// The model NAME is a literal rather than `self.class.name`, the same
/// rule `push_attachable_sgid` and `lower::signed_id` state: it is a
/// compile-time fact, and baking it keeps the runtime free of
/// reflection. NOTE the consequence for STI — `Rooms::Closed` mints
/// `gid://app/Rooms::Closed/3` where Rails mints the same, because
/// Rails uses the instance's own class too.
pub fn push_to_gid_param(
    methods: &mut Vec<crate::dialect::MethodDef>,
    model: &crate::dialect::Model,
) {
    let name = crate::ident::Symbol::from("to_gid_param");
    if methods
        .iter()
        .any(|m| m.name == name && m.receiver == crate::dialect::MethodReceiver::Instance)
    {
        return;
    }
    let mut model_lit = Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Lit { value: Literal::Str { value: model.name.0.as_str().to_string() } },
    );
    model_lit.ty = Some(crate::ty::Ty::Str);
    let mut id_read = Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Ivar { name: crate::ident::Symbol::from("id") },
    );
    id_read.ty = Some(crate::ty::Ty::Int);
    let mut body = Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                crate::span::Span::synthetic(),
                ExprNode::Const { path: vec![crate::ident::Symbol::from("GlobalID")] },
            )),
            method: crate::ident::Symbol::from("param"),
            args: vec![model_lit, id_read],
            block: None,
            parenthesized: true,
        },
    );
    body.ty = Some(crate::ty::Ty::Str);
    methods.push(crate::dialect::MethodDef {
        name,
        receiver: crate::dialect::MethodReceiver::Instance,
        params: vec![],
        body,
        signature: None,
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    });
}

/// `GlobalID.param("Room", <id expr>)` — the runtime mint. The model
/// name is camelized from the streamable's singular, which is the same
/// name the record's class carries.
fn gid_param_call(singular: &str, id: &Expr) -> Expr {
    let model = crate::naming::camelize(singular);
    let model_lit = Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Lit { value: Literal::Str { value: model } },
    );
    let recv = Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Const { path: vec![crate::ident::Symbol::from("GlobalID")] },
    );
    Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Send {
            recv: Some(recv),
            method: crate::ident::Symbol::from("param"),
            args: vec![model_lit, id.clone()],
            block: None,
            parenthesized: true,
        },
    )
}

pub fn stream_name(parts: &[Streamable]) -> Expr {
    let mut interp: Vec<crate::expr::InterpPart> = Vec::new();
    let mut pending = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            pending.push(':');
        }
        match part {
            Streamable::Literal(text) => pending.push_str(text),
            Streamable::Record { singular, id } => {
                interp.push(crate::expr::InterpPart::Text {
                    value: std::mem::take(&mut pending),
                });
                interp.push(crate::expr::InterpPart::Expr {
                    expr: gid_param_call(singular, id),
                });
            }
        }
    }
    if !pending.is_empty() {
        interp.push(crate::expr::InterpPart::Text { value: pending });
    }
    // A record in first position pushes an EMPTY leading text part (the
    // separator buffer had nothing in it yet). Harmless to emit and ugly
    // to read, and it would defeat the all-literal check below.
    interp.retain(|p| !matches!(p, crate::expr::InterpPart::Text { value } if value.is_empty()));
    // An all-literal name is a plain String, not a one-part interp —
    // `turbo_stream_from "articles"` has always emitted the literal and
    // the blog's e2e pins that byte-for-byte.
    if let [crate::expr::InterpPart::Text { value }] = interp.as_slice() {
        return Expr::new(
            crate::span::Span::synthetic(),
            ExprNode::Lit { value: Literal::Str { value: value.clone() } },
        );
    }
    Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::StringInterp { parts: interp },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_app;
    use std::path::Path;

    fn model_named<'a>(app: &'a crate::App, name: &str) -> &'a Model {
        app.models
            .iter()
            .find(|m| m.name.0.as_str() == name)
            .expect("model not found")
    }

    #[test]
    fn real_blog_article_broadcasts_to_prepend() {
        let app = ingest_app(Path::new("fixtures/real-blog")).expect("ingest");
        let article = model_named(&app, "Article");
        let lowered = lower_broadcasts(article);
        // One save (prepend) and one destroy (remove).
        assert_eq!(lowered.save.len(), 1);
        assert_eq!(lowered.destroy.len(), 1);
        assert_eq!(lowered.save[0].action, BroadcastAction::Prepend);
        assert_eq!(lowered.destroy[0].action, BroadcastAction::Remove);
        // self_param set — the lambda is `->(_article) { "articles" }`.
        assert_eq!(
            lowered.save[0].self_param.as_ref().map(|s| s.as_str()),
            Some("_article"),
        );
        assert!(lowered.save[0].on_association.is_none());
    }

    #[test]
    fn real_blog_comment_has_broadcasts_to_and_commit_hooks() {
        let app = ingest_app(Path::new("fixtures/real-blog")).expect("ingest");
        let comment = model_named(&app, "Comment");
        let lowered = lower_broadcasts(comment);
        // broadcasts_to + after_create_commit → two save calls.
        // broadcasts_to's destroy + after_destroy_commit → two destroy calls.
        assert_eq!(lowered.save.len(), 2);
        assert_eq!(lowered.destroy.len(), 2);

        // First save: broadcasts_to (append is the default — matches
        // Rails' turbo-rails default when no `inserts_by:` is given).
        assert_eq!(lowered.save[0].action, BroadcastAction::Append);
        assert!(lowered.save[0].on_association.is_none());
        assert_eq!(
            lowered.save[0].self_param.as_ref().map(|s| s.as_str()),
            Some("comment"),
        );

        // Second save: after_create_commit → parent article replace.
        let save_assoc = lowered.save[1]
            .on_association
            .as_ref()
            .expect("second save uses an association");
        assert_eq!(save_assoc.name.as_str(), "article");
        assert_eq!(save_assoc.target_class.as_str(), "Article");
        assert_eq!(save_assoc.target_table, "articles");
        assert_eq!(save_assoc.foreign_key.as_str(), "article_id");
        assert_eq!(lowered.save[1].action, BroadcastAction::Replace);

        // Destroy symmetric — first the broadcasts_to remove, then
        // the after_destroy_commit parent-article replace.
        assert_eq!(lowered.destroy[0].action, BroadcastAction::Remove);
        assert!(lowered.destroy[0].on_association.is_none());
        assert_eq!(lowered.destroy[1].action, BroadcastAction::Replace);
        assert!(lowered.destroy[1].on_association.is_some());
    }

    #[test]
    fn model_without_broadcasts_lowers_to_empty() {
        // ApplicationRecord in any fixture has no broadcast decls.
        let app = ingest_app(Path::new("fixtures/real-blog")).expect("ingest");
        for m in &app.models {
            if m.name.0.as_str() == "ApplicationRecord" {
                let lowered = lower_broadcasts(m);
                assert!(lowered.is_empty());
            }
        }
    }
}
