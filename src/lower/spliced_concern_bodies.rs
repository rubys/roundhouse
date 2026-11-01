//! A controller concern's instance methods, after the splice, are a
//! SECOND copy with no caller — drop them.
//!
//! `ingest::splice_concerns_into_controllers` copies every instance
//! method of an included concern into each including controller (that
//! is how the strict targets, which have no module mixin, get
//! `require_authentication`), and it drops the controller's `include`
//! while doing so. The module itself still emits, and its methods still
//! carry the ORIGINAL bodies: bare `redirect_to`, bare `head`, bare
//! `cookies` — controller-context sends that were only ever going to
//! resolve through an includer.
//!
//! That husk is what stopped campfire's spinel build:
//!
//! ```text
//! app/models/authentication.rb:43: unsupported call:
//!   node 63884 (CallNode `redirect_to`) recv=-/ty-1 argc=1
//! ```
//!
//! and, once that module was stubbed by hand, `authorization.rb:6`'s
//! `head` behind it — one wall per concern, all the way down. Spinel
//! resolves a module's receiverless sends through the classes that
//! include it, which is why the SAME body compiles inside
//! `ApplicationController` and inside `Authentication::SessionLookup`
//! (`ApplicationCable::Connection` includes that one, and supplies
//! `cookies`) but not inside a module nothing includes. The ruby-family
//! lanes were never wrong here — they just never called the copy.
//!
//! The gate is two facts, and BOTH are needed:
//!
//!   * the module handed at least one instance method to a controller
//!     (`App::concern_spliced_actions`) — without this a plain helper
//!     module, which nothing includes either, would lose its body; and
//!   * nothing else in the emit still says `include <module>`, and it is
//!     not an `app/helpers` module (views reach those) — a MODEL
//!     concern keeps its `include User::Bannable` at the model
//!     (`splice_concerns_into_models` leaves it), and campfire's
//!     `Authentication::SessionLookup` is included by
//!     `ApplicationCable::Connection`. Either one means the methods
//!     have a live home and stay.
//!
//! Kept regardless: constants (the splice REWRITES lexical refs to
//! `Module::CONST`, so the module is where they live), class-side
//! methods, `include` directives and replayed class-body calls. Only
//! the instance methods go, and only when both facts hold.
//!
//! The exception is a replayed class-body call that DECORATES a pruned
//! method: `statsd_count_if(:login_from_session, "…")` (and `memoize`,
//! `helper_method`, a visibility change) takes the method's name as its
//! leading symbol
//! and raises `ArgumentError: could not find method login_from_session
//! for SessionLogin` when the module no longer defines it. Those
//! calls go with the method they name.
//!
//! Class-side methods are left alone on purpose. A concern's
//! `class_methods do` block is dead for the same reason — campfire's
//! `Authentication.allow_unauthenticated_access` calls a bare
//! `skip_before_action` — but ingest merges `class_methods do` and a
//! plain `def self.foo` into one `MethodReceiver::Class` list with no
//! provenance, so pruning them would also delete a module function a
//! call site may name (`Foo.bar`). No build has demanded it; when one
//! does, the discriminator is a whole-app scan for `<Module>.<name>`,
//! not a guess here.

use crate::app::App;
use crate::dialect::MethodReceiver;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::det::HashSet;

pub fn apply_spliced_concern_body_prune(app: &mut App) {
    // Modules that handed at least one instance method to a controller.
    // Keyed by module, not by controller: one splice anywhere is what
    // makes the module's own copy the second one.
    let spliced: HashSet<ClassId> = app
        .concern_spliced_actions
        .values()
        .flat_map(|per_method| per_method.values().cloned())
        .collect();
    if spliced.is_empty() {
        return;
    }

    // Every `include <M>` the emit still carries OUTSIDE a controller.
    // Controllers are excluded by construction: their include is what
    // the splice consumed.
    let mut still_included: HashSet<ClassId> = HashSet::default();
    for model in &app.models {
        still_included.extend(crate::analyze::model_includes(model));
    }
    for lc in &app.library_classes {
        still_included.extend(lc.includes.iter().cloned());
    }
    for tm in &app.test_modules {
        still_included.extend(tm.includes.iter().cloned());
        for inner in &tm.inner_classes {
            still_included.extend(inner.includes.iter().cloned());
        }
    }
    // An `app/helpers` module is mixed into every VIEW as well, and the
    // view lowering calls it as `<Helper>.name(…)`. Upstream lobsters'
    // ApplicationController `include ApplicationHelper` spliced it, and
    // the prune left the module empty under ~45 view call sites.
    still_included.extend(app.helper_method_index.values().cloned());

    for lc in &mut app.library_classes {
        if !spliced.contains(&lc.name) || still_included.contains(&lc.name) {
            continue;
        }
        let pruned: HashSet<Symbol> = lc
            .methods
            .iter()
            .filter(|m| matches!(m.receiver, MethodReceiver::Instance))
            .map(|m| m.name.clone())
            .collect();
        lc.methods.retain(|m| !matches!(m.receiver, MethodReceiver::Instance));
        lc.unknown_calls.retain(|call| !decorates(call, &pruned));
    }
}

/// A receiverless class-body call that looks up the method a symbol (or
/// string) argument names, and so raises once that method is gone: a visibility
/// change, `memoize` / `memo_wise`, `helper_method`, `deprecate`, New
/// Relic's `add_method_tracer`, a StatsD instrumentation macro
/// (`statsd_count_if`, `statsd_measure`, …) by its leading symbol, and
/// `alias_method` by either. Any other call that happens to take a pruned
/// name (`register(:ensure_admin, "policy")`, filling a registry) stays.
///
/// The list is knowingly incomplete: a decorator it does not name stays
/// in the husk module and raises its NameError at load, loudly, where
/// the module's own replayed call would.
fn decorates(call: &Expr, names: &HashSet<Symbol>) -> bool {
    let ExprNode::Send { recv: None, method, args, .. } = &*call.node else { return false };
    // Ruby takes a String name wherever it takes a Symbol one
    // (`private "ensure_admin"`, `alias_method "x", "y"`).
    let names_pruned = |a: &Expr| match &*a.node {
        ExprNode::Lit { value: Literal::Sym { value } } => names.contains(value),
        ExprNode::Lit { value: Literal::Str { value } } => names.contains(&Symbol::from(value.as_str())),
        _ => false,
    };
    if method.as_str() == "alias_method" {
        return args.iter().any(names_pruned);
    }
    let decorator = matches!(
        method.as_str(),
        "private"
            | "protected"
            | "public"
            | "module_function"
            | "memoize"
            | "memo_wise"
            | "helper_method"
            | "deprecate"
            | "add_method_tracer"
    ) || method.as_str().starts_with("statsd_");
    decorator && args.first().is_some_and(names_pruned)
}
