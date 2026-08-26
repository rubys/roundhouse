//! Non-model library classes under app/models (route-helper includes,
//! Singleton, superclass links), ActionMailer classes, ActiveJob classes,
//! and Sidekiq workers. Extracted verbatim from `Analyzer::with_adapter`.

use std::collections::HashMap;

use crate::analyze::ClassInfo;
use crate::App;
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub(in crate::analyze) fn register(
    classes: &mut HashMap<ClassId, ClassInfo>,
    app: &App,
    route_helper_names: &[String],
) {
    // Library classes: non-model classes living under app/models/
    // (e.g. specialized has_many proxies). Register each as a known
    // class so references like `ArticleCommentsProxy.new(self)` from
    // model methods resolve. Method-by-method registration with
    // proper signatures is a follow-up; for now an empty ClassInfo
    // is enough to type the constructor reference.
    // Modules at least one MODEL mixes in — campfire's
    // `User::Transferable`, `User::Avatar`, `Message::Searchable`.
    // A concern's body is typed as a library class, with `self_ty` set
    // to the MODULE (`analyze/mod.rs`, the `library_classes` loop), so a
    // bare self-send inside one dispatches against the module's own
    // surface and nothing else. ActiveRecord's instance methods are not
    // on that surface, so every `save` / `errors` / `signed_id` written
    // bare in a concern resolved to nothing.
    //
    // Measured on campfire: `User::Transferable#transfer_id` is a
    // one-line `signed_id(purpose: :transfer, expires_in: D)`, and
    // harvested `untyped`. The view's
    // `session_transfer_url(user.transfer_id)` then carried no evidence
    // for `routes_to_library::string_segment_demand`, so that route's
    // `id` segment kept its name-based Integer default and the emitted
    // signature contradicted its only call site — which is where spinel
    // stopped the build.
    //
    // `diagnose` never walks `library_classes`, which is why a gap this
    // broad stayed invisible: the strict emit reported zero errors while
    // the AOT compiler would not accept the tree.
    let model_concerns: std::collections::HashSet<ClassId> = app
        .models
        .iter()
        .flat_map(crate::analyze::model_includes)
        .collect();

    for lc in &app.library_classes {
        let cls = classes.entry(lc.name.clone()).or_default();
        // The AR instance surface a concern inherits from its includer.
        // `or_insert`, so a name the module DEFINES always wins, and so
        // this can never overwrite what the return harvest establishes.
        // Only catalog entries that carry a return type: an entry with
        // `return_kind: None` is not neutral — it lands in the same
        // place an unknown name does.
        if lc.is_module && model_concerns.contains(&lc.name) {
            for entry in crate::catalog::AR_CATALOG {
                if entry.receiver != crate::catalog::ReceiverContext::Instance {
                    continue;
                }
                let Some(kind) = entry.return_kind else { continue };
                if let Some(ty) = ar_instance_ty(kind) {
                    cls.instance_methods.entry(Symbol::from(entry.name)).or_insert(ty);
                }
            }
        }
        // A helper module's own `include`s carry transitively to
        // any class that includes it; record them so dispatch can
        // chase nested mixins.
        cls.includes = lc.includes.clone();
        // `include Singleton` provides `.instance` returning the
        // singleton — the one stdlib mixin worth special-casing:
        // service objects use it pervasively
        // (`ActivityPub::TagManager.instance.uri_for(...)`) and the
        // module itself is stdlib, never ingested, so the concern
        // fold can't supply it.
        if lc.includes.iter().any(|i| i.0.as_str() == "Singleton") {
            cls.class_methods.entry(Symbol::from("instance")).or_insert(Ty::Class {
                id: lc.name.clone(),
                args: vec![],
            });
        }
        // `include Rails.application.routes.url_helpers` (recorded
        // at ingest as an include of the generated RouteHelpers
        // module — lobsters' Routes class does this inside
        // `class << self`): the whole route-helper surface becomes
        // class-callable, every helper returning a path/URL String.
        if lc.includes.iter().any(|i| i.0.as_str() == "RouteHelpers") {
            for name in route_helper_names {
                cls.class_methods
                    .entry(Symbol::from(name.as_str()))
                    .or_insert(Ty::Str);
            }
        }
        // Carry the superclass link so inheritance dispatch walks it.
        // Crucial for classes extending an *unmodeled* gem parent
        // (`TimeSeries < SVG::Graph::TimeSeries`): the walk reaches the
        // unknown ancestor and treats inherited methods as gradual
        // rather than erroring. `is_some` guard so we never clobber a
        // parent another pass established with `None`.
        if lc.parent.is_some() {
            cls.parent = lc.parent.clone();
        }
    }

    // `self.becomes_from(source)` on an STI subclass — the recast
    // constructor `lower::sti_scope` synthesizes for
    // `room.becomes!(Rooms::Closed)`. That pass runs AFTER analyze, so
    // nothing typed the name and campfire's `@room = @room
    // .becomes!(Rooms::Closed)` left `@room` shapeless from the filter
    // on down: four reads in `Rooms::ClosedsController` and two more in
    // the Opens twin reported `@room has no known type` about an ivar
    // holding exactly what its own class name says.
    //
    // Registered for every subclass of a model rather than only the
    // recast targets, on the same reasoning `attachable_sgid` carries:
    // this walk cannot see the demand gate, and the method's TYPE is
    // the same either way — whether it EXISTS is decided at the seam
    // that synthesizes it. Nobody writes the name by hand, so a
    // registration here cannot mask a typo.
    {
        let model_names: std::collections::HashSet<&ClassId> =
            app.models.iter().map(|m| &m.name).collect();
        for lc in &app.library_classes {
            if !lc.parent.as_ref().is_some_and(|p| model_names.contains(p)) {
                continue;
            }
            classes
                .entry(lc.name.clone())
                .or_default()
                .class_methods
                .entry(Symbol::from("becomes_from"))
                .or_insert(Ty::Class { id: lc.name.clone(), args: vec![] });
        }
    }

    // ActionMailer classes: a mailer declares its actions as plain
    // instance `def`s (`def notify(user, …)`) but Rails invokes them
    // on the *class* and returns a deliverable
    // (`BanNotification.notify(…).deliver_now`). The library-class
    // ingest above captured those as instance methods + the
    // `ApplicationMailer < ActionMailer::Base` parent link, so here we
    // (a) identify mailer classes by walking the parent chain to
    // `ActionMailer::Base`, then (b) re-expose each public action as a
    // *class* method returning `ActionMailer::MessageDelivery`. Without
    // this, `Mailer.action` dispatches to "no known method" (no
    // class-side method exists). `entry().or_insert` so a real
    // class-side `def self.x` always wins.
    {
        let parent_of: HashMap<&ClassId, Option<&ClassId>> = app
            .library_classes
            .iter()
            .map(|lc| (&lc.name, lc.parent.as_ref()))
            .collect();
        let is_mailer = |start: &ClassId| -> bool {
            let mut cur = Some(start);
            let mut depth = 0usize;
            while let Some(id) = cur {
                if id.0.as_str() == "ActionMailer::Base" {
                    return true;
                }
                depth += 1;
                if depth > 32 {
                    break;
                }
                cur = parent_of.get(id).copied().flatten();
            }
            false
        };
        let delivery_ty = Ty::Class {
            id: ClassId(Symbol::from("ActionMailer::MessageDelivery")),
            args: vec![],
        };
        for lc in &app.library_classes {
            if !is_mailer(&lc.name) {
                continue;
            }
            let cls = classes.entry(lc.name.clone()).or_default();
            cls.parent = lc.parent.clone();
            for method in &lc.methods {
                // Only source-defined instance actions become
                // class-callable. Real `def self.x` (Class receiver),
                // synthesized accessors, and `initialize` are not
                // mailer actions.
                if method.receiver != crate::dialect::MethodReceiver::Instance
                    || method.kind != crate::dialect::AccessorKind::Method
                    || method.name.as_str() == "initialize"
                {
                    continue;
                }
                cls.class_methods
                    .entry(method.name.clone())
                    .or_insert_with(|| delivery_ty.clone());
            }
        }

        // The deliverable returned by a mailer action. `deliver_now`
        // sends synchronously (really returning the `Mail::Message`);
        // `deliver_later` enqueues an ActiveJob. We model *every*
        // `deliver_*` as returning the delivery itself — the actual
        // `Mail::Message` return is deliberately NOT modeled, because a
        // bare `Mail::Message` class would collide with an app `Message`
        // model under single-segment const resolution (a real lobsters
        // hazard: `Message.find` would resolve to the mail class). The
        // delivery result is invariably discarded at the call site, so
        // a concrete self-type both avoids that collision and keeps the
        // `.deliver_*` link off the gradual-escape (`Untyped`) path.
        let mut delivery_cls = ClassInfo::default();
        for m in [
            "deliver_now",
            "deliver_now!",
            "deliver",
            "deliver_later",
            "deliver_later!",
        ] {
            delivery_cls
                .instance_methods
                .insert(Symbol::from(m), delivery_ty.clone());
        }
        delivery_cls
            .instance_methods
            .insert(Symbol::from("processed?"), Ty::Bool);
        classes
            .entry(ClassId(Symbol::from("ActionMailer::MessageDelivery")))
            .or_insert(delivery_cls);
    }

    // ActiveJob classes: the app defines an instance
    // `def perform(…)` but *calls* the class-side queue entries —
    // `Job.perform_later(…)` / `perform_now(…)` /
    // `set(wait: …).perform_later(…)`. Same shape as the mailer
    // block above: identify jobs by walking the parent chain to
    // `ActiveJob::Base`, then register the entries.
    // `perform_later`/`set` return the class-typed value (`set`
    // collapses to self under the inline semantics
    // `lower::job_class_side` synthesizes, so the chained
    // `perform_later` re-dispatches on the class);
    // `perform_now` returns `perform`'s declared type.
    {
        let parent_of: HashMap<&ClassId, Option<&ClassId>> = app
            .library_classes
            .iter()
            .map(|lc| (&lc.name, lc.parent.as_ref()))
            .collect();
        let is_job = |start: &ClassId| -> bool {
            let mut cur = Some(start);
            let mut depth = 0usize;
            while let Some(id) = cur {
                if id.0.as_str() == "ActiveJob::Base" {
                    return true;
                }
                depth += 1;
                if depth > 32 {
                    break;
                }
                cur = parent_of.get(id).copied().flatten();
            }
            false
        };
        for lc in &app.library_classes {
            if !is_job(&lc.name) {
                continue;
            }
            let self_ty = Ty::Class { id: lc.name.clone(), args: vec![] };
            let perform_ret = lc
                .methods
                .iter()
                .find(|m| {
                    m.receiver == crate::dialect::MethodReceiver::Instance
                        && m.name.as_str() == "perform"
                })
                .and_then(|m| match &m.signature {
                    Some(Ty::Fn { ret, .. }) => Some((**ret).clone()),
                    _ => None,
                })
                .unwrap_or(Ty::Untyped);
            let cls = classes.entry(lc.name.clone()).or_default();
            cls.parent = lc.parent.clone();
            for (entry, ty) in [
                ("perform_later", self_ty.clone()),
                ("set", self_ty.clone()),
                ("perform_now", perform_ret),
            ] {
                cls.class_methods.entry(Symbol::from(entry)).or_insert(ty);
            }
        }
    }

    // Sidekiq workers: `include Sidekiq::Worker` grants the
    // class-side enqueue surface — the app defines an instance
    // `def perform(…)` but *calls* `FooWorker.perform_async(…)` /
    // `perform_in(delay, …)` / `perform_at(time, …)`, all of which
    // return the job id String (invariably discarded). Same shape
    // as the mailer pass above: identify workers by walking the
    // parent chain (Mastodon subclasses base workers, e.g.
    // `UpdateDistributionWorker < RawDistributionWorker`) checking
    // each level's `include` list, then register the enqueue
    // methods. `entry().or_insert` so a real `def self.` wins.
    {
        let lc_of: HashMap<&ClassId, &crate::dialect::LibraryClass> = app
            .library_classes
            .iter()
            .map(|lc| (&lc.name, lc))
            .collect();
        let is_worker = |start: &ClassId| -> bool {
            let mut cur = Some(start);
            let mut depth = 0usize;
            while let Some(id) = cur {
                let Some(lc) = lc_of.get(id) else { break };
                if lc
                    .includes
                    .iter()
                    .any(|inc| inc.0.as_str() == "Sidekiq::Worker")
                {
                    return true;
                }
                depth += 1;
                if depth > 32 {
                    break;
                }
                cur = lc.parent.as_ref();
            }
            false
        };
        for lc in &app.library_classes {
            if !is_worker(&lc.name) {
                continue;
            }
            let cls = classes.entry(lc.name.clone()).or_default();
            if cls.parent.is_none() {
                cls.parent = lc.parent.clone();
            }
            for m in ["perform_async", "perform_in", "perform_at"] {
                cls.class_methods
                    .entry(Symbol::from(m))
                    .or_insert(Ty::Str);
            }
        }
    }
}

