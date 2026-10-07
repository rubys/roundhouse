//! Deferred `includes`/`preload` for the ruby family: every association
//! reader waits on its record's pending preload before reading its cache.
//!
//! `ActiveRecord::Relation#load_records` no longer batch-loads a
//! relation's included associations when the rows arrive; it hands each
//! record an `ActiveRecord::PendingPreload` for the group
//! (runtime/ruby/active_record/relation.rb). The first association read
//! on any record of the group runs that preload, for all of them, once.
//! On campfire's room page the 40 messages only feed a cached collection
//! whose key needs ids and updated_at, so on a hit no association is read
//! and the rich text, creator, boost and attachment loads never run.
//!
//! This pass is the reader half: an instance method of a model whose
//! first statement reads an association's loaded flag (`@<assoc>_loaded`,
//! `@__rich_text_<attr>_loaded`, ... — the guard `apply_preload_lowering`
//! and the rich-text lowering put on every reader, and the body of
//! `<assoc>_loaded?`), or reads an ivar one of the model's `_preload_*`
//! writers fills (an attachment reader checks `@<name>_cache` for nil),
//! gets `_await_preload` prepended. On a record with nothing pending that
//! is one nil check. `_autosave_*` is left alone: a save has no reason to
//! run the group's preload. Ruby-family only, applied at emit like
//! `lazy_model_state`: the strict targets keep eager loading.

use std::collections::HashSet;

use crate::dialect::{LibraryClass, MethodReceiver};
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::span::Span;
use crate::App;

pub fn apply(lcs: &mut [LibraryClass], app: &App) {
    for lc in lcs.iter_mut() {
        if !app.models.iter().any(|m| m.name == lc.name) {
            continue;
        }
        // Ivars the model's preload writers fill (`_preload_<assoc>`).
        let filled: HashSet<String> = lc
            .methods
            .iter()
            .filter(|m| m.name.as_str().starts_with("_preload_") && matches!(m.receiver, MethodReceiver::Instance))
            .flat_map(|m| ivar_names(&format!("{:?}", m.body)))
            .collect();
        for m in lc.methods.iter_mut() {
            if !matches!(m.receiver, MethodReceiver::Instance) {
                continue;
            }
            let name = m.name.as_str();
            if name == "initialize"
                || name.starts_with("_preload_")
                || name.starts_with("_autosave_")
                || name == "_await_preload"
            {
                continue;
            }
            let first = match &*m.body.node {
                ExprNode::Seq { exprs } => match exprs.first() {
                    Some(e) => format!("{e:?}"),
                    None => continue,
                },
                _ => format!("{:?}", m.body),
            };
            if !ivar_names(&first).iter().any(|n| n.ends_with("_loaded") || filled.contains(n)) {
                continue;
            }
            let wait = Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("_await_preload"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            );
            if let ExprNode::Seq { exprs } = &mut *m.body.node {
                exprs.insert(0, wait);
            } else {
                let body = std::mem::replace(&mut m.body, Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }));
                m.body = Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![wait, body] });
            }
        }
    }
}

/// The ivar names in an expression's debug rendering: the first quoted
/// string after each `Ivar { name: ` (reads and assignment targets alike).
fn ivar_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("Ivar { name: ") {
        rest = &rest[at + "Ivar { name: ".len()..];
        let Some(open) = rest.find('"') else { break };
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        out.push(after[..close].to_string());
        rest = &after[close..];
    }
    out
}
