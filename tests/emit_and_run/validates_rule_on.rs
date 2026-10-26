use super::emit_and_run;

/// `on:` inside a validator's own hash (`presence: { if: :available?, on:
/// :update }`), beside or instead of a call-level `on:`. Rails merges the
/// call's options under each validator's own, so the rule's `on:` wins
/// (`code` is checked on update although the call says `on: :create`), and
/// it combines with the rule's `if:`. It was a reported gap. Answers are
/// Rails 8.1.4's for the same model and records: a new record, then the
/// saved one with the attribute available, then not.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"items\", force: :cascade do |t|\n    t.string \"pictures\"\n    t.boolean \"available\"\n    t.string \"name\"\n    t.string \"code\"\n  end\nend\n",
        )
        .write(
            "app/models/item.rb",
            r#"class Item < ApplicationRecord
  validates :pictures, presence: { if: :available?, on: :update }
  validates :name, presence: { on: :create }, length: { maximum: 3 }
  validates :code, presence: { on: :update }, on: :create
end
"#,
        )
}

#[test]
fn a_rule_level_on_guards_like_rails() {
    app()
        .run_ruby(
            r##"def errs(r) = (r.valid?; r.errors.to_a)
n = Item.new
n.available = true
got = [errs(n)]
i = Item.new
{ pictures: "x", available: true, name: "ab", code: "c" }.each { |k, v| i.public_send(:"#{k}=", v) }
i.save!
i.pictures = nil
i.name = nil
i.code = nil
got << errs(i)
i.available = false
i.name = "long"
got << errs(i)
want = [
  ["Name can't be blank"],
  ["Pictures can't be blank", "Code can't be blank"],
  ["Name is too long (maximum is 3 characters)", "Code can't be blank"],
]
raise "errors: #{got.inspect}" unless got == want
puts "rule-level on passed"
"##,
        )
        .assert_passes();
}

#[test]
fn strict_targets_emit_a_rule_level_on_without_errors() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Go, BuildTarget::Crystal] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}
