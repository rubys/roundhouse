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

/// Methods that RETURN an inquirer — their body's tail is the
/// `.inquiry` call this pass is about to erase.
///
/// Collected before the rewrite because the rewrite destroys the
/// evidence: campfire's `Message#content_type` is a `case … end.inquiry`,
/// and once that call is folded away nothing downstream can tell the
/// method from any other String-returning reader. The call SITE is what
/// needs to know — `message.content_type.attachment?` lives in a
/// template, where the receiver is a user method whose return type the
/// analyzer does not infer, so the `Ty::Str` gate below never fires and
/// the predicate survived to raise NoMethodError on a String.
///
/// Same shape as `lower::html_safe` recording `App::html_safe_methods`:
/// a value-level fact the runtime cannot carry, so record it and erase
/// the call. Keyed by NAME, which can over-match a same-named method on
/// another class — the same tradeoff `html_safe_methods` takes, and the
/// consequence is confined to a predicate no String answers.
fn inquirer_methods(app: &App) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let mut note = |m: &crate::dialect::MethodDef| {
        if tail_is_inquiry(&m.body) {
            out.insert(m.name.as_str().to_string());
        }
    };
    for model in &app.models {
        for method in model.methods() {
            note(method);
        }
    }
    for lc in &app.library_classes {
        for method in &lc.methods {
            note(method);
        }
    }
    out
}

/// Does this body's VALUE — its tail expression — come from `.inquiry`?
fn tail_is_inquiry(body: &Expr) -> bool {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.last().is_some_and(tail_is_inquiry),
        ExprNode::Return { value } => tail_is_inquiry(value),
        ExprNode::Send { method, args, block: None, .. } => {
            method.as_str() == "inquiry" && args.is_empty()
        }
        _ => false,
    }
}

fn rewrite(expr: &mut Expr, inquirers: &std::collections::HashSet<String>) {
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
        ExprNode::Send { method, .. } if inquirers.contains(method.as_str())
    );
    if !matches!(recv.ty, Some(Ty::Str)) && !recv_is_inquirer {
        return;
    }
    let recv = recv.clone();
    let span = expr.span;
    *expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("=="),
            args: vec![Expr::new(span, ExprNode::Lit { value: Literal::Str { value: label.to_string() } })],
            block: None,
            parenthesized: false,
        },
    );
    expr.ty = Some(Ty::Bool);
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
