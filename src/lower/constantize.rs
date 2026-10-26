use std::collections::BTreeSet;

use crate::App;
use crate::build_target::BuildTarget;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;

fn case_runs(target: BuildTarget) -> bool {
    matches!(target, BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel)
}

fn keeps_source_call(target: BuildTarget) -> bool {
    matches!(target, BuildTarget::Blog | BuildTarget::Roda | BuildTarget::Futamura)
}

pub fn lower_for_target(app: &App, target: BuildTarget) -> Option<App> {
    if keeps_source_call(target) {
        return None;
    }
    let mut found = false;
    crate::lower::for_each_emit_body_ref(app, &mut |body| found |= has_call(body));
    if !found {
        return None;
    }
    let runs = case_runs(target);
    let lookup = runs.then(|| lookup_class(&app_constants(app)));
    let mut app = app.clone();
    let target_name = target.as_str();
    crate::lower::for_each_emit_body(&mut app, &mut |body| rewrite(body, runs, target_name));
    app.library_classes.extend(lookup.into_iter().flatten());
    Some(app)
}

// Only the app's own constants: a name from run-time data must not reach every class in the process
fn app_constants(app: &App) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    names.extend(app.models.iter().map(|m| m.name.0.as_str().to_string()));
    names.extend(app.controllers.iter().map(|c| c.name.0.as_str().to_string()));
    names.extend(
        app.library_classes
            .iter()
            .filter(|lc| lc.origin.is_none())
            .map(|lc| lc.name.0.as_str().to_string()),
    );
    names
}

fn constantize_method(expr: &Expr) -> Option<&'static str> {
    let ExprNode::Send { recv: Some(_), method, args, block: None, .. } = &*expr.node else { return None };
    if !args.is_empty() {
        return None;
    }
    match method.as_str() {
        "constantize" => Some("constantize"),
        "safe_constantize" => Some("safe_constantize"),
        _ => None,
    }
}

// `lower::record_snapshot` rewrites `name.constantize.instantiate(attrs)` itself, so it is left alone here
fn is_snapshot_rebuild(expr: &Expr) -> bool {
    let ExprNode::Send { recv: Some(recv), method, args, .. } = &*expr.node else { return false };
    method.as_str() == "instantiate" && args.len() == 1 && constantize_method(recv) == Some("constantize")
}

fn has_call(expr: &Expr) -> bool {
    if is_snapshot_rebuild(expr) {
        let ExprNode::Send { args, .. } = &*expr.node else { return false };
        return args.iter().any(has_call);
    }
    let mut found = constantize_method(expr).is_some();
    expr.node.for_each_child(&mut |child| found |= has_call(child));
    found
}

const LOOKUP: &str = "RoundhouseConstants";

fn rewrite(expr: &mut Expr, runs: bool, target: &str) {
    if is_snapshot_rebuild(expr) {
        if let ExprNode::Send { args, .. } = &mut *expr.node {
            for arg in args {
                rewrite(arg, runs, target);
            }
        }
        return;
    }
    expr.node.for_each_child_mut(&mut |child| rewrite(child, runs, target));
    let Some(method) = constantize_method(expr) else { return };
    let span = expr.span;
    let ExprNode::Send { recv: Some(recv), .. } = &*expr.node else { return };
    let recv = recv.clone();
    *expr = if runs {
        Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from(LOOKUP)] })),
                method: Symbol::from(method),
                args: vec![recv],
                block: None,
                parenthesized: true,
            },
        )
    } else {
        let text = crate::emit::diagnostics::report_unsupported(
            span,
            target,
            method,
            format!(
                "`String#{method}` resolves a name only over the app's own constants, \
                 as a `case` that this target does not run"
            ),
        );
        refusal(span, &text)
    };
}

// One shared `case`: inlined at every call site, a large app's emit grew thirteenfold
fn lookup_class(names: &BTreeSet<String>) -> Vec<crate::dialect::LibraryClass> {
    let mut source = format!("class {LOOKUP}\n  def self.safe_constantize(name)\n    case name.delete_prefix(\"::\")\n");
    for name in names {
        source.push_str(&format!("    when {name:?} then {name}\n"));
    }
    source.push_str("    end\n  end\n\n  def self.constantize(name)\n");
    source.push_str("    safe_constantize(name) || raise(NameError, \"uninitialized constant #{name}\")\n  end\nend\n");
    crate::ingest::ingest_library_classes(source.as_bytes(), "<roundhouse constants>").expect("the constant lookup parses")
}

fn refusal(span: Span, text: &str) -> Expr {
    let error = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("NotImplementedError")] })),
            method: Symbol::from("new"),
            args: vec![Expr::new(span, ExprNode::Lit { value: Literal::Str { value: text.to_string() } })],
            block: None,
            parenthesized: true,
        },
    );
    Expr::new(span, ExprNode::Raise { value: error })
}
