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
//!   * nothing else in the emit still says `include <module>` — a MODEL
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
use crate::ident::ClassId;
use std::collections::HashSet;

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
    let mut still_included: HashSet<ClassId> = HashSet::new();
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

    for lc in &mut app.library_classes {
        if !spliced.contains(&lc.name) || still_included.contains(&lc.name) {
            continue;
        }
        lc.methods.retain(|m| !matches!(m.receiver, MethodReceiver::Instance));
    }
}
