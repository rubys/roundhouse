//! Return soundness and diagnostic checks for opaque transfer probes.
//!
//! A catalog's computed `untyped` is a runtime value, not evidence that
//! an `||` fallback is the only return. Concrete controls may become more
//! precise, but an opaque answer must remain visible at the routed boundary.
use roundhouse::analyze::{DiagnosticKind, diagnose};

pub fn check(
    app: &roundhouse::App,
    rbs: &str,
    prefix: &str,
    depth: usize,
    known_value: Option<&str>,
    known_forward: Option<&str>,
    source: &str,
) {
    let names =
        std::iter::once("value".to_owned()).chain((0..depth).map(|i| format!("forward{i}")));
    let mut result = "";
    for name in names {
        let start = format!("def {prefix}{name}: (String raw) -> ");
        let lines = rbs
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix(&start))
            .collect::<Vec<_>>();
        assert_eq!(
            lines.len(),
            1,
            "missing/duplicated typed parameter: {source}\n{rbs}"
        );
        let actual = lines[0];
        let known = if name == "value" {
            known_value
        } else {
            known_forward
        };
        assert!(
            actual == "untyped"
                || known == Some(actual)
                || (name != "value" && known_value == Some(actual)),
            "unsupported concrete promise for {name}: {source}\n{rbs}"
        );
        if name == "value" {
            result = actual;
        }
    }
    let diagnostics = diagnose(app);
    let at_call = |d: &&roundhouse::analyze::Diagnostic| {
        let file = &app.sources[d.span.file.0 as usize - 1];
        file.path == "app/controllers/probes_controller.rb" && file.line_col(d.span.start) == (3, 3)
    };
    if result == "untyped" {
        // Warning policy only reports bare untyped. Pin the inferred receiver
        // directly so a retained concrete | untyped union stays covered too.
        assert!(
            unknown_receiver_at_call(app),
            "unknown return lost its boundary type: {source}\n{diagnostics:?}"
        );
    } else if matches!(result, "String" | "Symbol") {
        let errors = diagnostics
            .iter()
            .filter(at_call)
            .filter(|d| {
                matches!(&d.kind, DiagnosticKind::SendDispatchFailed { method, .. }
                if method.as_str() == "no_such_method")
            })
            .count();
        assert_eq!(
            errors, 1,
            "known invalid call lost its error: {source}\n{diagnostics:?}"
        );
    }
}

fn unknown_receiver_at_call(app: &roundhouse::App) -> bool {
    fn visit(app: &roundhouse::App, expr: &roundhouse::expr::Expr, found: &mut bool) {
        if expr.span.file.0 > 0 {
            let file = &app.sources[expr.span.file.0 as usize - 1];
            if file.path == "app/controllers/probes_controller.rb"
                && file.line_col(expr.span.start) == (3, 3)
                && let roundhouse::expr::ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node
                && method.as_str() == "no_such_method"
                && recv.ty.as_ref().is_some_and(roundhouse::ty::Ty::has_untyped_arm)
            {
                *found = true;
            }
        }
        expr.node.for_each_child(&mut |child| visit(app, child, found));
    }
    let mut found = false;
    for controller in &app.controllers {
        for action in controller.actions() {
            visit(app, &action.body, &mut found);
        }
    }
    for class in &app.library_classes {
        for method in &class.methods {
            visit(app, &method.body, &mut found);
        }
    }
    found
}
