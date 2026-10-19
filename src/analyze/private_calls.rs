use std::collections::HashMap;

use crate::App;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::dialect::{MethodDef, MethodReceiver, MethodVisibility};
use crate::expr::{Expr, ExprNode};
use crate::ident::ClassId;
use crate::ty::Ty;

struct Owners<'a> {
    parents: HashMap<&'a ClassId, &'a ClassId>,
    defined: HashMap<(&'a ClassId, bool, &'a str), MethodVisibility>,
}

impl<'a> Owners<'a> {
    fn of(app: &'a App) -> Self {
        let mut out = Owners { parents: HashMap::new(), defined: HashMap::new() };
        let mut define = |owner: &'a ClassId, methods: &mut dyn Iterator<Item = &'a MethodDef>| {
            for m in methods {
                let key = (owner, m.receiver == MethodReceiver::Class, m.name.as_str());
                // Not the first definition: Ruby keeps the later one, visibility included.
                out.defined.insert(key, m.visibility);
            }
        };
        for model in &app.models {
            define(&model.name, &mut model.methods());
        }
        for lc in &app.library_classes {
            // Not a module's instance side: whether an includer's call reaches it depends on the includer.
            define(&lc.name, &mut lc.methods.iter().filter(|m| !lc.is_module || m.receiver == MethodReceiver::Class));
        }
        for model in &app.models {
            for (class_side, name, visibility) in crate::lower::model_to_library::inherited_visibility(model) {
                out.defined.insert((&model.name, class_side, name), visibility);
            }
        }
        for model in &app.models {
            if let Some(p) = &model.parent {
                out.parents.insert(&model.name, p);
            }
        }
        for lc in &app.library_classes {
            if let Some(p) = &lc.parent {
                out.parents.insert(&lc.name, p);
            }
        }
        out
    }

    fn private(&self, class: &ClassId, class_side: bool, name: &str) -> bool {
        let mut cur = Some(class);
        for _ in 0..32 {
            let Some(id) = cur else { return false };
            if let Some(visibility) = self.defined.get(&(id, class_side, name)) {
                return *visibility == MethodVisibility::Private;
            }
            cur = self.parents.get(id).copied();
        }
        false
    }
}

pub(crate) fn diagnose(app: &App) -> Vec<Diagnostic> {
    let owners = Owners::of(app);
    let mut out = Vec::new();
    let mut visit = |body: &Expr| walk(body, &owners, &mut out);
    for model in &app.models {
        model.methods().for_each(|m| visit(&m.body));
        model.scopes().for_each(|s| visit(&s.body));
    }
    for controller in &app.controllers {
        controller.actions().for_each(|a| visit(&a.body));
        controller.class_methods().for_each(|m| visit(&m.body));
    }
    for lc in &app.library_classes {
        lc.methods.iter().for_each(|m| visit(&m.body));
    }
    for view in &app.views {
        visit(&view.body);
    }
    if let Some(seeds) = &app.seeds {
        visit(seeds);
    }
    out
}

fn walk(expr: &Expr, owners: &Owners<'_>, out: &mut Vec<Diagnostic>) {
    if let ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node
        && !matches!(&*recv.node, ExprNode::SelfRef)
        && let Some(Ty::Class { id, .. }) = &recv.ty
    {
        // Not the receiver's type: `self.class` types as the class it names, the same `Ty` an instance has.
        let class_side = match &*recv.node {
            ExprNode::Const { .. } => true,
            ExprNode::Send { method, args, .. } => method.as_str() == "class" && args.is_empty(),
            _ => false,
        };
        if owners.private(id, class_side, method.as_str()) {
            let kind = DiagnosticKind::SendDispatchFailed { method: method.clone(), recv_ty: Ty::Class { id: id.clone(), args: vec![].into() } };
            out.push(Diagnostic {
                span: expr.span,
                severity: crate::diagnostic::Severity::Error,
                kind,
                message: format!(
                    "private method `{}` called on {}{}: only `self` reaches it",
                    method.as_str(),
                    id.0.as_str(),
                    if class_side { "" } else { " (an instance)" }
                ),
            });
        }
    }
    expr.node.for_each_child(&mut |child| walk(child, owners, out));
}
