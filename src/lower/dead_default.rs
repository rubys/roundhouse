//! Dead kwarg-default grounding: a param default that is a bare
//! zero-arg send resolving to NOTHING on the defining class rewrites
//! to `raise "undefined local variable or method '<name>'"`.
//!
//! MRI never evaluates a default the caller supplies, so Rails code
//! carries statically-broken defaults (lobsters' `for_user: user` on
//! `User#recent_threads` — no `user` method anywhere on the chain,
//! every call site passes the kwarg). AOT targets compile the default
//! expression regardless and refuse the unresolvable send. The raise
//! preserves MRI's observable behavior — evaluating the default
//! raises with the same message — while being statically
//! compilable.
//!
//! Conservative on both sides: only fires when the defining class IS
//! in the analyzer registry and the name resolves nowhere on the
//! chain (own methods, attribute row, includes, then parents — the
//! same order dispatch uses). A class the registry doesn't know, or
//! a name any hop knows, keeps its default untouched. The analyzer
//! already ledgers these sites (`unresolved_type` on reads of the
//! poisoned param), so no new diagnostic is added here.

use std::collections::HashMap;

use crate::analyze::ClassInfo;
use crate::app::App;
use crate::dialect::Param;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::ClassId;

pub fn apply_dead_default_lowering(app: &mut App, registry: &HashMap<ClassId, ClassInfo>) {
    for model in &mut app.models {
        let class = model.name.clone();
        for item in &mut model.body {
            if let crate::dialect::ModelBodyItem::Method { method, .. } = item {
                ground_params(&mut method.params, &class, registry);
            }
        }
    }
    for lc in &mut app.library_classes {
        let class = lc.name.clone();
        for method in &mut lc.methods {
            ground_params(&mut method.params, &class, registry);
        }
    }
}

fn ground_params(params: &mut [Param], class: &ClassId, registry: &HashMap<ClassId, ClassInfo>) {
    if !registry.contains_key(class) {
        return;
    }
    for p in params {
        let Some(default) = &mut p.default else { continue };
        let ExprNode::Send { recv: None, method, args, block: None, .. } = &*default.node else {
            continue;
        };
        if !args.is_empty() || resolves(class, method.as_str(), registry, 0) {
            continue;
        }
        let span = default.span;
        *default = Expr::new(
            span,
            ExprNode::Raise {
                value: Expr::new(
                    span,
                    ExprNode::Lit {
                        value: Literal::Str {
                            value: format!(
                                "undefined local variable or method '{}'",
                                method.as_str()
                            ),
                        },
                    },
                ),
            },
        );
    }
}

fn resolves(
    class: &ClassId,
    name: &str,
    registry: &HashMap<ClassId, ClassInfo>,
    depth: usize,
) -> bool {
    // Unknown hop = can't prove absence; treat as resolving. Depth cap
    // guards a cyclic parent chain.
    if depth > 8 {
        return true;
    }
    let Some(info) = registry.get(class) else { return true };
    if info.instance_methods.keys().any(|m| m.as_str() == name)
        || info.attributes.fields.keys().any(|f| f.as_str() == name)
    {
        return true;
    }
    if info.includes.iter().any(|m| resolves(m, name, registry, depth + 1)) {
        return true;
    }
    match &info.parent {
        Some(parent) => resolves(parent, name, registry, depth + 1),
        None => false,
    }
}
