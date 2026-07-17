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
    // set collapses to self (inline semantics), residue-ledgered.
    let set_body = format!("{:?}", class_method("set").body);
    assert!(set_body.contains("SelfRef"), "`set` returns self: {set_body}");
    assert!(
        diags.iter().any(|d| d.message.contains("dropped under inline")),
        "dropped set-options must be ledgered: {diags:?}"
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
