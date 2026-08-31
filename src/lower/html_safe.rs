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
//!
//! ## The Action Text filter-chain exemption
//!
//! The same fact arrives from one more place, without anyone writing
//! `.html_safe`: campfire's `ActionText::Content::Filter.apply` wraps
//! every filter's product back into `ActionText::Content.new(...)`, and
//! an `ActionText::Content`'s `to_s` is born html-safe in Rails — which
//! is the only reason `MessagesHelper#message_presentation`'s
//!
//! ```text
//! auto_link h(ContentFilters::TextMessagePresentationFilters.apply(...))
//! ```
//!
//! renders markup instead of escaped text. Neither lane here carries
//! that mark at runtime (the shared runtime has no safe-buffer type by
//! design, and the overlay's gem sanitizer answers a plain String), so
//! `h` escaped the sanitized body and every HTML message rendered as
//! its own source — found in a browser the day the safe-list sanitizer
//! stopped raising.
//!
//! The type is not recoverable by inference — the chain runs through a
//! `*splat` of class objects and a `reduce`, so `Filters#apply` types
//! `untyped` — but the CONSTRUCTION is statically visible: a constant
//! initialized `ActionText::Content::Filters.new(...)` is a filter
//! chain, and `h(<that constant>.apply(...))` is Rails-defined to pass
//! the markup through. So this pass collects those constants and
//! rewrites the call to `<chain>.apply(...).to_s` — same escape
//! exemption, decided at the call-site rewrite layer where every other
//! exemption in this runtime is decided.

use crate::app::App;
use crate::dialect::ModelBodyItem;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_html_safe_lowering(app: &mut App) {
    apply_filter_chain_h_exemption(app);
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

/// See "The Action Text filter-chain exemption" in the module note:
/// `h(<chain>.apply(x))` / `html_escape(<chain>.apply(x))` becomes
/// `<chain>.apply(x).to_s` for every constant the app initialized as
/// `ActionText::Content::Filters.new(...)`.
fn apply_filter_chain_h_exemption(app: &mut App) {
    // The chains, as owner-qualified constant paths — campfire's
    // `module ContentFilters; TextMessagePresentationFilters = ...`
    // yields ["ContentFilters", "TextMessagePresentationFilters"].
    let mut chains: Vec<Vec<Symbol>> = Vec::new();
    let mut collect = |owner: &crate::ident::ClassId, constants: &[(Symbol, Expr)]| {
        for (name, init) in constants {
            let ExprNode::Send { method, recv: Some(recv), block: None, .. } = &*init.node else {
                continue;
            };
            if method.as_str() != "new" {
                continue;
            }
            let ExprNode::Const { path } = &*recv.node else { continue };
            if !path_is(path, &["ActionText", "Content", "Filters"]) {
                continue;
            }
            let mut qualified: Vec<Symbol> =
                owner.0.as_str().split("::").map(Symbol::from).collect();
            qualified.push(name.clone());
            chains.push(qualified);
        }
    };
    for lc in &app.library_classes {
        collect(&lc.name, &lc.constants);
    }
    if chains.is_empty() {
        return;
    }
    for model in &mut app.models {
        for item in &mut model.body {
            match item {
                ModelBodyItem::Method { method, .. } => unescape_chain_h(&mut method.body, &chains),
                ModelBodyItem::Scope { scope, .. } => unescape_chain_h(&mut scope.body, &chains),
                ModelBodyItem::Unknown { expr, .. } => unescape_chain_h(expr, &chains),
                _ => {}
            }
        }
    }
    for lc in &mut app.library_classes {
        for m in &mut lc.methods {
            unescape_chain_h(&mut m.body, &chains);
        }
    }
}

fn path_is(path: &[Symbol], want: &[&str]) -> bool {
    path.len() == want.len() && path.iter().zip(want).all(|(s, w)| s.as_str() == *w)
}

/// Does the constant path written at a call site name one of the
/// chains? Exact match, or the written path is a trailing segment of
/// the qualified one — how a reference inside the owning module spells
/// it (`TextMessagePresentationFilters` from inside `ContentFilters`).
fn names_chain(written: &[Symbol], chains: &[Vec<Symbol>]) -> bool {
    chains.iter().any(|q| {
        written.len() <= q.len()
            && q[q.len() - written.len()..]
                .iter()
                .zip(written)
                .all(|(a, b)| a.as_str() == b.as_str())
    })
}

fn unescape_chain_h(expr: &mut Expr, chains: &[Vec<Symbol>]) {
    expr.node.for_each_child_mut(&mut |e| unescape_chain_h(e, chains));
    {
        let ExprNode::Send { recv, method, args, block: None, .. } = &*expr.node else {
            return;
        };
        let name = method.as_str();
        if name != "h" && name != "html_escape" {
            return;
        }
        if args.len() != 1 {
            return;
        }
        // Bare `h(...)`, or spelled through the helper module.
        if let Some(r) = recv {
            let ExprNode::Const { path } = &*r.node else { return };
            if path.last().map(|s| s.as_str()) != Some("ViewHelpers") {
                return;
            }
        }
        let ExprNode::Send { method: inner_m, recv: Some(inner_r), block: None, .. } =
            &*args[0].node
        else {
            return;
        };
        if inner_m.as_str() != "apply" {
            return;
        }
        let ExprNode::Const { path } = &*inner_r.node else { return };
        if !names_chain(path, chains) {
            return;
        }
    }
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { args, .. } = node else { unreachable!() };
    let inner = args.into_iter().next().unwrap();
    let span = expr.span;
    *expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(inner),
            method: Symbol::from("to_s"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
}
