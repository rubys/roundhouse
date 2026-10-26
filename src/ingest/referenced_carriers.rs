//! A nested `module ClassMethods` is folded onto its owner as class
//! methods and not emitted on its own: under ActiveSupport::Concern or
//! an `included` bridge that is all Ruby makes of it. A module that
//! another one names, though — ancestry's `has_ancestry` does
//! `extend Ancestry::ClassMethods` — is a module Ruby defines and the
//! program reads as a constant. Each one named by a qualified path from
//! outside its owner is ingested again from its source as that module.

use std::collections::HashSet;

use crate::App;
use crate::dialect::ModelBodyItem;
use crate::expr::{Expr, ExprNode};
use crate::span::SourceFile;

use super::library_class::library_class_from_module_node_with_scope;
use super::util::{find_all_modules_with_scope, module_name_path};

const CARRIER: &str = "ClassMethods";

pub(super) fn surface_referenced_carriers(app: &mut App, sources: &[SourceFile]) {
    let known: HashSet<String> = app.library_classes.iter().map(|c| c.name.0.as_str().to_string()).collect();
    let mut wanted: HashSet<String> = HashSet::new();
    let mut note = |site: Option<&str>, expr: &Expr| {
        visit(expr, &mut |e| {
            let ExprNode::Const { path } = &*e.node else { return };
            let segments: Vec<&str> = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
            let [owner @ .., last] = segments.as_slice() else { return };
            if *last != CARRIER || owner.is_empty() {
                return;
            }
            let owner = owner.join("::");
            // The owner's own `base.extend Owner::ClassMethods` is the bridge the fold already stands for
            if site == Some(owner.as_str()) || !known.contains(&owner) {
                return;
            }
            let name = format!("{owner}::{CARRIER}");
            if !known.contains(&name) {
                wanted.insert(name);
            }
        });
    };
    for model in &app.models {
        for item in &model.body {
            if let ModelBodyItem::Unknown { expr, .. } = item {
                note(None, expr);
            }
        }
        for method in model.methods() {
            note(None, &method.body);
        }
    }
    for class in &app.library_classes {
        let site = Some(class.name.0.as_str());
        for call in &class.unknown_calls {
            note(site, call);
        }
        for method in &class.methods {
            note(site, &method.body);
        }
    }
    if wanted.is_empty() {
        return;
    }
    for source in sources {
        if !source.text.contains("module ClassMethods") {
            continue;
        }
        let parsed = super::prism::parse(source.text.as_bytes(), &source.path);
        let root = parsed.node();
        for (scope, module) in find_all_modules_with_scope(&root) {
            if scope.is_empty() || module_name_path(&module).as_deref() != Some(&[CARRIER.to_string()]) {
                continue;
            }
            let name = format!("{}::{CARRIER}", scope.join("::"));
            if !wanted.contains(&name) {
                continue;
            }
            if let Ok(carrier) = library_class_from_module_node_with_scope(&module, &scope, &source.path) {
                app.library_classes.push(carrier);
            }
        }
    }
}

fn visit(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    e.node.for_each_child(&mut |c| visit(c, f));
}
