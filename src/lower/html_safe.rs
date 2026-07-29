//! `<e>.html_safe` — the value-level safety mark, answered statically.
//!
//! Rails marks a String html-safe so ERB won't escape it on the way out.
//! There is no safe-string type here: `html_safe` is not a method any
//! target's String answers, and giving it one means subclassing the
//! built-in String — which the CRuby overlay does (`SafeString`) and the
//! shared runtime deliberately cannot. Under spinel the call is simply a
//! `NoMethodError`, and it took down `/u/:username`, whose page renders
//! `hat.to_html_label` — a lobsters model method whose body ends in
//! `h.html_safe`.
//!
//! Escaping in the emitted views is decided POSITIONALLY at lowering
//! time (the view walker wraps `<%= expr %>` in `html_escape`), so what
//! the mark actually carries is one fact: *do not escape the result of
//! this method*. That fact belongs in the IR rather than in a runtime
//! type, so this pass records it and erases the call:
//!
//! * every method whose body can return `<e>.html_safe` is recorded in
//!   `App::html_safe_methods`, which the view walker consults before it
//!   wraps a call in `html_escape`;
//! * `<e>.html_safe` itself becomes `<e>`.
//!
//! Marking the whole METHOD when only one branch carries the mark is a
//! deliberate simplification. Lobsters' `Comment#score_display` returns
//! a score, `"~"`, or `"&nbsp;".html_safe`; the two unmarked branches
//! are a number and a tilde, so not escaping them renders identically.
//! A method mixing marked markup with genuinely escapable user text
//! would need per-branch tracking, and no corpus method does.
//!
//! Views and CONTROLLERS are deliberately left alone, for the same
//! reason in two places: a mark written at a call site is answered by
//! whoever lowers that call site, and erasing it first turns the answer
//! into its opposite. `<%= x.html_safe %>` belongs to the view walker.
//! `render html: content.html_safe` belongs to
//! `controller_to_library::rewrites`, which peels the mark to decide
//! whether the body needs `html_escape` — running ahead of it made `/u`
//! serve its whole cached page as escaped text (`&lt;div class=…`),
//! which is how this boundary was found.
//!
//! A controller `.html_safe` that is NOT a `render html:` argument
//! therefore still reaches the target and still raises there. None
//! exists in the corpus, and the fix when one shows up is to teach the
//! consuming lowering, not to widen the erasure.

use crate::app::App;
use crate::dialect::ModelBodyItem;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_html_safe_lowering(app: &mut App) {
    // Collect before rewriting: the marks are what is about to be erased.
    let mut safe: Vec<Symbol> = Vec::new();
    for model in &app.models {
        for item in &model.body {
            if let ModelBodyItem::Method { method, .. } = item {
                if returns_html_safe(&method.body) {
                    safe.push(method.name.clone());
                }
            }
        }
    }
    for lc in &app.library_classes {
        for m in &lc.methods {
            if returns_html_safe(&m.body) {
                safe.push(m.name.clone());
            }
        }
    }
    app.html_safe_methods.extend(safe);
    for model in &mut app.models {
        for item in &mut model.body {
            match item {
                ModelBodyItem::Method { method, .. } => rewrite(&mut method.body),
                ModelBodyItem::Scope { scope, .. } => rewrite(&mut scope.body),
                ModelBodyItem::Unknown { expr, .. } => rewrite(expr),
                _ => {}
            }
        }
    }
    for lc in &mut app.library_classes {
        for m in &mut lc.methods {
            rewrite(&mut m.body);
        }
    }
}

/// Can this body return a marked value? Every tail position is a
/// candidate — `if`/`case` branches included, since any of them can be
/// the value the caller sees.
fn returns_html_safe(body: &Expr) -> bool {
    match &*body.node {
        ExprNode::Send { method, args, block: None, recv: Some(_), .. } => {
            method.as_str() == "html_safe" && args.is_empty()
        }
        ExprNode::Seq { exprs } => exprs.last().map(returns_html_safe).unwrap_or(false),
        ExprNode::Let { body, .. } => returns_html_safe(body),
        ExprNode::If { then_branch, else_branch, .. } => {
            returns_html_safe(then_branch) || returns_html_safe(else_branch)
        }
        ExprNode::Case { arms, .. } => arms.iter().any(|a| returns_html_safe(&a.body)),
        _ => false,
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv: Some(_), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "html_safe" || !args.is_empty() {
        return;
    }
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { recv, .. } = node else { unreachable!() };
    let inner = recv.unwrap();
    // Keep the outer node's own type where the mark carried one: the
    // receiver is a String either way, and a `None` ty here would make
    // the type-gated passes downstream give up on a value they could
    // read before.
    let ty = expr.ty.clone().or_else(|| inner.ty.clone());
    *expr.node = *inner.node;
    expr.ty = ty;
}
