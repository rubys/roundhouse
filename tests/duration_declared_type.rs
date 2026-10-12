//! `TTL = 30.minutes #: ActiveSupport::Duration`, a duration constant
//! with a Sorbet-style ascription. The Ruby-family trees
//! ship that class (`runtime/spinel/active_support_duration.rb`), so the
//! ascription names a modeled dependency and
//! its surface types the reads. A tree that ships no Duration refuses
//! the value instead.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const POLICY: &str = r#"class CachePolicy
  TTL = 30.minutes #: ActiveSupport::Duration

  def ttl_seconds
    TTL.to_i
  end

  def expires_later?
    TTL.from_now > Time.now
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :things do |t|\n    t.string :name\n  end\nend\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("lib/cache_policy.rb", POLICY)
        .write("test/cache_policy_test.rb", r#"require "test_helper"
require "cache_policy"
class CachePolicyTest < ActiveSupport::TestCase
  test "a declared duration constant reads as seconds and as a time" do
    assert_equal 1800, CachePolicy.new.ttl_seconds
    assert_equal true, CachePolicy.new.expires_later?
  end
end
"#)
}

#[test]
fn a_declared_duration_is_a_modeled_dependency() {
    let (_, errors) = app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn a_declared_duration_runs_in_emitted_ruby() {
    app().run_test("test/models/cache_policy_test.rb").assert_passes();
}

#[test]
fn a_tree_without_a_duration_class_refuses_it() {
    let (_, errors) = app().emit(BuildTarget::Rust);
    assert!(errors.iter().any(|e| e.contains("the rust runtime ships none")), "{errors:#?}");
}
