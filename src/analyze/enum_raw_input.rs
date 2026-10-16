//! Enum storage alone cannot implement Rails' original-input reader.
//! Refuse that generated API until assignment provenance is represented.
//! Source-defined implementations retain ordinary dispatch and diagnostics.

use crate::{App, ClassId, Expr, ExprNode, Symbol};
use crate::diagnostic::Diagnostic;
use crate::dialect::MethodReceiver;
use crate::ty::Ty;

pub(super) fn diagnose(app: &App) -> Vec<Diagnostic> {
    if app.models.iter().all(|model| model.enums.is_empty()) {
        return Vec::new();
    }
    fn walk(expr: &Expr, owner: Option<&ClassId>, app: &App, out: &mut Vec<Diagnostic>) {
        if let ExprNode::Send { recv, method, .. } = &*expr.node {
            if let Some(column) = method.as_str().strip_suffix("_before_type_cast") {
                let receiver = match recv {
                    Some(recv) => match &recv.ty { Some(Ty::Class { id, .. }) => Some(id), _ => None },
                    None => owner,
                };
                if let Some(id) = receiver {
                    if app.models.iter().any(|m| &m.name == id && m.enums.contains_key(&Symbol::from(column)))
                        && !crate::analyze::source_instance_method(app, id, method.as_str())
                    {
                        out.push(Diagnostic::unsupported(expr.span, None, "enum_before_type_cast",
                            format!("{}#{} requires original enum assignment inputs, which are not modeled; a stored-value reader is not equivalent", id.0, method)));
                    }
                }
            }
        }
        expr.node.for_each_child(&mut |child| walk(child, owner, app, out));
    }
    let mut out = Vec::new();
    crate::lower::for_each_hook_body_ref(app, &mut |body| walk(body, None, app, &mut out));
    for model in &app.models {
        for method in model.methods().filter(|m| m.receiver == MethodReceiver::Instance) {
            walk(&method.body, Some(&model.name), app, &mut out);
        }
    }
    for view in &app.views { walk(&view.body, None, app, &mut out); }
    out
}
