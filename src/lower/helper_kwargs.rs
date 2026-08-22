//! A helper's NAMED keyword argument, passed as a keyword, bound to the
//! wrong thing.
//!
//! `ingest::library_class` lowers an optional keyword parameter to a
//! positional-with-default — deliberately, because the trailing-kwargs
//! normalize path depends on that shape. The call sites were never
//! moved to match:
//!
//! ```text
//! def self.room_display_name(room, for_user = Current.user)   # def
//! RoomsHelper.room_display_name(message.room, for_user: nil)  # call
//! ```
//!
//! Ruby binds the trailing `{for_user: nil}` HASH to the positional
//! `for_user`. campfire's sidebar then reaches
//! `room.users.without(for_user)`, `excluding` hands that Hash to
//! `where.not`, and `hash_conditions` takes its nested-table branch —
//! emitting `NOT (id.for_user IS NULL)`, SQL naming a column that does
//! not exist.
//!
//! WHY THE EXISTING CHECK MISSES IT. `analyze::body::send::
//! normalize_trailing_kwargs` flips a call's trailing hash to a plain
//! argument only when the callee's last positional is TYPED `Hash`.
//! That is right for the `**attributes` helpers — `link_to_room(room,
//! attributes = {})` called with `id:`/`class:` genuinely wants one
//! hash — and silent for a named keyword, whose type is whatever the
//! parameter is. It also cannot fix this one: it takes `&mut [Expr]`, a
//! slice, and the repair changes the argument COUNT.
//!
//! THE RULE IS NAMES, NOT TYPES. If every trailing-kwarg key matches a
//! positional PARAMETER NAME of the callee, the caller meant those
//! parameters, so the values are spliced in positionally. `id:` and
//! `class:` do not match `attributes`, so the splat helpers are
//! untouched by construction — no marker on the parameter is needed,
//! and none survives ingest anyway.
//!
//! Silent where it does not crash: a keyword whose value happens to be
//! usable as a hash would simply produce wrong results. That is why
//! this is a correctness pass rather than a convenience.

use std::collections::HashMap;

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub fn apply_helper_kwarg_positional_lowering(app: &mut App) {
    apply_to_test_modules(app);
    let params = helper_param_names(app);
    if params.is_empty() {
        return;
    }
    let mut rewrite = |e: &mut Expr| rewrite_calls(e, &params);
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite_calls(&mut view.body, &params);
    }
}

/// The same repair inside a TEST CLASS.
///
/// Ingest lowers a keyword parameter the same way wherever it is
/// declared, so a test's own private helper has the identical break:
///
/// ```text
/// def stub_successful_request(url: "https://www.example.com/")   # def
/// stub_successful_request(url: "https://fxtwitter.com/…")        # call
/// ```
///
/// campfire's `unfurl_links_controller_test` then hands WebMock the
/// HASH and gets "URI should be a String … Got: Hash" — the same shape
/// that emitted `NOT (id.for_user IS NULL)` from a view helper.
///
/// Resolved PER TEST CLASS, not through `helper_method_index`: the
/// callee is the class's own method, so there is no cross-module
/// ambiguity to rule out and no reason to require a globally unique
/// name. Two helpers in ONE class with the same name are still skipped,
/// because then the call site genuinely does not say which.
fn apply_to_test_modules(app: &mut App) {
    for tm in &mut app.test_modules {
        let mut params: HashMap<Symbol, Vec<Symbol>> = HashMap::new();
        let mut ambiguous: Vec<Symbol> = Vec::new();
        for m in &tm.helpers {
            // Same two exclusions as the module path: a `rest` parameter
            // ends the simple positional story, and one that SURVIVED as
            // a keyword is already bound correctly.
            if m.params.iter().any(|p| p.rest || p.keyword) {
                continue;
            }
            let names: Vec<Symbol> = m.params.iter().map(|p| p.name.clone()).collect();
            if params.insert(m.name.clone(), names).is_some() {
                ambiguous.push(m.name.clone());
            }
        }
        for name in ambiguous {
            params.remove(&name);
        }
        if params.is_empty() {
            continue;
        }
        if let Some(setup) = &mut tm.setup {
            rewrite_calls(setup, &params);
        }
        for t in &mut tm.tests {
            rewrite_calls(&mut t.body, &params);
        }
        for m in &mut tm.helpers {
            rewrite_calls(&mut m.body, &params);
        }
    }
}

/// Helper name → its parameter names, in declaration order.
///
/// Only helpers whose name is UNIQUE across modules are registered: a
/// name two modules define is one this pass cannot resolve from the
/// call site alone, and binding it to the wrong signature is exactly
/// the failure being fixed.
fn helper_param_names(app: &App) -> HashMap<Symbol, Vec<Symbol>> {
    let mut out: HashMap<Symbol, Vec<Symbol>> = HashMap::new();
    let mut ambiguous: Vec<Symbol> = Vec::new();
    for (name, owner) in &app.helper_method_index {
        let Some(lc) = app.library_classes.iter().find(|c| &c.name == owner) else {
            continue;
        };
        let Some(m) = lc.methods.iter().find(|m| &m.name == name) else {
            continue;
        };
        // A `rest` parameter and the positional story stops being a
        // simple index. A param that SURVIVED as a keyword
        // (`keeps_keywords` in ingest) is already bound correctly at the
        // call site and must not be moved — a signature mixing the two
        // is the one case where the name rule cannot tell which is
        // which, so the whole helper is skipped.
        if m.params.iter().any(|p| p.rest || p.keyword) {
            continue;
        }
        let names: Vec<Symbol> = m.params.iter().map(|p| p.name.clone()).collect();
        if out.insert(name.clone(), names).is_some() {
            ambiguous.push(name.clone());
        }
    }
    for name in ambiguous {
        out.remove(&name);
    }
    out
}

fn rewrite_calls(e: &mut Expr, params: &HashMap<Symbol, Vec<Symbol>>) {
    e.node.for_each_child_mut(&mut |c| rewrite_calls(c, params));
    let ExprNode::Send { recv, method, args, .. } = &mut *e.node else { return };
    // Receiverless only: the bare spelling a view writes, before
    // `rewrite_helper_calls` prefixes the module at emit time.
    if recv.is_some() {
        return;
    }
    let Some(names) = params.get(method) else { return };
    let Some(last) = args.last() else { return };
    let ExprNode::Hash { entries, kwargs: true } = &*last.node else { return };
    if entries.is_empty() {
        return;
    }
    // Every key must be a Symbol naming a parameter, and the parameters
    // it names must be exactly the ones this call has not already filled
    // positionally. Anything else and the call means something this pass
    // cannot prove, so it is left alone.
    let filled = args.len() - 1;
    let mut supplied: Vec<(usize, Expr)> = Vec::new();
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else { return };
        let Some(pos) = names.iter().position(|n| n == value) else { return };
        if pos < filled {
            return;
        }
        supplied.push((pos, v.clone()));
    }
    supplied.sort_by_key(|(pos, _)| *pos);
    // Contiguous from the first unfilled slot: a gap would need the
    // parameter's own default in between, which is not something to
    // invent here.
    if supplied
        .iter()
        .enumerate()
        .any(|(i, (pos, _))| *pos != filled + i)
    {
        return;
    }
    args.pop();
    args.extend(supplied.into_iter().map(|(_, v)| v));
}
