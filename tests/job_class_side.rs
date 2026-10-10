//! ActiveJob class-side entries (`lower::job_class_side`) + the
//! url_helpers include marker — both upstream-lobsters idioms.

use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_app_from_tree;

fn app_from(files: Vec<(&str, &str)>) -> roundhouse::App {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

#[test]
fn job_classes_gain_inline_class_side_entries() {
    let mut app = app_from(vec![
        (
            "app/jobs/application_job.rb",
            "class ApplicationJob < ActiveJob::Base\nend\n",
        ),
        (
            "app/jobs/notify_job.rb",
            "class NotifyJob < ApplicationJob\n  def perform(thing)\n    thing\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::lower::job_class_side::apply_job_class_side(&mut app);

    let notify = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "NotifyJob")
        .expect("app/jobs ingests as a library class");
    let class_method = |name: &str| {
        notify
            .methods
            .iter()
            .find(|m| {
                m.name.as_str() == name && m.receiver == roundhouse::dialect::MethodReceiver::Class
            })
            .unwrap_or_else(|| panic!("`{name}` class-side entry not synthesized"))
    };

    // perform_later / perform_now forward to new.perform positionally.
    for entry in ["perform_later", "perform_now"] {
        let body = format!("{:?}", class_method(entry).body);
        assert!(
            body.contains("perform") && body.contains("new"),
            "`{entry}` wraps new.perform: {body}"
        );
    }
    // Nothing calls `set`, so none is synthesized.
    assert!(
        !notify.methods.iter().any(|m| m.name.as_str() == "set"),
        "no `set` without a caller"
    );
    let _ = diags;
}

/// `Job.set(opts).perform_later(args)` folds to `Job.perform_later(args)`,
/// the dropped options ledgered at the call site. A `set` whose value is
/// kept (not chained) still gets the class-side `set`, typed `untyped` —
/// its value is the class object, which no `Ty` names.
#[test]
fn a_set_chain_folds_and_a_kept_set_is_synthesized() {
    let mut app = app_from(vec![
        (
            "app/jobs/application_job.rb",
            "class ApplicationJob < ActiveJob::Base\nend\n",
        ),
        (
            "app/jobs/notify_job.rb",
            "class NotifyJob < ApplicationJob\n  def perform(thing)\n    thing\n  end\nend\n",
        ),
        (
            "app/jobs/other_job.rb",
            "class OtherJob < ApplicationJob\n  def perform(thing)\n    thing\n  end\nend\n",
        ),
        (
            "app/models/caller.rb",
            "class Caller\n  def go\n    NotifyJob.set(wait: 5).perform_later(1)\n  end\n\n  def keep\n    OtherJob.set(queue: :low)\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::lower::job_class_side::apply_job_class_side(&mut app);
    let class = |n: &str| {
        app.library_classes
            .iter()
            .find(|lc| lc.name.0.as_str() == n)
            .expect(n)
    };
    let go = class("Caller")
        .methods
        .iter()
        .find(|m| m.name.as_str() == "go")
        .unwrap();
    let body = format!("{:?}", go.body);
    assert!(!body.contains("\"set\""), "the chain folds: {body}");
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("dropped under inline")),
        "dropped set-options must be ledgered: {diags:?}"
    );
    assert!(
        !class("NotifyJob")
            .methods
            .iter()
            .any(|m| m.name.as_str() == "set"),
        "a folded chain leaves no `set` behind"
    );
    let set = class("OtherJob")
        .methods
        .iter()
        .find(|m| m.name.as_str() == "set")
        .expect("a kept `set` is synthesized");
    assert!(format!("{:?}", set.body).contains("SelfRef"));
    assert!(
        matches!(&set.signature, Some(roundhouse::ty::Ty::Fn { ret, .. })
            if matches!(**ret, roundhouse::ty::Ty::Untyped)),
        "`set` is typed untyped, not as an instance: {:?}",
        set.signature
    );
}

#[test]
fn kwarg_perform_stays_unwrapped_on_the_residue_ledger() {
    let mut app = app_from(vec![
        (
            "app/jobs/application_job.rb",
            "class ApplicationJob < ActiveJob::Base\nend\n",
        ),
        (
            "app/jobs/kw_job.rb",
            "class KwJob < ApplicationJob\n  def perform(thing:)\n    thing\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::lower::job_class_side::apply_job_class_side(&mut app);
    let kw = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "KwJob")
        .expect("ingested");
    assert!(
        !kw.methods
            .iter()
            .any(|m| m.name.as_str() == "perform_later"),
        "kwarg perform must not gain a positional wrapper"
    );
    assert!(
        diags.iter().any(|d| d.message.contains("keyword")),
        "kwarg residue must be ledgered: {diags:?}"
    );
}

#[test]
fn url_helpers_include_records_the_route_helpers_marker() {
    // lobsters' Routes: `class << self; include Rails.application.
    // routes.url_helpers; end` — recorded as an include of the
    // generated RouteHelpers module (analyzer registers helper names
    // off it; ruby emit rewrites `Routes.<helper>` call sites).
    let app = app_from(vec![(
        "extras/routes.rb",
        "class Routes\n  class << self\n    include Rails.application.routes.url_helpers\n\n    def title_path(story)\n      story_path(story)\n    end\n  end\nend\n",
    )]);
    let routes = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "Routes")
        .expect("Routes ingested");
    assert!(
        routes
            .includes
            .iter()
            .any(|i| i.0.as_str() == "RouteHelpers"),
        "url_helpers include records the RouteHelpers marker: {:?}",
        routes.includes
    );
}

#[test]
fn singleton_class_includes_are_ledgered_until_emission_support_exists() {
    let app = app_from(vec![(
        "extras/extensions.rb",
        "class ExtensionTarget\n  class << self\n    include ConstantMixin\n    send(:include, DynamicMixin)\n    public_send(:include, OtherMixin)\n  end\nend\n",
    )]);
    let target = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "ExtensionTarget")
        .expect("ExtensionTarget ingested");
    let retained = format!("{:?}", target.unknown_calls);
    for mixin in ["ConstantMixin", "DynamicMixin", "OtherMixin"] {
        assert!(
            retained.contains(mixin),
            "singleton-class extension `{mixin}` must remain in the class body: {retained}"
        );
    }
    assert!(
        !retained.contains("\"include\""),
        "singleton-class include operations normalize to class-side extensions: {retained}"
    );

    let (files, diagnostics) = roundhouse::emit::diagnostics::scope(|| emit_library(&app));
    assert!(
        diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("is not modelled and is dropped")),
        "class-side extensions that the Ruby emitter cannot preserve must remain visible as a gap: {diagnostics:#?}"
    );
    let output = files
        .iter()
        .map(|file| file.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !output.contains("extend ConstantMixin"),
        "do not claim runtime support until class-side extensions survive emission: {output}"
    );
}

#[test]
fn a_sidekiq_worker_without_a_positional_perform_keeps_its_include_and_errors() {
    let mut app = app_from(vec![
        (
            "app/workers/keyword_worker.rb",
            "class KeywordWorker\n  include Sidekiq::Job\n  sidekiq_options retry: 3\n\n  def perform(title:)\n    title\n  end\nend\n",
        ),
        (
            "app/models/caller.rb",
            "class Caller\n  def self.go\n    KeywordWorker.perform_in(5, title: \"x\")\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::lower::job_class_side::apply_job_class_side(&mut app);

    let worker = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "KeywordWorker")
        .expect("app/workers ingests as a library class");
    assert!(
        worker.includes.iter().any(|inc| inc.0.as_str() == "Sidekiq::Job"),
        "the include is kept on an unwrapped worker"
    );
    assert!(
        !worker.methods.iter().any(|m| m.name.as_str() == "perform_async"),
        "no wrapper for a keyword perform"
    );
    assert!(
        diags.iter().any(|d| d.severity == roundhouse::diagnostic::Severity::Error),
        "an unwrapped worker is an error: {diags:?}"
    );
    let body = format!("{:?}", app.library_classes.iter().find(|lc| lc.name.0.as_str() == "Caller").unwrap().methods);
    assert!(body.contains("perform_in"), "perform_in is not folded for an unwrapped worker: {body}");
}

#[test]
fn a_sidekiq_worker_with_a_rest_perform_is_not_wrapped() {
    let mut app = app_from(vec![(
        "app/workers/rest_worker.rb",
        "class RestWorker\n  include Sidekiq::Job\n\n  def perform(first, *rest)\n    first\n  end\nend\n",
    )]);
    let diags = roundhouse::lower::job_class_side::apply_job_class_side(&mut app);
    let worker = app.library_classes.iter().find(|lc| lc.name.0.as_str() == "RestWorker").unwrap();
    assert!(
        !worker.methods.iter().any(|m| m.name.as_str() == "perform_async"),
        "a wrapper would pass the rest as one Array"
    );
    assert!(
        diags.iter().any(|d| d.severity == roundhouse::diagnostic::Severity::Error),
        "an unwrapped worker is an error: {diags:?}"
    );
}

#[test]
fn a_sidekiq_workers_own_perform_in_is_not_folded() {
    let mut app = app_from(vec![
        (
            "app/workers/timed_worker.rb",
            "class TimedWorker\n  include Sidekiq::Job\n\n  def self.perform_in(delay, title)\n    perform_async(title)\n  end\n\n  def perform(title)\n    title\n  end\nend\n",
        ),
        (
            "app/models/caller.rb",
            "class Caller\n  def self.go\n    TimedWorker.perform_in(5, \"x\")\n  end\nend\n",
        ),
    ]);
    let _ = roundhouse::lower::job_class_side::apply_job_class_side(&mut app);
    let body = format!("{:?}", app.library_classes.iter().find(|lc| lc.name.0.as_str() == "Caller").unwrap().methods);
    assert!(body.contains("perform_in"), "the worker's own perform_in is called: {body}");
}
