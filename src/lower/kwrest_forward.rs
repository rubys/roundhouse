//! A forwarded `**` bundle landed in an OPTIONAL KEYWORD's slot instead
//! of the `**rest` slot it was aimed at.
//!
//! ```ruby
//! def local_datetime_tag(datetime, style: :time, **attributes)   # def
//! def message_timestamp(message, **attributes)
//!   local_datetime_tag message.created_at, **attributes          # call
//! ```
//!
//! `ingest::library_class` flattens BOTH keyword forms to positionals —
//! `style:` to a positional-with-default, `**attributes` to a trailing
//! positional defaulting to `{}` — and `ingest::expr` erases the call's
//! `**` into the `merge` chain it is defined to be, which is likewise
//! positional. Each half is individually sound. Together they slide the
//! bundle one slot left:
//!
//! ```ruby
//! def self.local_datetime_tag(datetime, style = :time, attributes = {})
//! TimeHelper.local_datetime_tag(message.created_at, attributes)
//! ```
//!
//! so `style` binds the whole hash and `attributes` binds `{}`. campfire
//! renders `message_timestamp(message, class: "message__timestamp")` as
//!
//! ```html
//! <time datetime="…" data-local-time-target="{class: &quot;message__timestamp&quot;}">
//! ```
//!
//! — the `class` dropped and the `data` attribute carrying an inspected
//! Hash. `lower::kwsplat` states the opposite of this in passing
//! ("`**rest` needs no special case … exactly the shape the desugar is
//! already correct for"); that holds only while the `**rest` slot is the
//! FIRST unfilled one, which an optional keyword beside it is precisely
//! what breaks.
//!
//! ## Why an argument in that slot can only be an erased `**`
//!
//! The same argument `kwsplat` makes from arity, made from position
//! instead. `style:` is a KEYWORD in the source: Ruby offers no way to
//! fill it positionally, so a positional argument sitting there cannot
//! have been written by the app's author. It is ingest's own flattening
//! showing through, and the only thing it can have been is the `**`.
//!
//! That inference needs a fact the flattening destroys, so ingest now
//! records it: [`Param::from_keyword`] and [`Param::from_kwrest`] mark
//! the two flattened shapes. Both are inert in emit — the emitted
//! parameter list is unchanged — and read only here.
//!
//! ## The repair
//!
//! Move the bundle to the `**rest` slot and fill the keyword slots it
//! skipped with the DEFAULTS the callee declared, which is what Ruby
//! does for a keyword the bundle does not carry:
//!
//! ```ruby
//! TimeHelper.local_datetime_tag(message.created_at, :time, attributes)
//! ```
//!
//! ## The guards
//!
//! - **Every skipped slot must be `from_keyword`.** A genuine optional
//!   positional (`def f(a, b = 1, **o)`) CAN be filled positionally in
//!   Ruby, so an argument there says nothing and is left alone.
//! - **Each default must be a literal.** It is re-evaluated in the
//!   CALLER's scope, where `Current.user` or `Time.now` is a different
//!   value than the callee would have computed.
//! - **The bundle must be a bare local, ivar or constant read.** Narrows
//!   to the forwarding shape and keeps this pass off anything an earlier
//!   pass placed: `helper_kwargs` splices literal keywords into these
//!   same slots, which is why it runs AFTER this one.
//! - **The trailing argument must not already be `Hash { kwargs: true }`**
//!   — a literal keyword list, which is `helper_kwargs`' business.
//!
//! The two guards about the ARGUMENT — a non-literal default and a
//! bundle that is not a bare read — LEDGER, on the same reasoning as
//! `kwsplat`: the call really is aimed at the `**rest` slot and really
//! does miss it, so leaving it alone means it still renders the wrong
//! thing. The two about the SLOT return silently, because they are not
//! gaps: a genuine optional positional is one the caller may legally
//! fill, and a literal keyword list already binds correctly through
//! `helper_kwargs`.
//!
//! ## Known divergence
//!
//! A bundle that actually CARRIES one of the named keywords —
//! `message_timestamp(message, style: :date)` — keeps it in the rest
//! hash instead of binding the parameter, so the callee sees the default
//! and the key renders as a stray attribute. Ruby distributes by key at
//! call time; nothing static knows the keys of a hash forwarded through
//! a method boundary. The pre-existing behaviour is wrong for EVERY
//! bundle, this one for the subset that names a keyword, and campfire's
//! two call sites are both in the correct subset.

use std::collections::HashMap;

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::Param;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_kwrest_forward_lowering(app: &mut App) -> Vec<Diagnostic> {
    let sigs = helper_signatures(app);
    let mut diags = Vec::new();
    if sigs.is_empty() {
        return diags;
    }
    super::for_each_hook_body(app, &mut |body| rewrite(body, &sigs, &mut diags));
    for view in &mut app.views {
        rewrite(&mut view.body, &sigs, &mut diags);
    }
    diags
}

