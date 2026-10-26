//! Give anonymous `**` / `...` packets their source keyword contract back.
//!
//! Ingest flattens optional keywords (`dir: DEFAULT`) and a named
//! `**rest` into trailing positionals. A literal keyword call can be
//! moved into those slots later; an anonymous packet cannot, because
//! the keys are not in the IR. Emitting the flattened `def` makes
//! `new(...)` and `target(**)` bind the whole packet to one positional.
//! Restoring the keyword parameters leaves the packet untouched and
//! lets Ruby bind it. Named `**hash` calls into the same definition
//! are expanded by `lower::kwsplat` afterwards.
//!
//! Runs at the start of analyze, before any signature is copied.
//! `check` does not apply post-analyze passes, so this is the path
//! both check and emit share.

use std::collections::HashSet;

use crate::App;
use crate::dialect::{MethodDef, ModelBodyItem, Association};
use crate::expr::{Expr, ExprNode};
use crate::ident::ClassId;
use crate::span::Span;

use super::super::body::ConstResolver;
use super::{
    SourceContractIndex, controller_defs, destination, has_forwarding, has_source_packet, methods,
    virtual_destinations,
};

pub(crate) fn restore_anonymous_forward_contracts(app: &mut App, resolver: &ConstResolver) {
    let actions = controller_defs(app);
    let contracts = SourceContractIndex::new(app, &actions);
    let mut spans = HashSet::new();
    for (owner, method) in methods(app).chain(actions.iter().map(|(c, m)| (c, m))) {
        note_anonymous_restores(app, resolver, &contracts, owner, method, &method.body, &mut spans);
        for default in method.params.iter().filter_map(|p| p.default.as_ref()) {
            note_anonymous_restores(app, resolver, &contracts, owner, method, default, &mut spans);
        }
    }
    drop(contracts);
    if spans.is_empty() {
        return;
    }
    restore_marked_params(app, &spans);
}

fn note_anonymous_restores(
    app: &App,
    resolver: &ConstResolver,
    contracts: &SourceContractIndex<'_>,
    owner: &ClassId,
    enclosing: &MethodDef,
    e: &Expr,
    spans: &mut HashSet<Span>,
) {
    if anonymous_packet(enclosing, e) && source_packet_is_declared(enclosing, e) {
        if let Some(resolved) =
            destination(app, contracts, Some((owner, enclosing)), e, Some(resolver))
        {
            if let Ok(virtuals) = virtual_destinations(contracts, Some((owner, enclosing)), e) {
                let mut candidates = vec![(resolved.lookup_owner.clone(), resolved.method)];
                candidates.extend(
                    virtuals
                        .into_iter()
                        .map(|(owner, (method, _))| (owner, method)),
                );
                let blocked = candidates.iter().any(|(lookup_owner, method)| {
                    hard_declaration_block(method)
                        || contracts.unretained.contains(&method.name_span)
                        || contracts.reopened(lookup_owner)
                        || method
                            .enclosing_class
                            .as_ref()
                            .is_some_and(|owner| contracts.reopened(&ClassId(owner.clone())))
                        || contracts.unretained_in_lookup(
                            lookup_owner,
                            &method.name,
                            method.name_span,
                            method.receiver,
                        )
                });
                if !blocked {
                    for (_, method) in candidates {
                        if method.name_span.is_synthetic() {
                            continue;
                        }
                        if method.params.iter().any(|p| p.from_keyword || p.from_kwrest) {
                            spans.insert(method.name_span);
                        }
                    }
                }
            }
        }
    }
    e.node.for_each_child(&mut |c| {
        note_anonymous_restores(app, resolver, contracts, owner, enclosing, c, spans)
    });
}

fn anonymous_packet(enclosing: &MethodDef, e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Send { args, .. } => has_source_packet(args),
        ExprNode::Super { args: Some(args) } => has_source_packet(args),
        ExprNode::Super { args: None } => enclosing.params.iter().any(|p| p.forwarding),
        _ => false,
    }
}

fn source_packet_is_declared(enclosing: &MethodDef, e: &Expr) -> bool {
    if enclosing.unsupported_formals.is_some() {
        return false;
    }
    let args = match &*e.node {
        ExprNode::Send { args, .. } => Some(args.as_slice()),
        ExprNode::Super { args: Some(args) } => Some(args.as_slice()),
        ExprNode::Super { args: None } => None,
        _ => return false,
    };
    let keywords = args.is_some_and(|args| {
        args.iter().any(|arg| {
            matches!(
                &*arg.node,
                ExprNode::ForwardKeywords | ExprNode::ForwardKeywordsWithPairs { .. }
            )
        })
    });
    let full = args.is_none_or(has_forwarding);
    if keywords
        && !enclosing.params.iter().any(|p| {
            p.keyword && p.rest && p.name.as_str().is_empty() && !p.forwarding
        })
    {
        return false;
    }
    if full && !enclosing.params.iter().any(|p| p.forwarding) {
        return false;
    }
    keywords || full
}

fn hard_declaration_block(method: &MethodDef) -> bool {
    method.unsupported_formals.is_some() || method.has_anonymous_block
}

fn restore_marked_params(app: &mut App, spans: &HashSet<Span>) {
    let restore = |method: &mut MethodDef| {
        if !spans.contains(&method.name_span) {
            return;
        }
        for param in &mut method.params {
            if param.from_keyword {
                param.keyword = true;
                param.from_keyword = false;
            } else if param.from_kwrest {
                param.keyword = true;
                param.rest = true;
                param.default = None;
                param.from_kwrest = false;
            }
        }
    };
    for class in &mut app.library_classes {
        for method in &mut class.methods {
            restore(method);
        }
    }
    if let Some(class) = app.rails_application.as_mut() {
        for method in &mut class.methods {
            restore(method);
        }
    }
    for model in &mut app.models {
        for method in model.methods_mut() {
            restore(method);
        }
        for item in &mut model.body {
            if let ModelBodyItem::Association {
                assoc: Association::HasMany { extension, .. },
                ..
            } = item
            {
                for method in extension {
                    restore(method);
                }
            }
        }
    }
    for module in &mut app.test_modules {
        for method in &mut module.helpers {
            restore(method);
        }
        for class in &mut module.inner_classes {
            for method in &mut class.methods {
                restore(method);
            }
        }
    }
}
