//! An app reopening a class the runtime ships, as Rails apps reopen
//! framework classes from `lib/core_ext/`, an initializer or a
//! `to_prepare` block. Ruby merges a reopen into the class: a new method
//! joins it, a redefined one replaces the runtime's, and the header's
//! superclass has to agree with the class's own. The tree now does the
//! same (`emit::ruby::reopen`): one definition and one RBS declaration
//! per method, the app's, and a header that names no parent. Before, a
//! redefinition left two declarations for Spinel's `--rbs` to refuse, and
//! a header naming Rails' parent stopped CRuby at load.
//!
//! `ActiveStorage::PurgeJob` is the case that showed it: an app that
//! defines the job itself, which Rails then loads in place of the
//! engine's, gets `purge_later` running its `perform` with the test
//! adapter's semantics intact.
//!
//! The native run needs a Spinel compiler, so it is `#[ignore]`d like
//! its sibling suites; CI's `spinel-framework` job runs it with
//! `--ignored` (`scripts/ci-plan.py`'s `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "docs", force: :cascade do |t|
    t.string "name"
  end
  create_table "active_storage_blobs", force: :cascade do |t|
    t.string "key", null: false
    t.string "filename", null: false
    t.string "content_type"
    t.text "metadata"
    t.string "service_name", null: false
    t.bigint "byte_size", null: false
    t.string "checksum"
    t.datetime "created_at", null: false
  end
  create_table "active_storage_attachments", force: :cascade do |t|
    t.string "name", null: false
    t.string "record_type", null: false
    t.bigint "record_id", null: false
    t.bigint "blob_id", null: false
    t.datetime "created_at", null: false
  end
  create_table "active_storage_variant_records", force: :cascade do |t|
    t.bigint "blob_id", null: false
    t.string "variation_digest", null: false
  end
end
"#;

/// A method the runtime's `Blob` has no counterpart for.
const ADDED: &str = r#"class ActiveStorage::Blob
  def kind_label
    image? ? "image" : "file"
  end
end
"#;

/// Redefinitions of one instance and one class method the runtime's
/// `Blob` defines.
const OVERRIDDEN: &str = r#"class ActiveStorage::Blob
  def audio?
    true
  end

  def self.generate_key(filename, byte_size)
    "app-key-" + byte_size.to_s
  end
end
"#;

/// A redefinition of a method the runtime makes private after its `def`
/// (`private :compute_cache_version`), which `cache_version` calls.
const RELATION: &str = r#"class ActiveRecord::Relation
  def compute_cache_version(timestamp_column)
    "app-version"
  end
end
"#;

/// A redefinition of a runtime method whose RBS signature spans lines
/// (`ActionView::ViewHelpers.word_wrap`, the module `word_wrap` in a view
/// lowers to).
const VIEW_HELPER: &str = r#"module ActionView::ViewHelpers
  def self.word_wrap(text, line_width: 80, break_sequence: "\n")
    "wrapped: " + text
  end
end
"#;

/// Rails' parent in the header: neither class is one the runtime ships.
const WITH_PARENT: &str = r#"class ActiveStorage::Blob < ActiveStorage::Record
  def kind_label
    image? ? "image" : "file"
  end
end
"#;

/// The job an app defines in place of the engine's. Its `perform` leaves
/// a row behind, so a test can tell it ran rather than the runtime's.
const PURGE_JOB: &str = r#"class ActiveStorage::PurgeJob < ActiveJob::Base
  queue_as { ActiveStorage.queues[:purge] }
  queue_with_priority 200

  discard_on ActiveRecord::RecordNotFound
  retry_on ActiveRecord::Deadlocked, attempts: 10, wait: :polynomially_longer

  def perform(blob)
    Doc.create!(name: "purged by the app")
    blob.purge
  end
end
"#;

