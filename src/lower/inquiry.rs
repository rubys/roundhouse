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
//! `start_with?`, `frozen?` and their siblings are real String methods
//! and are left alone; the registry is consulted rather than a list
//! kept here, so a method the runtime grows is covered without editing
//! this file. What remains — a predicate no String answers — has
//! exactly one meaning in Ruby, and it is this one: on a plain String
//! it is a NoMethodError, and on the inquirer it is the comparison.
//!
//! An app defining its own `inquiry` disables the pass wholesale, the
//! same coarse opt-out `exclude_predicate` takes beside this file: the
//! name would then mean something the app chose, and a receiver type
//! rarely names the class that defined it.

use std::collections::HashMap;

use crate::analyze::ClassInfo;
use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub fn apply_inquiry_lowering(app: &mut App, registry: &HashMap<ClassId, ClassInfo>) {
    if app_defines_inquiry(app) {
        return;
    }
    let known = string_methods(registry);
    super::for_each_hook_body(app, &mut |e| rewrite(e, &known));
    for view in &mut app.views {
        rewrite(&mut view.body, &known);
    }
}

/// Instance-method names String answers, from the analyzer's registry.
/// Empty when String isn't registered, which makes the predicate arm
/// inert rather than wrong — no rewrite is better than one that eats a
/// real method.
fn string_methods(registry: &HashMap<ClassId, ClassInfo>) -> Vec<Symbol> {
    registry
        .get(&ClassId(Symbol::from("String")))
        .map(|info| info.instance_methods.keys().cloned().collect())
        .unwrap_or_default()
}

fn rewrite(expr: &mut Expr, known: &[Symbol]) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, known));

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
    if label.is_empty() || known.iter().any(|m| m.as_str() == method.as_str()) {
        return;
    }
    if !matches!(recv.ty, Some(Ty::Str)) {
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
