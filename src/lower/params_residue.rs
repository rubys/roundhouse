// Not silent on the strict targets: their runtimes model params through `Params.sub`/`Params.str` only, so a nested read there is uncompilable.
use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode};
use crate::ty::Ty;

pub fn apply_params_residue_ledger(app: &App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    super::for_each_hook_body_ref(app, &mut |body| walk(body, &mut diags));
    diags
}

fn is_param_value(ty: Option<&Ty>) -> bool {
    matches!(ty, Some(Ty::Class { id, .. }) if id.0.as_str() == crate::analyze::PARAM_VALUE)
}

fn walk(e: &Expr, diags: &mut Vec<Diagnostic>) {
    if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node {
        if is_param_value(r.ty.as_ref()) {
            diags.push(super::residue_diagnostic(
                "params_residue",
                "nested_params",
                e.span,
                "nested_request_params",
                format!(
                    "`{}` on a nested request-params value runs on the request's own hashes and arrays in \
                     ruby-family targets, unsupported at strict-target emit",
                    method.as_str()
                ),
            ));
        }
    }
    e.node.for_each_child(&mut |c| walk(c, diags));
}

#[cfg(not(target_arch = "wasm32"))]
pub fn elevate_nested_params_for_target(diags: &mut [Diagnostic], target: crate::project::BuildTarget) {
    // Not a predicate of its own: the targets with a runtime Relation are exactly the ruby family, whose params are native hashes.
    if target.has_runtime_relation() {
        return;
    }
    for d in diags.iter_mut() {
        let crate::diagnostic::DiagnosticKind::LowerResidue { pass, construct, .. } = &d.kind else {
            continue;
        };
        if pass.as_str() != "params_residue" || construct.as_str() != "nested_params" {
            continue;
        }
        d.severity = crate::diagnostic::Severity::Error;
        d.message = format!(
            "{} — `{}` is one: its params runtime reads only a resource's scalar fields",
            d.message,
            target.as_str(),
        );
    }
}
