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
    for lc in &app.library_classes {
        let cls = classes.entry(lc.name.clone()).or_default();
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
