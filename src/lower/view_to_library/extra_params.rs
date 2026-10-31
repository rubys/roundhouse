//! Collect bareword free-variable references from a view body so they
//! can surface as additional positional params on the emitted method.
//!
//! Owns the flash-param contract: rename when `@notice`/`@alert` collide
//! with the uniform flash extras (`flash_locals`), rewrite the body, then
//! seed extras from those names (`collect_extra_params`). Callers go
//! through [`rewritten_extras`] so the three sites cannot disagree.

use std::borrow::Cow;
use crate::det::HashSet;

use crate::dialect::View;
use crate::expr::{Expr, ExprNode, InterpPart, LValue};
use crate::ident::Symbol;

use super::{local_assigns_key, rewrite_ivars_to_locals, IvarLocals};

/// Rename flash reads when needed, rewrite `@ivar` → locals, then collect
/// trailing extras. One entry for the def site and both call-site maps so
/// the pipeline order cannot drift.
pub(super) fn rewritten_extras(
    view: &View,
    ivar_locals: &IvarLocals,
    record: &str,
    keys: Option<&[String]>,
    request_context: bool,
) -> (Expr, Vec<String>) {
    let (body, flash) = flash_locals(view, ivar_locals, record, keys);
    let rewritten = rewrite_ivars_to_locals(&body, ivar_locals);
    let extras = collect_extra_params(&rewritten, record, &flash, request_context);
    (rewritten, extras)
}

/// The names `view`'s flash params go by, and its body with its flash
/// reads renamed to match.
///
/// Every template takes the flash pair as trailing params
/// (`collect_extra_params`), and `@notice` reads as the local `notice`,
/// so a view reading both read them through one local: the flash param
/// was dropped while every caller still passed it. The ivar keeps its
/// name, which every site threading it relies on. The flash takes the
/// first of `notice_`, `notice__`, … that no name of the view has: none
/// its body spells or binds, and none it is handed (its `record`, the
/// locals `keys` its callers pass, its `local_assigns` keys).
///
/// A view reading `local_assigns[:notice]` moves its flash the same way,
/// though no ivar takes the name: that read is the render's `notice:`
/// local or nil, and a bare `notice` is that local or else the flash, so
/// the one param could not hold both (a call site binds the moved flash
/// from the `notice:` local too, `local_key_of`).
///
/// Only the flash reads move, a bare `notice` call: a local a block or
/// an assignment binds reads as a `Var` and keeps its name. A view handed
/// a `notice` local reads that, not the flash, so it keeps its names, as
/// does a strict-locals partial, which takes no flash.
pub(super) fn flash_locals<'a>(
    view: &'a View,
    ivar_locals: &IvarLocals,
    record: &str,
    keys: Option<&[String]>,
) -> (Cow<'a, Expr>, [String; 2]) {
    fn names(e: &Expr, out: &mut HashSet<String>, handed: &mut HashSet<String>) {
        match &*e.node {
            ExprNode::Var { name, .. }
            | ExprNode::Assign { target: LValue::Var { name, .. }, .. }
            | ExprNode::OpAssign { target: LValue::Var { name, .. }, .. }
            | ExprNode::Send { recv: None, method: name, .. } => {
                out.insert(name.as_str().to_string());
            }
            ExprNode::Lambda {
                params,
                rest_param,
                extra_params,
                block_param,
                ..
            } => {
                let bound = params.iter().chain(rest_param).chain(block_param);
                out.extend(
                    bound
                        .chain(extra_params.iter().map(|p| &p.name))
                        .map(|p| p.as_str().to_string()),
                );
            }
            _ => {}
        }
        if let Some(k) = local_assigns_key(e) {
            handed.insert(k);
        }
        e.node.for_each_child(&mut |c| names(c, out, handed));
    }
    fn rename(e: &mut Expr, from: &str, to: &Symbol) {
        if let ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } = &mut *e.node
        {
            if args.is_empty() && method.as_str() == from {
                *method = to.clone();
            }
        }
        e.node.for_each_child_mut(&mut |c| rename(c, from, to));
    }
    fn local_assigns_keys(e: &Expr, out: &mut HashSet<String>) {
        if let Some(k) = local_assigns_key(e) {
            out.insert(k);
        }
        e.node.for_each_child(&mut |c| local_assigns_keys(c, out));
    }
    let mut flash = ["notice".to_string(), "alert".to_string()];
    let shadowed = |n: &String| ivar_locals.values().any(|l| l == n);
    let mut read_as_local: HashSet<String> = HashSet::default();
    local_assigns_keys(&view.body, &mut read_as_local);
    let split = |n: &String| n != record && !shadowed(n) && read_as_local.contains(n);
    if view.strict_locals.is_some() || !flash.iter().any(|n| shadowed(n) || split(n)) {
        return (Cow::Borrowed(&view.body), flash);
    }
    let mut handed: HashSet<String> = std::iter::once(record)
        .chain(keys.into_iter().flatten().map(String::as_str))
        .map(str::to_string)
        .collect();
    let mut taken: HashSet<String> = ivar_locals.values().cloned().collect();
    names(&view.body, &mut taken, &mut handed);
    taken.extend(handed.iter().cloned());
    let mut body = Cow::Borrowed(&view.body);
    for n in &mut flash {
        if !split(n) && (!shadowed(n) || handed.contains(n.as_str())) {
            continue;
        }
        let mut local = format!("{n}_");
        while taken.contains(&local) {
            local.push('_');
        }
        rename(body.to_mut(), n, &Symbol::from(local.as_str()));
        taken.insert(local.clone());
        *n = local;
    }
    (body, flash)
}

