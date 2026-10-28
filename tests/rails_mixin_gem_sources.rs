//! A Rails mixin from a gem compiled as source is the app's code: a model's
//! `include` of its Concern is lowered.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOCK: &str = "PATH\n  remote: vendor/gems/softer_delete-0.9.0\n  specs:\n    softer_delete (0.9.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.0.2)\n\nDEPENDENCIES\n  activesupport\n  softer_delete!\n";

const RBI: &str = "# typed: true\nmodule SofterDelete\n  module UsingIsNotDeleted\n    def deleted?; end\n  end\nend\n";

const ENTRY: &str = "require \"softer_delete/version\"\nrequire \"active_support\"\nrequire \"active_support/concern\"\nrequire \"active_record\"\n\nmodule SofterDelete\n  module UsingIsNotDeleted\n    extend ActiveSupport::Concern\n\n    included do\n      scope :deleted, -> { where(is_not_deleted: nil) }\n      scope :not_deleted, -> { where(is_not_deleted: true) }\n    end\n\n    def deleted?\n      !is_not_deleted\n    end\n  end\nend\n";

const SCHEMA: &str = "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"notes\", force: :cascade do |t|\n    t.string \"title\"\n    t.boolean \"is_not_deleted\", default: true\n  end\nend\n";

const CARRIED: &str = "test_paths:\n  - test\n";

const NOTE_TEST: &str = r#"require "test_helper"

class NoteTest < ActiveSupport::TestCase
  test "the carried concern's scopes and predicate run" do
    kept = Note.create!(title: "kept")
    gone = Note.create!(title: "gone")
    gone.update!(is_not_deleted: nil)
    assert_equal false, kept.deleted?
    assert_equal true, gone.reload.deleted?
    assert_equal ["kept"], Note.not_deleted.map(&:title)
    assert_equal ["gone"], Note.deleted.map(&:title)
    assert_equal true, Note.include?(SofterDelete::UsingIsNotDeleted)
  end
end
"#;

/// A gem whose load closure reaches Rails (`rails_dependent`).
const GEMS: &str = "rails_dependent_by_name: true\nrails_dependent:\n  - {gem: softer_delete, why: \"requiring it loads 167 Rails framework files\"}\n";

fn app(config: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", &format!("{config}extensions:\n  - ext\n"))
        .write("ext/gems.yml", GEMS)
        .write("Gemfile.lock", LOCK)
        .write("sorbet/rbi/gems/softer_delete@0.9.0.rbi", RBI)
        .write("vendor/gems/softer_delete-0.9.0/lib/softer_delete.rb", ENTRY)
        .write("vendor/gems/softer_delete-0.9.0/lib/softer_delete/version.rb", "module SofterDelete\n  VERSION = \"0.9.0\"\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/note.rb", "class Note < ApplicationRecord\n  include SofterDelete::UsingIsNotDeleted\nend\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("test/models/note_test.rb", NOTE_TEST)
}

#[test]
#[ignore = "fork spec: rails_dependent_by_name"]
fn a_carried_rails_dependent_gem_s_concern_include_is_not_refused() {
    let (emitted, errors) = app(CARRIED).emit(BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("gem_requires_rails")), "{errors:#?}");
    let note = std::fs::read_to_string(emitted.join("app/models/note.rb")).expect("note.rb");
    assert!(!note.contains("gem_requires_rails"), "{note}");
    assert!(note.contains("SofterDelete::UsingIsNotDeleted"), "{note}");
}

#[test]
#[ignore = "fork spec: rails_dependent_by_name"]
fn a_carried_rails_dependent_gem_s_concern_runs_in_emitted_ruby() {
    app(CARRIED).run_test("test/models/note_test.rb").assert_passes();
}

/// The same app with the gem left behind its RBI: the include is the
/// gem's, and the refusal names it.
#[test]
#[ignore = "fork spec: rails_dependent_by_name"]
fn without_the_key_the_include_is_refused_by_name() {
    let (emitted, errors) = app("test_paths:\n  - test\n").emit(BuildTarget::Ruby);
    let refusal = "Note includes SofterDelete::UsingIsNotDeleted, declared by the softer_delete gem, whose load closure requires Rails";
    assert!(errors.iter().any(|e| e.contains("gem_requires_rails") && e.contains(refusal)), "{errors:#?}");
    let note = std::fs::read_to_string(emitted.join("app/models/note.rb")).expect("note.rb");
    assert!(note.contains("error[gem_requires_rails]"), "{note}");
}

/// softer_delete/common.rb and ar_transaction_changes.rb branch on
/// `ActiveRecord.version` in their module bodies (softer_delete 0.9.0:
/// `if ActiveRecord.version >= "7.2.0.alpha"`). Bundler.require'd
/// gem-source entries load unguarded, so before the runtime answered
/// `ActiveRecord.version` (runtime/ruby/active_record.rb, Rails
/// 8.2.0.alpha's Gem::Version) the load raised NoMethodError and one such
/// gem stopped the boot. Native Rails 8.1.4 takes the `>= 7.2` branch, as
/// the pinned 8.2.0.alpha does.
#[test]
#[ignore = "fork spec: rails_dependent_by_name"]
fn a_carried_gem_s_active_record_version_branch_loads() {
    const VERSIONED: &str = "require \"softer_delete/version\"\nrequire \"active_support\"\nrequire \"active_support/concern\"\nrequire \"active_record\"\n\nmodule SofterDelete\n  module UsingIsNotDeleted\n    extend ActiveSupport::Concern\n\n    if ActiveRecord.version >= \"7.2.0.alpha\"\n      def deleted?\n        !is_not_deleted\n      end\n    else\n      def deleted?\n        :old_rails\n      end\n    end\n\n    included do\n      scope :deleted, -> { where(is_not_deleted: nil) }\n      scope :not_deleted, -> { where(is_not_deleted: true) }\n    end\n  end\nend\n";
    app(CARRIED)
        .write("vendor/gems/softer_delete-0.9.0/lib/softer_delete.rb", VERSIONED)
        .run_test("test/models/note_test.rb")
        .assert_passes();
}
