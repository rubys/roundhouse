//! `obj.extend Mod` on an INSTANCE — mixing a module into one live
//! object — is outside the subset every target compiles: the class
//! graph and each object's method table are fixed at compile time, and
//! spinel refuses the call outright ("Object#extend is not supported by
//! AOT compilation: mixing a module into a live object needs a
//! per-object method table"). Left in the emit it is a compile error on
//! the strict lanes, and on spinel one such line keeps a whole test
//! file from LINKING, charging every test in it — campfire's
//! `action_text_attachment_test` makes a `Room` attachable for one test
//! (`rooms(:pets).tap { |r| r.extend ActionText::Attachable }`) and the
//! file's other two tests were never run.
//!
//! So the site becomes a raise stub carrying the report, the same
//! `Expr.diagnostic` short-circuit the arel stub uses: the test that
//! wrote it fails at that line with the reason, the file links, and
//! the ledger names the construct. A WARNING, not an error — the strict
//! archive build refuses an error, and a test that reaches for a
//! dynamic shape is a gap to record, not a reason to withhold the
//! whole tree.
//!
//! In a TEST the stub is the whole test body, not the one site: what
//! follows the `extend` is written against the object it produced
//! (`rooms(:pets).attachable_sgid` — a method `Room` does not have,
//! which is what the `extend` was for), so it cannot type either, and
//! a raise at the first line leaves the rest to fail compilation one
//! line down. The test fails with the reason on every lane; the file
//! links. App code keeps the site-level stub: a raise there is the
//! right runtime answer, and what follows it is the app's own.
//!
//! Only the instance form. `extend Foo` in a class body (no receiver)
//! and `Foo.extend Bar` (a constant receiver) restructure a CLASS,
//! which the ingest reads as a body item and the strict targets model
//! statically; neither reaches this pass as a Send on an object.

use crate::app::App;
use crate::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

const CONSTRUCT: &str = "Object#extend";
const DETAIL: &str = "mixing a module into one live object needs a per-object method \
                      table, which no compiled target has; the site raises";

pub fn apply_object_extend_stub(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |e| stub(e, &mut diags));
    for view in &mut app.views {
        stub(&mut view.body, &mut diags);
    }
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            stub(setup, &mut diags);
        }
        for t in &mut tm.tests {
            let before = diags.len();
            stub(&mut t.body, &mut diags);
            if diags.len() > before {
                t.body = stub_expr(t.body.span);
            }
        }
        for m in &mut tm.helpers {
            stub(&mut m.body, &mut diags);
        }
    }
    diags
}

fn stub(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    expr.node.for_each_child_mut(&mut |c| stub(c, diags));
    let ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node else { return };
    if method.as_str() != "extend" {
        return;
    }
    // A constant or `self` receiver is the class-level form.
    if matches!(&*recv.node, ExprNode::Const { .. } | ExprNode::SelfRef) {
        return;
    }
    let mut d = Diagnostic::unsupported(expr.span, None, CONSTRUCT, DETAIL);
    d.severity = Severity::Warning;
    diags.push(d);
    *expr = stub_expr(expr.span);
}

/// A nil carrying the report — the ruby emitter renders it as the raise.
fn stub_expr(span: crate::span::Span) -> Expr {
    let mut e = Expr::new(span, ExprNode::Lit { value: Literal::Nil });
    e.diagnostic = Some(DiagnosticKind::Unsupported {
        target: None,
        construct: Symbol::from(CONSTRUCT),
        detail: DETAIL.to_string(),
    });
    e
}
