//! Where the `Oj.mimic_JSON` stand-in is allowed to ship.
//!
//! Ingest installs one Ruby class (`ingest::oj`) whose `generate` is
//! `JSON.generate` and whose `load` is the one-argument `JSON.parse`.
//! Ruby, JRuby and Spinel emit that class and run it. Every other
//! transpile target either drops the class (Python, Crystal) or refuses
//! an incidental call inside it (`String#read` on TypeScript, the `JSON`
//! constant on Rust and the other strict targets). Both are the wrong
//! program: a call with nothing behind it, or an error that is not about
//! Oj. This pass is the one gate. It drops the class, then rewrites each
//! `Oj.*` call and each `Oj` / `Oj::…` constant on the emit-body walk
//! into a `raise`, and records one unsupported diagnostic so a clean
//! check cannot claim the reference.

use crate::App;
use crate::dialect::LibraryClassOrigin;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::project::BuildTarget;

/// Targets that emit the stand-in and execute it as Ruby.
pub fn stand_in_runs(target: BuildTarget) -> bool {
    matches!(
        target,
        BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel
    )
}

/// Blog, Roda and Futamura keep the app's own call: they run Ruby (the
/// real gem, or residual Rails), not a transpile that would drop the class.
fn keeps_source_call(target: BuildTarget) -> bool {
    matches!(
        target,
        BuildTarget::Blog | BuildTarget::Roda | BuildTarget::Futamura
    )
}

/// `None` when `app` is already what `target` should emit. `Some` is a
/// copy with the stand-in removed and each `Oj` call or constant rewritten.
pub fn without_unrun_stand_in(app: &App, target: BuildTarget) -> Option<App> {
    if stand_in_runs(target) || keeps_source_call(target) {
        return None;
    }
    if !app
        .library_classes
        .iter()
        .any(|lc| matches!(lc.origin, Some(LibraryClassOrigin::Oj)))
    {
        return None;
    }
    let mut app = app.clone();
    // Drop the class before the walk so its own `JSON.parse` is not refused,
    // and so a later emit cannot write the class the calls no longer reach.
    app.library_classes
        .retain(|lc| !matches!(lc.origin, Some(LibraryClassOrigin::Oj)));
    let target_name = target.as_str();
    crate::lower::for_each_emit_body(&mut app, &mut |body| rewrite(body, target_name));
    Some(app)
}

fn rewrite(expr: &mut Expr, target: &str) {
    if let Some(method) = oj_send_method(expr) {
        rewrite_send_args(expr, target);
        refuse(expr, target, &format!("`Oj.{method}`"));
        return;
    }
    expr.node
        .for_each_child_mut(&mut |child| rewrite(child, target));
    if let Some(name) = oj_const_name(expr) {
        refuse(expr, target, &format!("`{name}`"));
    }
}

fn rewrite_send_args(expr: &mut Expr, target: &str) {
    let ExprNode::Send { args, block, .. } = &mut *expr.node else {
        return;
    };
    for arg in args {
        rewrite(arg, target);
    }
    if let Some(block) = block {
        rewrite(block, target);
    }
}

/// A call whose receiver is the top-level `Oj` constant (`Oj.load`, `::Oj.generate`).
fn oj_send_method(expr: &Expr) -> Option<String> {
    let ExprNode::Send {
        recv: Some(recv),
        method,
        ..
    } = &*expr.node
    else {
        return None;
    };
    if !is_top_level_oj(recv) {
        return None;
    }
    Some(method.as_str().to_string())
}

/// `Oj`, `::Oj`, or `Oj::ParseError`. A constant nested under another class
/// (`Foo::Oj`) is a different name.
fn oj_const_name(expr: &Expr) -> Option<String> {
    let ExprNode::Const { path } = &*expr.node else {
        return None;
    };
    let mut parts = path.iter().filter(|seg| !seg.as_str().is_empty());
    if parts.next().is_none_or(|seg| seg.as_str() != "Oj") {
        return None;
    }
    let mut name = String::from("Oj");
    for seg in parts {
        name.push_str("::");
        name.push_str(seg.as_str());
    }
    Some(name)
}

fn is_top_level_oj(expr: &Expr) -> bool {
    matches!(oj_const_name(expr).as_deref(), Some("Oj"))
}

fn refuse(expr: &mut Expr, target: &str, name: &str) {
    crate::emit::diagnostics::report_unsupported(
        expr.span,
        target,
        "Oj",
        format!(
            "{name} lowers to the Ruby stand-in, which this target does not emit. \
             The reference is refused instead of being written against a missing class"
        ),
    );
    let span = expr.span;
    let ty = expr.ty.clone();
    let mut raised = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("raise"),
            args: vec![Expr::new(
                span,
                ExprNode::Lit {
                    value: Literal::Str {
                        value: "oj stand-in is not emitted on this target".into(),
                    },
                },
            )],
            block: None,
            parenthesized: true,
        },
    );
    raised.ty = ty;
    *expr = raised;
}