/// Helper name → its parameter list, for the helpers this pass can act
/// on at all.
///
/// Resolved through `helper_method_index`, which is the SAME index emit
/// uses to rewrite a bare `tagged(…)` into `<Module>.tagged(…)`. That
/// makes the correspondence exact rather than merely likely: whatever
/// module the emitted call dispatches to is the module whose signature
/// was consulted here. The index is keyed by name and documents itself
/// as last-writer-wins on a collision, mirroring Rails include order, so
/// there is no ambiguity left for this pass to re-adjudicate — and an
/// ambiguity guard copied from `helper_kwargs::helper_param_names` would
/// be dead code, since a `HashMap`'s keys are already unique.
///
/// Full `Param`s rather than names: this pass needs the flattening marks
/// and the declared defaults, not just the arity.
fn helper_signatures(app: &App) -> HashMap<Symbol, Vec<Param>> {
    let mut out: HashMap<Symbol, Vec<Param>> = HashMap::new();
    for (name, owner) in &app.helper_method_index {
        let Some(lc) = app.library_classes.iter().find(|c| &c.name == owner) else {
            continue;
        };
        let Some(m) = lc.methods.iter().find(|m| &m.name == name) else {
            continue;
        };
        // A `*rest` absorbs any number of positionals, so no slot has a
        // fixed index. A param that SURVIVED as a keyword is bound by
        // name at the call site and was never flattened.
        if m.params.iter().any(|p| p.rest || p.keyword) {
            continue;
        }
        // Nothing to repair unless the last slot is a flattened `**rest`
        // — that is the destination the bundle missed.
        if !m.params.last().is_some_and(|p| p.from_kwrest) {
            continue;
        }
        out.insert(name.clone(), m.params.clone());
    }
    out
}

fn rewrite(e: &mut Expr, sigs: &HashMap<Symbol, Vec<Param>>, diags: &mut Vec<Diagnostic>) {
    e.node
        .for_each_child_mut(&mut |c| rewrite(c, sigs, diags));
    let ExprNode::Send { recv, method, args, .. } = &mut *e.node else {
        return;
    };
    // Receiverless only: the bare spelling a helper body writes, before
    // emit prefixes the module.
    if recv.is_some() {
        return;
    }
    let Some(params) = sigs.get(method) else { return };

    // The bundle sits in the last supplied slot; the `**rest` slot is
    // the last parameter. Equal means it already landed correctly.
    let filled = args.len();
    let rest_slot = params.len() - 1;
    if filled == 0 || filled > rest_slot {
        return;
    }
    let bundle_slot = filled - 1;

    let Some(bundle) = args.last() else { return };
    // A literal keyword list renders correctly as-is and belongs to
    // `helper_kwargs`.
    if matches!(&*bundle.node, ExprNode::Hash { kwargs: true, .. }) {
        return;
    }
    // Only the slots BETWEEN the bundle and its destination matter; a
    // genuine optional positional among them means the argument could
    // have been meant for it.
    if !params[bundle_slot..rest_slot].iter().all(|p| p.from_keyword) {
        return;
    }
    if !is_pure_read(bundle) {
        diags.push(residue(
            bundle,
            "forwarded-bundle-not-a-read",
            format!(
                "`{method}` receives a keyword bundle in `{}`'s slot, but the argument is not a \
                 local, ivar or constant read, so it cannot be identified as a forwarded `**`",
                params[bundle_slot].name,
            ),
        ));
        return;
    }
    // Re-evaluated in the CALLER's scope, so only a literal is safe.
    let Some(defaults) = params[bundle_slot..rest_slot]
        .iter()
        .map(|p| p.default.clone().filter(is_literal))
        .collect::<Option<Vec<Expr>>>()
    else {
        let names: Vec<&str> = params[bundle_slot..rest_slot]
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        diags.push(residue(
            bundle,
            "keyword-default-not-a-literal",
            format!(
                "`{method}` forwards a keyword bundle past `{}`, whose declared default is not a \
                 literal and would be re-evaluated in the caller's scope",
                names.join("`, `"),
            ),
        ));
        return;
    };

    let bundle = args.pop().expect("checked above");
    args.extend(defaults);
    args.push(bundle);
}

/// Narrow enough that the argument can only be the forwarded hash — and
/// not something `helper_kwargs` or `kwsplat` computed into this slot.
fn is_pure_read(expr: &Expr) -> bool {
    matches!(
        &*expr.node,
        ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::Const { .. }
    )
}

/// A default safe to re-evaluate at the call site. Deliberately narrower
/// than `is_pure_read`, which asks a different question of a different
/// expression: an ivar or local is not even in scope in the caller, and
/// a constant may be nested in the CALLEE's module and resolve to
/// something else (or nothing) where the call site sits. A literal
/// means the same thing everywhere, and the defaults apps actually
/// write here are symbols, strings, numbers, `nil` and empty
/// collections.
fn is_literal(expr: &Expr) -> bool {
    match &*expr.node {
        ExprNode::Lit { .. } => true,
        ExprNode::Hash { entries, .. } => entries.is_empty(),
        ExprNode::Array { elements, .. } => elements.is_empty(),
        _ => false,
    }
}

fn residue(expr: &Expr, reason: &str, message: String) -> Diagnostic {
    super::residue_diagnostic("kwrest_forward", "keyword-bundle forward", expr.span, reason, message)
}
