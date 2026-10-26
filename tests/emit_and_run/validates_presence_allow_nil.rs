use super::emit_and_run;

/// `validates :x, presence: true, allow_nil: true`: Rails' EachValidator
/// skips a nil value, so presence then checks only a non-nil value for
/// blank (`""` fails). It was a reported gap. A call-level `if:` still
/// guards the call. The rule's own `allow_nil: false` wins over the
/// call's `allow_nil: true`, so `label` still rejects nil. Answers are Rails 8.1.4's for the same model and
/// records. A whitespace-only String is left out: the presence check's
/// blank test is narrower than Rails' `blank?`, a separate known gap.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"items\", force: :cascade do |t|\n    t.string \"name\"\n    t.string \"code\"\n    t.string \"label\"\n    t.boolean \"live\"\n  end\nend\n",
        )
        .write(
            "app/models/item.rb",
            r#"class Item < ApplicationRecord
  validates :name, presence: true, allow_nil: true
  validates :code, presence: true, length: { maximum: 3 }, allow_nil: true, if: :live?
  validates :label, presence: { allow_nil: false }, allow_nil: true
end
"#,
        )
}

#[test]
fn presence_with_allow_nil_skips_nil_like_rails() {
    app()
        .run_ruby(
            r#"rows = [[nil, nil, true, nil], ["", "", true, ""], ["x", "long", true, "y"], ["x", "", false, "y"]]
got = rows.map do |name, code, live, label|
  r = Item.new
  r.name = name
  r.code = code
  r.live = live
  r.label = label
  r.valid?
  r.errors.to_a
end
want = [
  ["Label can't be blank"],
  ["Name can't be blank", "Code can't be blank", "Label can't be blank"],
  ["Code is too long (maximum is 3 characters)"],
  [],
]
raise "errors: #{got.inspect}" unless got == want
puts "presence allow_nil passed"
"#,
        )
        .assert_passes();
}

#[test]
fn strict_targets_emit_presence_with_allow_nil_without_errors() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Go, BuildTarget::Crystal] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}
