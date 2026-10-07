//! A nested class header's superclass and its body's mixins, written
//! with a leading `::` in the source, keep binding the top-level
//! constant in the emitted tree.
//!
//! Reduction: `class Value < ::Api::Base` inside `module Admin; module
//! Api; module Nested`. The ingest records the absolute name without its
//! `::`, and emitted bare, `Api` binds `Admin::Api`: `uninitialized
//! constant Admin::Api::Base` at load. A mixin written relative
//! (`include Helpers`) still binds the scope's.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn a_shadowed_top_level_superclass_and_mixin_stay_top_level() {
    // Native oracle (ruby -Ilib, same files): [:top, :top_shared, :admin_helper].
    let run = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("lib/api/base.rb", "module Api\n  class Base\n    def self.kind = :top\n  end\nend\n")
        .write("lib/api/shared.rb", "module Api\n  module Shared\n    def shared = :top_shared\n  end\nend\n")
        .write("lib/helpers.rb", "module Helpers\n  def helper = :top_helper\nend\n")
        .write(
            "lib/admin/api/helpers.rb",
            "module Admin\n  module Api\n    module Helpers\n      def helper = :admin_helper\n    end\n  end\nend\n",
        )
        .write(
            "lib/admin/api/nested/value.rb",
            "require \"api/base\"\nrequire \"api/shared\"\nrequire \"helpers\"\nrequire \"admin/api/helpers\"\n\nmodule Admin\n  module Api\n    module Nested\n      class Value < ::Api::Base\n        include ::Api::Shared\n        include Helpers\n      end\n    end\n  end\nend\n",
        )
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("test/value_test.rb", r#"require "test_helper"
require "admin/api/nested/value"
class ValueTest < ActiveSupport::TestCase
  test "the header and mixins bind what the source names" do
    value = Admin::Api::Nested::Value
    assert_equal [:top, :top_shared, :admin_helper], [value.kind, value.new.shared, value.new.helper]
  end
end
"#)
        .run_test("test/models/value_test.rb");
    run.assert_passes();
}
