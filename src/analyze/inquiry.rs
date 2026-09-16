//! The evidence that a `<recv>.<label>?` is an ActiveSupport inquirer
//! predicate, shared by the body typer (which answers it `Bool`) and
//! `lower::inquiry` (which folds it to `<recv> == "<label>"`).
//!
//! `"bot_key".inquiry` answers a `StringInquirer`, a String subclass
//! whose `method_missing` turns any predicate into a comparison. The
//! typer keeps the value a `Str` — a model method returning the
//! inquirer CLASS would put `ActiveSupport::StringInquirer` in every
//! emitted signature — and instead recognises the two shapes the
//! source can take: the explicit `.inquiry.<label>?` pair, and a call
//! to a method whose body ends in `.inquiry` (campfire's
//! `Message#content_type` is a `case … end.inquiry`, read as
//! `message.content_type.attachment?` in a template). Both are the
//! lowering's evidence too; sharing it here is what keeps `check`, the
//! LSP and the MCP — which never run the lowering — from reporting
//! `no known method attachment? on Str` about a call the emitted
//! program answers.
//!
//! Keyed by NAME, which can over-match a same-named method on another
//! class — the same tradeoff `html_safe_methods` takes, and the
//! consequence is confined to a predicate no String answers.

use std::collections::HashSet;

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

/// Methods that RETURN an inquirer — their body's tail is an
/// `.inquiry` call.
pub fn inquirer_methods(app: &App) -> HashSet<Symbol> {
    let mut out = HashSet::new();
    let mut note = |m: &crate::dialect::MethodDef| {
        if tail_is_inquiry(&m.body) {
            out.insert(m.name.clone());
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
pub fn tail_is_inquiry(body: &Expr) -> bool {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.last().is_some_and(tail_is_inquiry),
        ExprNode::Return { value } => tail_is_inquiry(value),
        // `@authenticated_by ||= "".inquiry` — the memoized form: the
        // value is whichever side ran, and the assigned side is the
        // inquirer.
        ExprNode::OpAssign { value, .. } | ExprNode::Assign { value, .. } => {
            tail_is_inquiry(value)
        }
        ExprNode::BoolOp { op: crate::expr::BoolOpKind::Or, right, .. } => {
            tail_is_inquiry(right)
        }
        ExprNode::Send { method, args, block: None, .. } => {
            method.as_str() == "inquiry" && args.is_empty()
        }
        _ => false,
    }
}

/// Is `recv.<method>` (no arguments) an inquirer predicate: a
/// `<label>?` that no String answers, asked of an explicit `.inquiry`
/// or of a call to a method in `inquirers`?
pub fn is_inquiry_predicate(
    recv: &Expr,
    method: &Symbol,
    args: &[Expr],
    inquirers: &HashSet<Symbol>,
) -> bool {
    if !args.is_empty() {
        return false;
    }
    let Some(label) = method.as_str().strip_suffix('?') else { return false };
    if label.is_empty() || crate::analyze::string_answers(method) {
        return false;
    }
    match &*recv.node {
        ExprNode::Send { method: rm, args: rargs, block: None, .. } => {
            (rm.as_str() == "inquiry" && rargs.is_empty()) || inquirers.contains(rm)
        }
        _ => false,
    }
}
