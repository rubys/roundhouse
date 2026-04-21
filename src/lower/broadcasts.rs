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
