//! A class method a model took from a gem module it extends
//! (`ingest::extend_splice`), called on a receiver whose type is
//! unknown. Rails' relation answers its model's class methods, running
//! them against itself; ancestry's `scope_depth(…)` returns the class
//! or a relation through a dynamic `send`, and
//! `.ordered_by_ancestry.ancestors_of(self)` is then called on either.
//!
//! The Ruby family answers it: `scope_chain` counts each such method as
//! one a relation reaches, so the model takes the relation parameter
//! and the emitted `relation_scopes.rb` gives `ActiveRecord::Relation`
//! the delegate. Other targets have no relation to hand on, and are
//! refused by name at emit (`project::report_relation_class_calls`).

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::dialect::MethodReceiver;
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

/// Each spliced class method name, with the one model that has it.
pub fn spliced_owners(app: &App) -> HashMap<Symbol, ClassId> {
    let mut owners: HashMap<Symbol, HashSet<ClassId>> = HashMap::new();
    for model in &app.models {
        for m in model.methods().filter(|m| m.receiver == MethodReceiver::Class && app.spliced_class_methods.contains(&m.name)) {
            owners.entry(m.name.clone()).or_default().insert(model.name.clone());
        }
    }
    owners.into_iter().filter(|(_, set)| set.len() == 1).map(|(name, set)| (name, set.into_iter().next().unwrap())).collect()
}

fn untyped_receiver(r: &Expr) -> bool {
    !matches!(&*r.node, ExprNode::Const { .. } | ExprNode::SelfRef)
        && matches!(r.ty.as_ref(), None | Some(Ty::Untyped) | Some(Ty::Var { .. }))
}

/// Spans of the calls a target without a relation to hand on cannot run.
pub fn untyped_calls(app: &App) -> Vec<(Span, Symbol)> {
    let mut out = Vec::new();
    if app.spliced_class_methods.is_empty() {
        return out;
    }
    fn visit(e: &Expr, names: &HashSet<Symbol>, out: &mut Vec<(Span, Symbol)>) {
        if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node
            && names.contains(method)
            && untyped_receiver(r)
        {
            out.push((e.span, method.clone()));
        }
        e.node.for_each_child(&mut |c| visit(c, names, out));
    }
    super::for_each_hook_body_ref(app, &mut |body| visit(body, &app.spliced_class_methods, &mut out));
    out.sort_by_key(|(span, _)| (span.file.0, span.start));
    out.dedup_by_key(|(span, _)| *span);
    out
}
