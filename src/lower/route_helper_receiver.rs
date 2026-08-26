//! A bare `<x>_path` in a LIBRARY CLASS or a VIEW gets its
//! `RouteHelpers.` receiver, the same one controllers and tests get.
//!
//! `controller_to_library`'s `rewrite_route_helpers` has qualified
//! controller and test bodies for a long time, and view lowering
//! qualifies the URL ARGUMENT of `link_to` / `form_with`. Nothing
//! qualified anywhere else, so campfire emitted the same helper spelled
//! two ways in one tree: `RouteHelpers.rails_blob_path(...)` on
//! `_actions.html.erb:33`, where it is `link_to`'s first argument, and a
//! bare `rails_blob_path(...)` five lines later, where it is a `data:`
//! hash value. `app/helpers/messages/attachment_presentation.rb` —
//! `delegate :rails_blob_path, to: :context` in Rails, a library class
//! here — emitted two more. Three calls to a method the emitted tree
//! does not define, and the spinel build stops at the first one.
//!
//! **The rule is "a name `RouteHelpers` ANSWERS", not the `_path`
//! suffix, and reusing `rewrite_route_helpers` to get the suffix rule
//! was measurably wrong.** That was the first attempt and it regressed
//! five sites in one emit, three ways at once:
//!
//! * It folds `_url` onto its `_path` twin. A later pass turns
//!   `new_session_url` into `"http://#{Rails.application.domain}#{…}"`;
//!   folding first left campfire's post-authentication redirect pointing
//!   at a bare path with no host.
//! * The suffix caught `image_path`, `asset_path` and `polymorphic_url`,
//!   which are `ActionView::ViewHelpers`' business — a later pass
//!   qualifies them there, and this one got to them first.
//! * It strips a `Rails.application.routes.url_helpers` receiver up
//!   front and re-adds `RouteHelpers.` after. With a shadow set in play
//!   the second half declines and the first half has already happened:
//!   `Webhook#room_bot_messages_path`, whose body calls the route helper
//!   of the very same name through `url_helpers`, came out bare.
//!
//! So the name set is the app's own route table (the `_path` spellings
//! — `RouteHelpers` emits no `_url` form) plus the engine-mounted
//! helpers the runtime stubs. `rails_blob_path` is why that second half
//! exists: Active Storage is a MOUNTED ENGINE, so it appears in no app
//! route table, and the only thing that answers it is
//! `runtime/ruby/active_storage.rb`.
//!
//! **The SHADOW set is what makes even that safe, and the corpus proves
//! it.** `Webhook` defines `message_path` and `room_bot_messages_path`,
//! colliding head-on with real route helpers of the same name and
//! forwarding to them under different arities;
//! `Messages::AttachmentPresentation` calls `rails_blob_path`, which it
//! does not define, from inside `download_url`, which it does. So the
//! shadow set is per-owner: a class's own methods, its parent's, and its
//! included modules'. A view's is every `_path`/`_url` a helper MODULE
//! defines, which is Rails' view scope.
//!
//! No `.id` projection here, deliberately. `rewrite_route_helpers` does
//! it for controllers off a segment-shape table; no library class in the
//! corpus hands a record to a route helper, and a site that starts to
//! will pass a record where the helper's signature says `Integer` — a
//! type error the ledger reports, which beats a silent guess
//! ([[project_lobsters_story_route_kwargs_poly_receiver]]).
//!
//! **WHERE it runs is a constraint, not a preference.** Every route
//! helper receiver in the emit is added at EMIT time — controllers get
//! theirs in `controller_to_library`, views in `emit_lowered_views` —
//! and that is because `lower_routes_to_library_functions` SURVEYS the
//! app IR for bare `<x>_path` sends first, to decide each generated
//! helper's segment types and query keys. Run this at app-lowering
//! time instead and the survey stops seeing the call: campfire's
//! `qr_code_path` lost the String-typed segment it had been given, and
//! `room_involvement_path` lost its `involvement:` query parameter
//! entirely. So this runs beside `apply_helper_lowering`, which is the
//! same reason that pass runs where it does.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use std::collections::{HashMap, HashSet};

/// Route helpers `RouteHelpers` answers that no app route declares —
/// mounted engines. Kept in step with the `module RouteHelpers` reopen
/// in `runtime/ruby/active_storage.rb`, which is the only place one
/// exists today.
const ENGINE_MOUNTED_HELPERS: &[&str] = &["rails_blob_path"];

fn is_helper_shaped(name: &Symbol) -> bool {
    name.as_str().ends_with("_path") || name.as_str().ends_with("_url")
}

