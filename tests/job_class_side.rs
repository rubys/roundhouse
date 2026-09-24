//! ActiveJob class-side entries (`lower::job_class_side`) + the
//! url_helpers include marker — both upstream-lobsters idioms.

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
                m.name.as_str() == name
                    && m.receiver == roundhouse::dialect::MethodReceiver::Class
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
        app.library_classes.iter().find(|lc| lc.name.0.as_str() == n).expect(n)
    };
    let go = class("Caller").methods.iter().find(|m| m.name.as_str() == "go").unwrap();
    let body = format!("{:?}", go.body);
    assert!(!body.contains("\"set\""), "the chain folds: {body}");
    assert!(
        diags.iter().any(|d| d.message.contains("dropped under inline")),
        "dropped set-options must be ledgered: {diags:?}"
    );
    assert!(
        !class("NotifyJob").methods.iter().any(|m| m.name.as_str() == "set"),
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
        "`set` is typed untyped, not as an instance: {:?}", set.signature
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
        !kw.methods.iter().any(|m| m.name.as_str() == "perform_later"),
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
        routes.includes.iter().any(|i| i.0.as_str() == "RouteHelpers"),
        "url_helpers include records the RouteHelpers marker: {:?}",
        routes.includes
    );
}