/// Walk the (already ivar-rewritten) body and collect bareword
/// references — `Send { recv: None, args: [], block: None }` and
/// `Var` reads — whose names are NOT the inferred view arg, NOT
/// `_buf`, and NOT a recognized view helper. Today this catches
/// `notice` / `alert` (Rails flash helpers parsed as bare Sends). They
/// surface as positional params on the emitted method so the body
/// type-checks under spinel-blog's runtime.
///
/// `notice` and `alert` are *always* included in the result (in that
/// order) regardless of body usage, so every view's signature has a
/// uniform shape: `(<main_arg>, notice = nil, alert = nil)`. The
/// controller→view rewrite (`rewrite_render_to_views`) relies on this
/// uniform shape to unconditionally pass `@flash[:notice]` /
/// `@flash[:alert]` at the matching positions — without it, the
/// controller would need view-specific signature knowledge to decide
/// whether to pass each flash slot. Cost is two unused locals per
/// view that doesn't reference flash; emit-side it's negligible. They
/// go by the names `flash` gives them (see `flash_locals`).
fn collect_extra_params(body: &Expr, arg_name: &str, flash: &[String; 2], request_context: bool) -> Vec<String> {
    let mut out: Vec<String> = flash.to_vec();
    // `action_name`/`controller_name` are controller context Rails exposes
    // to views. Unlike flash, they're collected only when the view
    // actually references them (kept right after notice/alert, in a fixed
    // order, so the controller→view rewrite can pass the matching literals
    // at known positions). The controller passes them only to views whose
    // contract records the usage (see action_view_ivar_map) — so views
    // that don't use them gain no extra params on any target.
    // A run-time url Hash resolves against the request's controller and
    // action too, in the view or a partial it renders (`request_context_views`).
    let dynamic_url = request_context;
    if dynamic_url || super::view_uses_bare_name(body, "action_name") {
        out.push("action_name".to_string());
    }
    if dynamic_url || super::view_uses_bare_name(body, "controller_name") {
        out.push("controller_name".to_string());
    }
    // The request's path parameters, for a view whose `url_for` options
    // hash (`{controller:, action:, page:}`) may resolve to a route with
    // a segment the hash leaves out: Rails recalls it from the current
    // request (see `lower_url_option_helpers`). Same rule as the two
    // above — collected only when used, at a fixed position.
    if dynamic_url || super::view_uses_url_options_hash(body) {
        out.push("path_parameters".to_string());
    }
    let mut bound: Vec<String> = Vec::new();
    if !arg_name.is_empty() {
        bound.push(arg_name.to_string());
    }
    bound.push("_buf".to_string());
    bound.push("io".to_string());
    walk_for_extra(body, &bound, &mut out);
    out
}

/// Drop the extras a threaded closure ivar already names. After the
/// ivar→local rewrite `@error` and `local_assigns[:error]` read the same
/// local `error`, which the closure param supplies; a second `error`
/// param is a duplicate argument name (#389). It runs after the `locals:`
/// keys are appended, so a key that names a closure ivar is dropped the
/// same way. The def site and both call-site contracts apply it to the
/// same lists, so positions stay aligned. `closure` holds `safe_local`
/// names; `extras` holds raw ones (`class` against `class_`).
pub(super) fn drop_closure_names(extras: &mut Vec<String>, closure: &[String]) {
    extras.retain(|k| !closure.contains(&crate::naming::safe_local(k)));
}

/// A partial local is bound by its own name, or by its `safe_local`
/// form: the record arg of `as: :for` is `for_`.
fn is_bound(bound: &[String], name: &str) -> bool {
    let safe = crate::naming::safe_local(name);
    bound.iter().any(|b| b == name || *b == safe)
}

