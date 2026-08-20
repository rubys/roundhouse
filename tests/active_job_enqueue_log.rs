//! `assert_enqueued_jobs` over an INLINE adapter.
//!
//! Rails' ActiveJob test helpers count enqueues by inspecting the test
//! adapter's queue. This runtime's adapter is inline — `perform_later`
//! runs the job — so there is no queue, which is equally true of Rails'
//! own `:inline` adapter and exactly why Rails' helpers require the
//! `:test` one.
//!
//! `ActiveJob::PERFORMED` is the equivalent seam, and it is the same
//! shape as `Broadcasts::LOG`: the wrapper appends before dispatching,
//! and every helper reads a LENGTH DELTA across its block. "Enqueued
//! during this block" and "ran during this block" are then the same
//! set; the shape that separates them — enqueued and never run —
//! inline semantics cannot produce.
//!
//! Entries are class NAMES. A class is not a first-class value on the
//! strict targets, so the call sites that name one
//! (`only: Bot::WebhookJob`) are rewritten to the string at compile
//! time by `lower::job_test_only`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::app::App;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn app() -> App {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :things do |t|\n    t.string :name\n  end\nend\n",
        ),
        ("app/models/thing.rb", "class Thing < ApplicationRecord\nend\n"),
        (
            "app/jobs/application_job.rb",
            "class ApplicationJob < ActiveJob::Base\nend\n",
        ),
        (
            "app/jobs/notify_job.rb",
            "class NotifyJob < ApplicationJob\n  def perform(thing)\n    nil\n  end\nend\n",
        ),
        (
            "test/models/thing_test.rb",
            r#"require "test_helper"

class ThingTest < ActiveSupport::TestCase
  test "enqueues" do
    assert_enqueued_jobs 1, only: NotifyJob do
      NotifyJob.perform_later(Thing.first)
    end
  end

  test "enqueues one of several" do
    assert_no_enqueued_jobs only: [ NotifyJob, OtherJob ] do
      nil
    end
  end

  test "enqueued with" do
    assert_enqueued_with(job: NotifyJob) do
      nil
    end
  end
end
"#,
        ),
    ]))
    .expect("ingest job app");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn emitted(suffix: &str) -> String {
    let app = app();
    let files = ruby::emit_library(&app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no file ending in {suffix}; got {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>()
            )
        })
}

/// The lowered test bodies, as IR — there is no per-file test emit
/// entry to read text from, and the rewrite is a lowering anyway.
fn test_bodies() -> String {
    let app = app();
    app.test_modules
        .iter()
        .flat_map(|m| m.tests.iter().map(|t| format!("{:?}", t.body)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `perform_later` logs; `perform_now` does not — in Rails it runs
/// WITHOUT enqueueing, and the helpers count enqueues.
#[test]
fn perform_later_records_and_perform_now_does_not() {
    let src = emitted("notify_job.rb");
    let later = src
        .split("def self.perform_later")
        .nth(1)
        .unwrap_or_else(|| panic!("no perform_later:\n{src}"));
    assert!(
        later.contains(r#"ActiveJob.record_performed("NotifyJob")"#),
        "perform_later must log:\n{later}"
    );
    let now = src
        .split("def self.perform_now")
        .nth(1)
        .unwrap_or_else(|| panic!("no perform_now:\n{src}"));
    let now_body = now.split("end").next().unwrap_or("");
    assert!(
        !now_body.contains("record_performed"),
        "perform_now enqueues nothing:\n{now_body}"
    );
}

/// The filters become an array of names in both spellings; `job:`,
/// which names exactly one class, becomes a plain string.
#[test]
fn job_classes_in_test_filters_become_names() {
    let ir = test_bodies();
    // Every job class named in a filter is a STRING literal now, and no
    // `Const` for one survives.
    for name in ["NotifyJob", "OtherJob"] {
        assert!(
            ir.contains(&format!("Str {{ value: \"{name}\" }}")),
            "{name} must reach the helper as a name:\n{ir}"
        );
    }
    // `OtherJob` appears ONLY inside a filter, so its Const surviving
    // would mean the rewrite missed it. (`NotifyJob` also appears as a
    // real receiver inside a block, where it is still a class.)
    assert!(
        !ir.contains("Const { path: [Symbol(\"OtherJob\")] }"),
        "a filter-only job class must not survive as a constant:\n{ir}"
    );
}