/// The same job reopened with no parent in its header: only `perform`
/// changes, and the runtime's `perform_later` runs it.
const PURGE_JOB_REOPEN: &str = r#"class ActiveStorage::PurgeJob
  def perform(blob)
    Doc.create!(name: "purged by the app")
    blob.purge
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write("app/models/doc.rb", "class Doc < ApplicationRecord\n  has_one_attached :file\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write(
            "config/application.rb",
            "require_relative \"boot\"\nrequire \"rails/all\"\n\nmodule Demo\n  class Application < Rails::Application\n    config.load_defaults 8.0\n    config.autoload_lib(ignore: %w(assets tasks core_ext))\n  end\nend\n",
        )
        .write(
            "config/initializers/core_ext.rb",
            "Dir[Rails.root.join(\"lib/core_ext/*.rb\")].each { |l| require l }\n",
        )
}

fn reopening(file: &str, source: &str) -> emit_and_run::Overlay {
    app().write(&format!("lib/core_ext/{file}"), source)
}

fn read(emitted: &std::path::Path, path: &str) -> String {
    std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// One class's body in a runtime file or sidecar: from its header to
/// its `end` at the namespace's indent.
fn class_body<'a>(source: &'a str, header: &str) -> &'a str {
    let start = source.find(header).unwrap_or_else(|| panic!("no {header:?}"));
    let rest = &source[start..];
    &rest[..rest.find("\n  end\n").unwrap_or(rest.len())]
}

/// The `def` lines a file has for `name`, as written.
fn defs<'a>(source: &'a str, name: &str) -> Vec<&'a str> {
    let def = format!("def {name}");
    source
        .lines()
        .map(str::trim)
        .filter(|l| l.strip_prefix(def.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with(['(', ':', ' '])))
        .collect()
}

const ATTACHED: &str = r#"
doc = Doc.create!(name: "kept")
doc.file.attach("bytes", "a.txt", "text/plain")
blob = doc.file.blob
"#;

#[test]
fn an_added_method_joins_the_runtime_class() {
    reopening("active_storage_blob.rb", ADDED)
        .run_ruby(&format!("{ATTACHED}raise blob.kind_label unless blob.kind_label == \"file\"\nraise \"lost #video?\" if blob.video?\n"))
        .assert_passes();
}

/// Each tree keeps one definition and one declaration of each method,
/// the app's; the runtime's other methods stay.
#[test]
fn a_redefined_method_replaces_the_runtimes_in_the_tree() {
    for (target, rbs) in [
        (BuildTarget::Spinel, "runtime/active_storage.rbs"),
        (BuildTarget::Ruby, "sig/runtime/active_storage.rbs"),
    ] {
        let (emitted, errors) = reopening("active_storage_blob.rb", OVERRIDDEN).emit(target);
        assert!(errors.is_empty(), "{errors:?}");
        let runtime = read(&emitted, "runtime/active_storage.rb");
        let runtime = class_body(&runtime, "\n  class Blob\n");
        let sig = read(&emitted, rbs);
        let sig = class_body(&sig, "\n  class Blob\n");
        assert_eq!(defs(runtime, "audio?"), Vec::<&str>::new(), "{target:?}");
        assert_eq!(defs(runtime, "self.generate_key"), Vec::<&str>::new(), "{target:?}");
        assert_eq!(defs(sig, "audio?"), Vec::<&str>::new(), "{target:?}");
        assert_eq!(defs(sig, "self.generate_key"), Vec::<&str>::new(), "{target:?}");
        assert_eq!(defs(runtime, "video?").len(), 1, "{target:?}");
        assert_eq!(defs(sig, "video?").len(), 1, "{target:?}");
        let app = read(&emitted, "app/models/active_storage/blob.rb");
        assert_eq!(defs(&app, "audio?").len(), 1, "{app}");
        assert_eq!(defs(&app, "self.generate_key").len(), 1, "{app}");
    }
}

#[test]
fn a_redefined_instance_and_class_method_win() {
    reopening("active_storage_blob.rb", OVERRIDDEN)
        .run_ruby(&format!(
            "{ATTACHED}raise \"audio? is the runtime's\" unless blob.audio?\nraise blob.key unless blob.key == \"app-key-5\"\n"
        ))
        .assert_passes();
}