fn walk_for_extra(e: &Expr, bound: &[String], out: &mut Vec<String>) {
    match &*e.node {
        ExprNode::Var { name, .. } => {
            let n = name.as_str();
            // A reserved-word local (`binding.local_variable_get(:class)`
            // in the template) has no other source than a caller's
            // locals, so it is a partial local like `local_assigns[:x]`.
            let partial_local = is_flash_name(n) || crate::naming::is_reserved_local(n);
            if !is_bound(bound, n) && !out.iter().any(|x| x == n) && partial_local {
                out.push(n.to_string());
            }
        }
        ExprNode::Send {
            recv,
            method,
            args,
            block,
            ..
        } => {
            if recv.is_none() && args.is_empty() && block.is_none() {
                let n = method.as_str();
                if !bound.iter().any(|b| b == n)
                    && !out.iter().any(|x| x == n)
                    && is_flash_name(n)
                {
                    out.push(n.to_string());
                }
            }
            // `defined?(name)` ingest produces `Send(None, :defined?,
            // [Var(name)])`. The author wrote `defined?(name)`
            // *specifically* to mark `name` as an optional partial-
            // local; collect it as an extra param regardless of
            // is_flash_name. Without this, `defined?(force_open)` in
            // _comment.html.erb would leave `force_open` as a free
            // reference the analyzer can't resolve.
            // `local_assigns[:x]` marks `x` as an optional partial-local
            // just as deliberately as `defined?(x)` does — Rails' two
            // spellings for "a local a caller may not have passed" — so
            // it earns a nil-default param the same way.
            if let Some(n) = super::local_assigns_key(e) {
                if !is_bound(bound, &n) && !out.iter().any(|x| x == &n) {
                    out.push(n);
                }
            }
            if recv.is_none() && method.as_str() == "defined?" && args.len() == 1 {
                if let ExprNode::Var { name, .. } = &*args[0].node {
                    let n = name.as_str();
                    if !is_bound(bound, n) && !out.iter().any(|x| x == n) {
                        out.push(n.to_string());
                    }
                }
            }
            if let Some(r) = recv {
                walk_for_extra(r, bound, out);
            }
            for a in args {
                walk_for_extra(a, bound, out);
            }
            if let Some(b) = block {
                walk_for_extra(b, bound, out);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                walk_for_extra(e, bound, out);
            }
        }
        ExprNode::If {
            cond,
            then_branch,
            else_branch,
        } => {
            walk_for_extra(cond, bound, out);
            walk_for_extra(then_branch, bound, out);
            walk_for_extra(else_branch, bound, out);
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk_for_extra(left, bound, out);
            walk_for_extra(right, bound, out);
        }
        ExprNode::Assign { value, .. } => walk_for_extra(value, bound, out),
        ExprNode::Lambda {
            body,
            params,
            extra_params,
            ..
        } => {
            let mut inner_bound = bound.to_vec();
            for p in params.iter().chain(extra_params.iter().map(|p| &p.name)) {
                inner_bound.push(p.as_str().to_string());
            }
            walk_for_extra(body, &inner_bound, out);
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                walk_for_extra(el, bound, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk_for_extra(k, bound, out);
                walk_for_extra(v, bound, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    walk_for_extra(expr, bound, out);
                }
            }
        }
        _ => {}
    }
}

/// Today's heuristic for "this bareword is a Rails flash helper that
/// should surface as a method parameter." Conservative — any other
/// unknown bareword stays as a free reference and the analyzer / type
/// checker is responsible for diagnosing it. Expand the set as
/// fixtures introduce more flash-style helpers.
fn is_flash_name(n: &str) -> bool {
    matches!(n, "notice" | "alert")
}

/// Whether `name` is the canonical flash slot `base` or a rename of it
/// (`notice_`, `notice__`, …). Used when mapping caller flash params to
/// callee flash slots by position.
pub(crate) fn is_flash_slot_name(name: &str, base: &str) -> bool {
    name == base
        || (name.starts_with(base)
            && name[base.len()..].chars().all(|c| c == '_')
            && !name[base.len()..].is_empty())
}

/// A view's flash params among its `extras`: the `notice` one, then the
/// `alert` one. `collect_extra_params` seeds the pair at the front, so
/// only a leading extra spelling the slot (or its `flash_locals` rename)
/// is the flash; a `local_assigns[:notice_]` the body reads further on
/// is an ordinary local, whatever its name.
pub(crate) fn flash_params(extras: &[String]) -> [Option<&str>; 2] {
    let mut out = [None, None];
    let mut at = 0;
    for (slot, base) in ["notice", "alert"].into_iter().enumerate() {
        if let Some(name) = extras.get(at).filter(|n| is_flash_slot_name(n, base)) {
            out[slot] = Some(name.as_str());
            at += 1;
        }
    }
    out
}

/// The flash slot (`notice` or `alert`) a partial's extra `name` is, if
/// it is one of its flash params (`flash_params`).
pub(crate) fn flash_slot_of(extras: &[String], name: &str) -> Option<&'static str> {
    ["notice", "alert"]
        .into_iter()
        .zip(flash_params(extras))
        .find_map(|(slot, param)| (param == Some(name)).then_some(slot))
}

/// The `locals:` key a render binds the partial's extra `name` from: its
/// own, or the slot's for a flash param (`notice` for `notice_`). A bare
/// `notice` reads the render's `notice:` local before the flash, however
/// `flash_locals` spelled the param it reads.
pub(crate) fn local_key_of<'a>(extras: &[String], name: &'a str) -> &'a str {
    flash_slot_of(extras, name).unwrap_or(name)
}

/// The caller's flash param for slot `base` among `in_scope` names.
/// When both the ivar local (`notice`) and the renamed flash (`notice_`)
/// are present, the longer spelling is the flash.
pub(crate) fn flash_param_in_scope(in_scope: &[Symbol], base: &str) -> Option<String> {
    in_scope
        .iter()
        .map(|p| p.as_str())
        .filter(|n| is_flash_slot_name(n, base))
        .max_by_key(|n| n.len())
        .map(str::to_string)
}