/// The type an AR instance method answers when the receiver is a
/// CONCERN rather than a model — `None` for the kinds that are only
/// meaningful relative to a concrete model.
///
/// A module does not know which model includes it (and several may), so
/// `Self`-relative answers cannot be instantiated here: `#reload`
/// returns `SelfType` and there is no honest `Self` to name. Answering
/// them as the module's own class would be a LIE the type system acts
/// on, and answering them `Untyped` would be worse than silence — an
/// `Untyped` arm absorbs dispatch, so it would mask real gaps in every
/// chain built on it. Leaving them out keeps those calls exactly where
/// they are today: unresolved, and honest about it.
///
/// The kinds that ARE model-independent — `#save` is a Bool whoever
/// mixes it in, `#signed_id` a String — carry through.
fn ar_instance_ty(kind: crate::catalog::ReturnKind) -> Option<Ty> {
    use crate::catalog::ReturnKind;
    match kind {
        ReturnKind::Int => Some(Ty::Int),
        ReturnKind::Bool => Some(Ty::Bool),
        ReturnKind::Str => Some(Ty::Str),
        ReturnKind::HashSymStr => {
            Some(Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Str) })
        }
        ReturnKind::ArrayOfSym => Some(Ty::Array { elem: Box::new(Ty::Sym) }),
        ReturnKind::ArrayOfInt => Some(Ty::Array { elem: Box::new(Ty::Int) }),
        ReturnKind::ClassRef(path) => {
            Some(Ty::Class { id: ClassId(Symbol::from(path)), args: vec![] })
        }
        // Self-relative, or a deliberate gradual escape.
        ReturnKind::SelfType
        | ReturnKind::ArrayOfSelf
        | ReturnKind::SelfOrNil
        | ReturnKind::RelationOfSelf
        | ReturnKind::ArrayOfUntyped
        | ReturnKind::Untyped => None,
    }
}
