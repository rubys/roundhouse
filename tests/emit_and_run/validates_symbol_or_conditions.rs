use super::emit_and_run;

/// `if: :a? || :b?` is evaluated by Ruby before `validates` sees it. A
/// Symbol is truthy, so `:a? || :b?` is `:a?` and `:a? && :b?` is `:b?`:
/// Rails stores one Symbol and checks only that predicate. The same holds
/// for the condition inside a validator's own hash
/// (`presence: { if: :a? || :b? }`), and along a chain
/// (`:a? || :b? || :c?` is `:a?`, `:a? && :b? && :c?` is `:c?`). Answers below are Rails 8.1.4's for
/// the same model and records: a "square" record skips the `if:` rules
/// (only `round?` is checked) and a "flat" one runs the `unless:` rules.
const MODEL: &str = r#"class Widget < ApplicationRecord
  validates :name, presence: true, if: :round? || :square?
  validates :label, presence: true, unless: :round? || :square?
  validates :code, presence: { if: :round? || :square? }
  validates :tag, presence: { unless: (:round? && :square?) }
  validates :note, presence: { if: :round? || :square? || :flat? }
  validates :memo, presence: true, if: :flat? && :round? && :square?

  def round?
    kind == "round"
  end

  def square?
    kind == "square"
  end

  def flat?
    kind == "flat"
  end
end
"#;

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "kind"
    t.string "name"
    t.string "label"
    t.string "code"
    t.string "tag"
    t.string "note"
    t.string "memo"
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", MODEL)
}

#[test]
fn symbol_or_and_conditions_report_no_gap() {
    let warnings = app().ingest_warnings();
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn symbol_or_and_conditions_check_the_symbol_ruby_yields_like_rails() {
    app()
        .run_ruby(
            r#"got = %w[round square flat].map do |kind|
  w = Widget.new(kind: kind)
  w.valid?
  [kind, w.errors.to_a]
end
want = [
  ["round", ["Name can't be blank", "Code can't be blank", "Tag can't be blank", "Note can't be blank"]],
  ["square", ["Label can't be blank", "Memo can't be blank"]],
  ["flat", ["Label can't be blank", "Tag can't be blank"]],
]
raise "errors: #{got.inspect}" unless got == want
puts "symbol or conditions passed"
"#,
        )
        .assert_passes();
}

/// The folded condition is the plain Symbol guard every target already
/// lowers; none rejects it.
#[test]
fn symbol_or_and_conditions_emit_cleanly_on_every_backend_target() {
    use roundhouse::project::BuildTarget::*;
    for target in [Ruby, Rust, Typescript, Go, Python, CSharp, Kotlin, Swift, Crystal, Elixir, Roda] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}

/// A callback's `if:` goes through the same reader, so it calls the same
/// one predicate Rails does: only `round?` stamps the note.
#[test]
fn a_callback_symbol_or_condition_runs_like_rails() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  before_validation :stamp, if: :round? || :square?\n\n  def stamp\n    self.tag = \"stamped\"\n  end\n\n  def round?\n    kind == \"round\"\n  end\n\n  def square?\n    kind == \"square\"\n  end\nend\n",
        )
        .run_ruby(
            r#"got = %w[round square flat].map do |kind|
  w = Widget.new(kind: kind)
  w.save
  w.tag
end
raise "tags: #{got.inspect}" unless got == ["stamped", nil, nil]
puts "callback symbol or condition passed"
"#,
        )
        .assert_passes();
}