/// The runtime's `private :compute_cache_version` goes with its `def`,
/// so the tree loads, and the runtime's `cache_version` calls the app's.
#[test]
fn a_redefined_private_method_loads_and_wins() {
    let (emitted, errors) = reopening("active_record_relation.rb", RELATION).emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let relation = read(&emitted, "runtime/active_record/relation.rb");
    assert!(!relation.contains("private :compute_cache_version"));
    assert!(relation.contains("private :windowed_cache_version"));
    drop(emitted);
    reopening("active_record_relation.rb", RELATION)
        .run_ruby("raise Doc.all.cache_version unless Doc.all.cache_version == \"app-version\"\n")
        .assert_passes();
}

/// The runtime's declaration goes whole, every line of its signature, so
/// the sidecar stays valid RBS; the app's method is the one that runs.
#[test]
fn a_redefined_method_with_a_multiline_signature_goes_whole() {
    let (emitted, errors) = reopening("action_view_view_helpers.rb", VIEW_HELPER).emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let sig = read(&emitted, "runtime/action_view/view_helpers.rbs");
    assert_eq!(defs(&sig, "self.word_wrap"), Vec::<&str>::new());
    assert!(sig.contains("Integer pos) -> Integer\n    def self.word_wrap_finish: "), "{sig}");
    drop(emitted);
    reopening("action_view_view_helpers.rb", VIEW_HELPER)
        .run_ruby("raise ActionView::ViewHelpers.word_wrap(\"abc\") unless ActionView::ViewHelpers.word_wrap(\"abc\") == \"wrapped: abc\"\n")
        .assert_passes();
}

/// `ActiveStorage::Record` is Rails' parent for `Blob`; the runtime has
/// neither it nor a parent for its `Blob`. The reopen loads, as in Rails.
#[test]
fn a_reopen_header_naming_rails_parent_loads() {
    let (emitted, errors) = reopening("active_storage_blob.rb", WITH_PARENT).emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(read(&emitted, "app/models/active_storage/blob.rb").contains("class ActiveStorage::Blob\n"));
    assert!(read(&emitted, "app/models/active_storage/blob.rbs").contains("  class Blob\n"));
    drop(emitted);
    reopening("active_storage_blob.rb", WITH_PARENT)
        .run_ruby(&format!("{ATTACHED}raise blob.kind_label unless blob.kind_label == \"file\"\n"))
        .assert_passes();
}

/// The job's `perform_later` and `perform` are the app's, once each,
/// and the header that named `ActiveJob::Base` reopens the runtime's
/// class without a mismatch.
#[test]
fn an_app_purge_job_replaces_the_runtimes() {
    for (target, rbs) in [
        (BuildTarget::Spinel, "runtime/active_storage.rbs"),
        (BuildTarget::Ruby, "sig/runtime/active_storage.rbs"),
    ] {
        let (emitted, errors) = reopening("active_storage_purge_job.rb", PURGE_JOB).emit(target);
        assert!(errors.is_empty(), "{errors:?}");
        let runtime = read(&emitted, "runtime/active_storage.rb");
        let sig = read(&emitted, rbs);
        for name in ["perform", "self.perform_later"] {
            assert_eq!(defs(class_body(&runtime, "\n  class PurgeJob\n"), name), Vec::<&str>::new(), "{target:?} {name}");
            assert_eq!(defs(class_body(&sig, "\n  class PurgeJob\n"), name), Vec::<&str>::new(), "{target:?} {name}");
        }
        assert!(runtime.contains("PurgeJob.perform_later(self)"), "Blob#purge_later still enqueues it");
        let job = read(&emitted, "app/models/active_storage/purge_job.rb");
        assert!(job.contains("class ActiveStorage::PurgeJob\n"), "{job}");
        assert_eq!(defs(&job, "self.perform_later").len(), 1, "{job}");
    }
}

