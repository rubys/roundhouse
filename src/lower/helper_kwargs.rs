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
//!
//! A GAP IS FILLED WITH THE PARAMETER'S OWN DEFAULT. Keywords are
//! unordered, so a caller may name the second and third and leave the
//! first to its default — campfire's `stub_successful_request(title:,
//! description:)` beside a `def stub_successful_request(url: "…",
//! title: "…", description: "…")`. The slot in between is filled with
//! the default the definition wrote, which is not an invention: it is
//! what Ruby binds. Only a CONTEXT-FREE default is moved — a literal, a
//! constant, an array or hash of those — because the default is now
//! evaluated at the call site rather than in the callee; one that reads
//! an earlier parameter (`def f(a, b = a)`) or the callee's `self` means
//! something different there, and such a call is left alone.

use std::collections::HashMap;

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub fn apply_helper_kwarg_positional_lowering(app: &mut App) {
    apply_to_test_modules(app);
    apply_to_library_class_calls(app);
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

/// The same repair for a library class's CLASS method called through
/// its constant — lobsters' `Routes.title_path(story, anchor: a)`
/// against `def title_path story, anchor: nil` (in `class << self`),
/// which ingest flattened to `title_path(story, anchor = nil)`. The
/// trailing `{anchor: a}` bound to `anchor` whole, and the redirect
/// for a merged story rendered its fragment as `#{anchor: "…"}`.
///
/// Narrower than the helper rule on purpose: only slots ingest marked
/// `from_keyword` may be named. A class method's genuine optional
/// positional (`def f(x, opts = {})`) called with `f(x, opts: 1)` is
/// handed the Hash `{opts: 1}` by Ruby, and must keep it.
fn apply_to_library_class_calls(app: &mut App) {
    let mut params: HashMap<(String, Symbol), Vec<Slot>> = HashMap::new();
    for lc in &app.library_classes {
        for m in &lc.methods {
            if m.receiver != crate::dialect::MethodReceiver::Class
                || m.params.iter().any(|p| p.rest || p.keyword)
                || !m.params.iter().any(|p| p.from_keyword)
            {
                continue;
            }
            params.insert(
                (lc.name.0.as_str().to_string(), m.name.clone()),
                m.params.iter().map(slot_of).collect(),
            );
        }
    }
    if params.is_empty() {
        return;
    }
    let mut rewrite = |e: &mut Expr| rewrite_class_calls(e, &params);
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite_class_calls(&mut view.body, &params);
    }
}

fn rewrite_class_calls(e: &mut Expr, params: &HashMap<(String, Symbol), Vec<Slot>>) {
    e.node.for_each_child_mut(&mut |c| rewrite_class_calls(c, params));
    let ExprNode::Send { recv: Some(recv), method, args, .. } = &mut *e.node else { return };
    let ExprNode::Const { path } = &*recv.node else { return };
    let class = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
    let Some(slots) = params.get(&(class, method.clone())) else { return };
    respell(args, slots, true);
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
        let mut params: HashMap<Symbol, Vec<Slot>> = HashMap::new();
        let mut ambiguous: Vec<Symbol> = Vec::new();
        for m in &tm.helpers {
            // Same two exclusions as the module path: a `rest` parameter
            // ends the simple positional story, and one that SURVIVED as
            // a keyword is already bound correctly.
            if m.params.iter().any(|p| p.rest || p.keyword) {
                continue;
            }
            let names: Vec<Slot> = m.params.iter().map(slot_of).collect();
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

/// One positional slot of a helper: its name, and its default when it
/// has one.
struct Slot {
    name: Symbol,
    default: Option<Expr>,
    from_keyword: bool,
}

fn slot_of(p: &crate::dialect::Param) -> Slot {
    Slot { name: p.name.clone(), default: p.default.clone(), from_keyword: p.from_keyword }
}

/// Helper name → its parameter slots, in declaration order.
///
/// Only helpers whose name is UNIQUE across modules are registered: a
/// name two modules define is one this pass cannot resolve from the
/// call site alone, and binding it to the wrong signature is exactly
/// the failure being fixed.
fn helper_param_names(app: &App) -> HashMap<Symbol, Vec<Slot>> {
    let mut out: HashMap<Symbol, Vec<Slot>> = HashMap::new();
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
        let names: Vec<Slot> = m.params.iter().map(slot_of).collect();
        if out.insert(name.clone(), names).is_some() {
            ambiguous.push(name.clone());
        }
    }
    for name in ambiguous {
        out.remove(&name);
    }
    out
}

fn rewrite_calls(e: &mut Expr, params: &HashMap<Symbol, Vec<Slot>>) {
    e.node.for_each_child_mut(&mut |c| rewrite_calls(c, params));
    let ExprNode::Send { recv, method, args, .. } = &mut *e.node else { return };
    // Receiverless only: the bare spelling a view writes, before
    // `rewrite_helper_calls` prefixes the module at emit time.
    if recv.is_some() {
        return;
    }
    let Some(slots) = params.get(method) else { return };
    respell(args, slots, false);
}

/// Move a call's trailing keywords into the positional slots they
/// name. `keyword_slots_only` limits the names to slots ingest
/// flattened from keywords.
fn respell(args: &mut Vec<Expr>, slots: &[Slot], keyword_slots_only: bool) {
    let Some(last) = args.last() else { return };
    let ExprNode::Hash { entries, kwargs: true } = &*last.node else { return };
    if entries.is_empty() {
        return;
    }
    // Every key must be a Symbol naming a parameter this call has not
    // already filled positionally. Anything else and the call means
    // something this pass cannot prove, so it is left alone.
    let filled = args.len() - 1;
    let mut supplied: Vec<(usize, Expr)> = Vec::new();
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else { return };
        let Some(pos) = slots.iter().position(|s| s.name == *value) else { return };
        if keyword_slots_only && !slots[pos].from_keyword {
            return;
        }
        if pos < filled {
            return;
        }
        supplied.push((pos, v.clone()));
    }
    supplied.sort_by_key(|(pos, _)| *pos);
    // From the first unfilled slot up to the last one named: a slot the
    // call skipped takes the parameter's own default — the value Ruby
    // binds — when that default means the same thing at the call site.
    // A required parameter, or a default that reads the callee's
    // context, is a gap this pass cannot fill.
    let last_pos = supplied.last().map(|(pos, _)| *pos).unwrap_or(filled);
    let mut moved: Vec<Expr> = Vec::new();
    let mut supplied = supplied.into_iter().peekable();
    for pos in filled..=last_pos {
        if let Some((p, _)) = supplied.peek() {
            if *p == pos {
                moved.push(supplied.next().map(|(_, v)| v).expect("peeked"));
                continue;
            }
        }
        let Some(default) = slots[pos].default.as_ref() else { return };
        if !is_context_free(default) {
            return;
        }
        moved.push(default.clone());
    }
    args.pop();
    args.extend(moved);
}

/// An expression that evaluates to the same thing wherever it is
/// written: a literal, a constant, or an array or hash of those.
fn is_context_free(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Lit { .. } | ExprNode::Const { .. } => true,
        ExprNode::Array { elements, .. } => elements.iter().all(is_context_free),
        ExprNode::Hash { entries, .. } => entries.iter().all(|(k, v)| is_context_free(k) && is_context_free(v)),
        _ => false,
    }
}