/// Every name a call can use and reach a real `RouteHelpers` method.
pub(crate) fn answered_names(app: &App) -> HashSet<Symbol> {
    let mut out: HashSet<Symbol> = ENGINE_MOUNTED_HELPERS.iter().map(|n| Symbol::from(*n)).collect();
    for route in crate::lower::routes::flatten_routes(app) {
        if !route.named || route.as_name.is_empty() {
            continue;
        }
        out.insert(Symbol::from(format!("{}_path", route.as_name)));
    }
    out
}

/// Add the `RouteHelpers.` receiver to bare calls naming a helper the
/// emitted tree defines. Receiver-bearing sends are left exactly as
/// they are — whatever put a receiver there meant it.
pub(crate) fn qualify(body: &Expr, answered: &HashSet<Symbol>, shadowed: &HashSet<Symbol>) -> Expr {
    crate::lower::controller_to_library::util::map_expr(body, &|e: &Expr| {
        let ExprNode::Send { recv: None, method, args, block, parenthesized } = &*e.node else {
            return None;
        };
        if !answered.contains(&*method) || shadowed.contains(&*method) {
            return None;
        }
        Some(Expr::new(
            e.span,
            ExprNode::Send {
                recv: Some(crate::lower::controller_to_library::rewrites::const_path(
                    &["RouteHelpers"],
                    e.span,
                )),
                method: method.clone(),
                args: args.iter().map(|a| qualify(a, answered, shadowed)).collect(),
                block: block.as_ref().map(|b| qualify(b, answered, shadowed)),
                parenthesized: *parenthesized,
            },
        ))
    })
}

/// Every `_path` / `_url` name defined by a HELPER MODULE — the view
/// scope's shadow set.
pub(crate) fn helper_module_shadows(app: &App) -> HashSet<Symbol> {
    app.library_classes
        .iter()
        .filter(|lc| lc.is_module)
        .flat_map(|lc| lc.methods.iter())
        .map(|m| m.name.clone())
        .filter(is_helper_shaped)
        .collect()
}

