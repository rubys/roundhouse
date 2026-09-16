//! ActiveSupport's `String#inquiry` → equality against the label.
//!
//! `"bot_key".inquiry` answers a `StringInquirer`, a String subclass
//! whose `method_missing` turns any predicate into a comparison:
//! `authenticated_by.bot_key?` is `authenticated_by == "bot_key"`. Both
//! halves are unreachable for a transpiled runtime — no target can
//! reopen the builtin, and a `method_missing` predicate is exactly the
//! dynamic dispatch the runtime is required not to have.
//!
//! It is also unnecessary. The predicate name IS the comparand, known
//! at compile time at every call site, so the whole thing folds:
//!
//! ```text
//! authenticated_by.bot_key?                 =>  authenticated_by == "bot_key"
//! involvement_previously_was.inquiry.invisible?
//!                                           =>  involvement_previously_was == "invisible"
//! ```
//!
//! Two rewrites, bottom-up in one walk:
//!
//!   * `<recv>.inquiry` → `<recv>`. The inquirer IS its string; the
//!     subclass exists only to host the predicates this pass removes.
//!   * `<recv>.<name>?` → `<recv> == "<name>"`, when the receiver types
//!     as a String and `<name>?` is not a method a String actually has.
//!
//! That second condition is what keeps the pass honest. `empty?`,
//! `present?`, `start_with?` and their siblings are real String methods
//! and are left alone — asked of `analyze::string_answers`, which is
//! the body-typer's own catalog and the only thing that knows the
//! ActiveSupport core_ext predicates are there. Asking the CLASS
//! registry instead, as this pass first did, answers "String has no
//! methods at all": every predicate looked unknown and
//! `notice.present?` became `notice == "present"`, wrong output that
//! compare never sees because a page without a flash renders the same
//! either way. What remains — a predicate no String answers — has
//! exactly one meaning in Ruby, and it is this one: on a plain String
//! it is a NoMethodError, and on the inquirer it is the comparison.
//!
//! An app defining its own `inquiry` disables the pass wholesale, the
//! same coarse opt-out `exclude_predicate` takes beside this file: the
//! name would then mean something the app chose, and a receiver type
//! rarely names the class that defined it.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_inquiry_lowering(app: &mut App) {
    if app_defines_inquiry(app) {
        return;
    }
    let inquirers = inquirer_methods(app);
    super::for_each_hook_body(app, &mut |body| rewrite(body, &inquirers));
    for view in &mut app.views {
        rewrite(&mut view.body, &inquirers);
    }
}

/// The inquirer-returning methods and the tail test now live in
/// `crate::analyze::inquiry`, shared with the body typer so the
/// analyze-only drivers (`check`, LSP, MCP) answer the predicate the
/// same way this pass folds it.
fn inquirer_methods(app: &App) -> std::collections::HashSet<Symbol> {
    crate::analyze::inquiry::inquirer_methods(app)
}

fn rewrite(expr: &mut Expr, inquirers: &std::collections::HashSet<Symbol>) {
    // `<recv>.inquiry.<name>?` — the DIRECT pair, matched BEFORE the
    // recursive walk folds the `.inquiry` away.
    //
    // The `Ty::Str` gate below cannot see this one: campfire writes
    // `involvement_previously_was.inquiry.invisible?`, and the receiver
    // is a synthesized Dirty reader whose value comes out of an untyped
    // saved-changes diff. The `inquirers` set does not cover it either —
    // that set is for a METHOD whose body ends in `.inquiry`, and here
    // the call is at the SITE.
    //
    // Which makes this the strongest evidence of the three and the only
    // one the pass was throwing away: an explicit `.inquiry` in the
    // source says the author meant the inquirer, whatever the receiver
    // types as. Same reason the doc above gives for collecting
    // `inquirers` first — the rewrite destroys the evidence — applied
    // one level in.
    if let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node {
        if args.is_empty() {
            if let Some(label) = method.as_str().strip_suffix('?') {
                if !label.is_empty() {
                    if let ExprNode::Send {
                        recv: Some(inner),
                        method: inner_method,
                        args: inner_args,
                        block: None,
                        ..
                    } = &*recv.node
                    {
                        if inner_method.as_str() == "inquiry" && inner_args.is_empty() {
                            let mut inner = inner.clone();
                            rewrite(&mut inner, inquirers);
                            let span = expr.span;
                            *expr = eq_label(span, inner, label);
                            return;
                        }
                    }
                }
            }
        }
    }

    expr.node.for_each_child_mut(&mut |c| rewrite(c, inquirers));

    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    if !args.is_empty() {
        return;
    }

    // `<recv>.inquiry` — the inquirer is its string.
    if method.as_str() == "inquiry" {
        let inner = recv.clone();
        *expr = inner;
        return;
    }

    // `<recv>.<name>?` on a String the registry doesn't answer that for.
    let Some(label) = method.as_str().strip_suffix('?') else { return };
    if label.is_empty() || crate::analyze::string_answers(method) {
        return;
    }
    // Either the analyzer typed the receiver a String, or the receiver
    // is a call to a method whose value came from `.inquiry` — the fact
    // collected above, since the fold erases the call itself.
    let recv_is_inquirer = matches!(
        &*recv.node,
        ExprNode::Send { method, .. } if inquirers.contains(method)
    );
    if !matches!(recv.ty, Some(Ty::Str)) && !recv_is_inquirer {
        return;
    }
    let recv = recv.clone();
    let span = expr.span;
    *expr = eq_label(span, recv, label);
}

/// `<recv> == "<label>"`, stamped Bool. The one shape both the direct
/// `.inquiry.<label>?` pair and the typed-receiver predicate fold to,
/// so the two cannot drift.
fn eq_label(span: crate::span::Span, recv: Expr, label: &str) -> Expr {
    let mut out = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("=="),
            args: vec![Expr::new(
                span,
                ExprNode::Lit { value: Literal::Str { value: label.to_string() } },
            )],
            block: None,
            parenthesized: false,
        },
    );
    out.ty = Some(Ty::Bool);
    out
}

/// True when any app class defines `inquiry` itself.
fn app_defines_inquiry(app: &App) -> bool {
    let named = |n: &Symbol| n.as_str() == "inquiry";
    app.models.iter().any(|m| m.methods().any(|d| named(&d.name)))
        || app
            .library_classes
            .iter()
            .any(|lc| lc.methods.iter().any(|d| named(&d.name)))
        || app
            .controllers
            .iter()
            .any(|c| c.actions().any(|a| named(&a.name)))
}
