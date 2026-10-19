//! A nested `def` — `Class.new(Parent) { def m = ... }`, a `private
//! def` in a `tap`, a `def` inside another method's body, any method
//! defined at run time from expression position — has no IR node. Ingest keeps the site as a nil
//! stamped with the refusal (`diagnostic::NESTED_DEF`), so it is no
//! longer the silent drop it was: campfire's
//! `Class.new(WebPush::Connections::HTTP) { private def begin_transport
//! ... }` emitted as `{ private nil }`, a class without the one method
//! it was made for, and nothing said so.
//!
//! App code needs nothing here. `analyze::diagnostics` reports the
//! stamp as an error, and the emitters render it as a raise.
//!
//! A TEST is stubbed whole, as `lower::object_extend` stubs one: what
//! follows the definition is written against the method it defined, so
//! a raise at the site would leave the rest to fail one line down for a
//! reason that is not the real one. Here the test fails with the
//! refusal on every lane, and the file still links. A WARNING, like the
//! extend stub: a test that reaches for a dynamic shape is a gap to
//! record, not a reason to withhold the whole tree. Setup and helper
//! bodies keep the site-level raise and report the same warning.

use crate::app::App;
use crate::diagnostic::{NESTED_DEF, Diagnostic, DiagnosticKind, Severity};
use crate::expr::{Expr, ExprNode, Literal};

pub fn apply_nested_def_test_stub(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            if let Some(d) = first_stamp(setup) {
                diags.push(d);
            }
        }
        for t in &mut tm.tests {
            if let Some(d) = first_stamp(&t.body) {
                t.body = stub_expr(t.body.span, &d);
                diags.push(d);
            }
        }
        for m in &mut tm.helpers {
            if let Some(d) = first_stamp(&m.body) {
                diags.push(d);
            }
        }
    }
    diags
}

/// The first stamped site in `expr`, as the warning it reports.
fn first_stamp(expr: &Expr) -> Option<Diagnostic> {
    if let Some(DiagnosticKind::Unsupported { target, construct, detail }) = &expr.diagnostic()
        && construct.as_str() == NESTED_DEF
    {
        let mut d = Diagnostic::unsupported(expr.span, target.clone(), NESTED_DEF, detail.clone());
        d.severity = Severity::Warning;
        return Some(d);
    }
    let mut found = None;
    expr.node.for_each_child(&mut |c| {
        if found.is_none() {
            found = first_stamp(c);
        }
    });
    found
}

/// A nil carrying the report — the ruby emitter renders it as the raise.
fn stub_expr(span: crate::span::Span, d: &Diagnostic) -> Expr {
    let mut e = Expr::new(span, ExprNode::Lit { value: Literal::Nil });
    e.set_diagnostic(Some(d.kind.clone()));
    e
}