/// Run over already-lowered `LibraryClass`es — view modules, models,
/// app helpers — after the pipeline's own bare-helper resolution has had
/// first claim on the name.
pub fn qualify_lcs(lcs: &mut [crate::dialect::LibraryClass], app: &App) {
    let answered = answered_names(app);
    // A helper module's own `<x>_path` outranks a route of the same
    // name everywhere the emit resolves bare helper calls. The modules
    // in `lcs` count as well as the ones on `app`: the view pipeline
    // passes view LCs while the helpers sit on `app`, and the library
    // pipeline passes the helpers themselves.
    let mut helper_shadows = helper_module_shadows(app);
    helper_shadows.extend(
        lcs.iter()
            .filter(|lc| lc.is_module)
            .flat_map(|lc| lc.methods.iter())
            .map(|m| m.name.clone())
            .filter(is_helper_shaped),
    );
    let owned: HashMap<ClassId, HashSet<Symbol>> = lcs
        .iter()
        .map(|lc| {
            (
                lc.name.clone(),
                lc.methods.iter().map(|m| m.name.clone()).filter(is_helper_shaped).collect(),
            )
        })
        .collect();
    let shadow_sets: Vec<HashSet<Symbol>> = lcs
        .iter()
        .map(|lc| {
            let mut set = helper_shadows.clone();
            for id in std::iter::once(&lc.name).chain(lc.includes.iter()).chain(lc.parent.iter()) {
                set.extend(owned.get(id).into_iter().flatten().cloned());
            }
            set
        })
        .collect();
    for (lc, shadows) in lcs.iter_mut().zip(shadow_sets.iter()) {
        for m in &mut lc.methods {
            m.body = qualify(&m.body, &answered, shadows);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
    use crate::effect::EffectSet;
    use crate::span::Span;

    fn bare_call(method: &str) -> Expr {
        Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from(method),
                args: vec![],
                block: None,
                parenthesized: true,
            },
        )
    }

    fn method(name: &str, body: Expr) -> MethodDef {
        MethodDef {
            name: Symbol::from(name),
            receiver: MethodReceiver::Instance,
            params: vec![],
            body,
            signature: None,
            effects: EffectSet::default(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        }
    }

    fn helper_module(name: &str, methods: Vec<MethodDef>) -> LibraryClass {
        LibraryClass {
            name: ClassId(Symbol::from(name)),
            is_module: true,
            parent: None,
            includes: Vec::new(),
            methods,
            nullable_columns: Vec::new(),
            origin: None,
            constants: Vec::new(),
            unknown_calls: Vec::new(),
        }
    }

    fn app_with_room_route() -> App {
        let table = crate::ingest::ingest_routes(
            b"Rails.application.routes.draw do\n  resources :rooms\nend\n",
            "config/routes.rb",
        )
        .expect("routes ingest");
        let mut app = App::default();
        app.routes = table;
        app
    }

    fn receiver_of(e: &Expr) -> Option<String> {
        let ExprNode::Send { recv, .. } = &*e.node else { return None };
        let ExprNode::Const { path } = &*recv.as_ref()?.node else { return None };
        Some(path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"))
    }

    /// The wall: `rails_blob_path` comes from a MOUNTED ENGINE, so it is
    /// in no app route table — a name set derived from `app.routes`
    /// alone leaves the call bare and the emitted tree defines nothing
    /// that answers it.
    #[test]
    fn engine_mounted_helper_is_qualified_in_a_library_class() {
        let app = app_with_room_route();
        let mut lcs =
            vec![helper_module("AttachmentPresentation", vec![method("preview", bare_call("rails_blob_path"))])];
        qualify_lcs(&mut lcs, &app);
        assert_eq!(
            receiver_of(&lcs[0].methods[0].body).as_deref(),
            Some("RouteHelpers"),
            "an engine-mounted route helper must reach the RouteHelpers stub"
        );
    }

    #[test]
    fn an_app_route_helper_is_qualified_in_a_library_class() {
        let app = app_with_room_route();
        let mut lcs = vec![helper_module("Presenter", vec![method("link", bare_call("room_path"))])];
        qualify_lcs(&mut lcs, &app);
        assert_eq!(receiver_of(&lcs[0].methods[0].body).as_deref(), Some("RouteHelpers"));
    }

    /// campfire's `Messages::AttachmentPresentation` calls
    /// `rails_blob_path`, which it does not define, from inside
    /// `download_url`, which it does. A suffix rule with no shadow set
    /// retargets the second call at `RouteHelpers` and the page dies.
    #[test]
    fn a_method_the_module_defines_itself_stays_bare() {
        let app = app_with_room_route();
        let mut lcs = vec![helper_module(
            "AttachmentPresentation",
            vec![
                method("download_url", bare_call("rails_blob_path")),
                method("lightbox_link", bare_call("download_url")),
            ],
        )];
        qualify_lcs(&mut lcs, &app);
        assert_eq!(
            receiver_of(&lcs[0].methods[1].body),
            None,
            "a call to the module's own `_url` method must not be retargeted"
        );
    }

    /// `Webhook#room_bot_messages_path` collides head-on with the route
    /// helper of the same name and forwards to it under a different
    /// arity. A helper module's definition wins over the route table
    /// everywhere the emit resolves a bare call.
    #[test]
    fn a_helper_module_definition_shadows_the_route_of_the_same_name() {
        let app = app_with_room_route();
        let mut lcs = vec![
            helper_module("RoomsHelper", vec![method("room_path", bare_call("noop"))]),
            helper_module("Other", vec![method("render", bare_call("room_path"))]),
        ];
        qualify_lcs(&mut lcs, &app);
        assert_eq!(
            receiver_of(&lcs[1].methods[0].body),
            None,
            "a helper module's own `room_path` outranks the route of that name"
        );
    }

    /// `image_path`, `asset_path` and `polymorphic_url` are
    /// `ActionView::ViewHelpers`' business — a later pass qualifies them
    /// there. The `_path` SUFFIX is not the rule.
    #[test]
    fn a_view_helper_that_is_not_a_route_is_left_alone() {
        let app = app_with_room_route();
        let mut lcs = vec![helper_module(
            "BroadcastsHelper",
            vec![method("a", bare_call("asset_path")), method("b", bare_call("polymorphic_url"))],
        )];
        qualify_lcs(&mut lcs, &app);
        assert_eq!(receiver_of(&lcs[0].methods[0].body), None);
        assert_eq!(receiver_of(&lcs[0].methods[1].body), None);
    }

    /// `RouteHelpers` emits no `_url` form; a later pass rewrites the
    /// URL spelling into a host-prefixed interpolation of the `_path`
    /// twin. Folding it here stripped the host off campfire's
    /// post-authentication redirect.
    #[test]
    fn the_url_spelling_is_left_for_the_host_prefix_pass() {
        let app = app_with_room_route();
        let mut lcs = vec![helper_module("Presenter", vec![method("link", bare_call("room_url"))])];
        qualify_lcs(&mut lcs, &app);
        assert_eq!(receiver_of(&lcs[0].methods[0].body), None);
    }
}
