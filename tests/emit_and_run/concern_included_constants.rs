use super::emit_and_run;

/// A concern's `included do` validation bounded by a constant of the
/// concern's own module body, in an includer that defines a constant of
/// the same name. Rails reads the concern's (the block's lexical scope),
/// so the answers below are Rails 8.1.4's for the same model and records:
/// 150 characters pass, 151 fail with the concern's bound in the message,
/// and the concern's regex rejects a capitalized kind.
#[test]
fn a_concern_constant_bounds_its_included_validation_like_rails() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"description\"\n    t.string \"kind\"\n  end\nend\n",
        )
        .write(
            "app/models/concerns/describable.rb",
            r#"module Describable
  extend ActiveSupport::Concern

  DESCRIPTION_LIMIT = 150
  KIND_RE = /\A[a-z]+\z/

  included do
    validates :description, length: { maximum: DESCRIPTION_LIMIT }
    validates :kind, format: { with: KIND_RE }
  end
end
"#,
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  DESCRIPTION_LIMIT = 99\n  include Describable\nend\n")
        .run_ruby(
            r#"got = [["a" * 150, "note"], ["a" * 151, "note"], ["a", "Note1"]].map do |description, kind|
  w = Widget.new(description: description, kind: kind)
  w.valid?
  w.errors.to_a
end
want = [[], ["Description is too long (maximum is 150 characters)"], ["Kind is invalid"]]
raise "errors: #{got.inspect}" unless got == want
puts "concern constants passed"
"#,
        )
        .assert_passes();
}