/// Without a reopen the runtime ships as written, its job included.
#[test]
fn without_a_reopen_the_runtime_ships_as_written() {
    let (emitted, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(read(&emitted, "runtime/active_storage.rb"), include_str!("../runtime/ruby/active_storage.rb"));
    let sig = read(&emitted, "runtime/active_storage.rbs");
    let job = class_body(&sig, "\n  class PurgeJob\n");
    assert_eq!(defs(job, "perform").len(), 1, "{job}");
    assert_eq!(defs(job, "self.perform_later").len(), 1, "{job}");
}

/// The test adapter holds the purge as it held the runtime's
/// (`assert_enqueued_jobs only: ActiveStorage::PurgeJob`), and running
/// it runs the app's `perform`.
const ENQUEUE_TEST: &str = r#"require "test_helper"

class PurgeJobTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  test "purge_later holds the app's job, which purges when performed" do
    doc = Doc.create!(name: "kept", file: { io: StringIO.new("bytes"), filename: "a.txt", content_type: "text/plain" })
    assert_enqueued_jobs 1, only: ActiveStorage::PurgeJob do
      doc.file.purge_later
    end
    assert_equal 1, ActiveStorage::Blob.count
    assert_equal 0, Doc.where(name: "purged by the app").count

    perform_enqueued_jobs only: ActiveStorage::PurgeJob

    assert_equal 0, ActiveStorage::Blob.count
    assert_equal 1, Doc.where(name: "purged by the app").count
  end
end
"#;

#[test]
fn purge_later_runs_the_app_purge_job() {
    reopening("active_storage_purge_job.rb", PURGE_JOB)
        .write("test/models/purge_job_test.rb", ENQUEUE_TEST)
        .run_test("test/models/purge_job_test.rb")
        .assert_passes();
}

#[test]
fn purge_later_runs_a_parentless_reopens_perform() {
    reopening("active_storage_purge_job.rb", PURGE_JOB_REOPEN)
        .write("test/models/purge_job_test.rb", ENQUEUE_TEST)
        .run_test("test/models/purge_job_test.rb")
        .assert_passes();
}

/// Inline, outside the test adapter: the owner's destroy purges through
/// the app's job too (`dependent: :purge_later`).
const DESTROY: &str = r#"
doc = Doc.create!(name: "kept")
doc.file.attach("bytes", "a.txt", "text/plain")
doc.destroy
raise "blobs left" unless ActiveStorage::Blob.count == 0
raise "the app's perform did not run" unless Doc.where(name: "purged by the app").count == 1
raise "PurgeJob not recorded" unless ActiveJob.performed.include?("ActiveStorage::PurgeJob")
"#;

#[test]
fn destroying_an_owner_runs_the_app_purge_job() {
    reopening("active_storage_purge_job.rb", PURGE_JOB).run_ruby(DESTROY).assert_passes();
}

/// Every shape above in one native build, under `--rbs` as the shipped
/// Makefile compiles it.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn the_reopens_compile_and_run_on_spinel() {
    reopening("active_storage_blob.rb", OVERRIDDEN)
        .write("lib/core_ext/active_storage_blob_label.rb", WITH_PARENT)
        .write("lib/core_ext/active_storage_purge_job.rb", PURGE_JOB)
        .write("lib/core_ext/active_record_relation.rb", RELATION)
        .write("lib/core_ext/action_view_view_helpers.rb", VIEW_HELPER)
        .run_spinel_with_rbs(&format!(
            "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{ATTACHED}\
             raise blob.kind_label unless blob.kind_label == \"file\"\n\
             raise \"audio? is the runtime's\" unless blob.audio?\n\
             raise blob.key unless blob.key == \"app-key-5\"\n\
             raise Doc.all.cache_version unless Doc.all.cache_version == \"app-version\"\n\
             raise ActionView::ViewHelpers.word_wrap(\"abc\") unless ActionView::ViewHelpers.word_wrap(\"abc\") == \"wrapped: abc\"\n\
             doc.destroy\n\
             raise \"blobs left\" unless ActiveStorage::Blob.count == 0\n\
             raise \"the app's perform did not run\" unless Doc.where(name: \"purged by the app\").count == 1\n\
             puts \"reopens passed\"\n"
        ))
        .assert_passes();
}
