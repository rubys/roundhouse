use super::emit_and_run;

/// Issue #985: `validates :attr, if:`/`unless:` with a lambda/proc, not
/// just the Symbol form #1077 already lowers. Reuses
/// `callback_condition` (`ingest/model.rs`) — the same zero-arity-body
/// reader a callback's `if:`/`unless:` already goes through — so a
/// lambda condition becomes a guard expression the same way a Symbol
/// condition becomes a bare predicate call, both wrapping the check in
/// `def validate` with `self` the record (Rails `instance_exec`s the
/// condition the same way).
///
/// `:name` is a call-level lambda, `:code` a call-level `lambda { }`,
/// `:color`/`:label` the Symbol form #1077 already covers (kept here so
/// one app exercises both forms together), and `:note` a RULE-level
/// lambda (`presence: { if: -> { … } }`) — the nested-hash spelling
/// `push_symbol_condition`'s call sites also parse.
const MODEL: &str = r#"class Widget < ApplicationRecord
  validates :name, presence: true, unless: -> { status.in?(%w[draft]) }
  validates :code, presence: true, if: lambda { status == "live" }
  validates :color, presence: true, unless: :draft?
  validates :label, presence: true, if: :live?
  validates :note, presence: { if: -> { live? } }

  def draft?
    status == "draft"
  end

  def live?
    status == "live"
  end
end
"#;

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
    t.string "code"
    t.string "color"
    t.string "label"
    t.string "note"
    t.string "status", default: "draft"
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", MODEL)
}

/// The exact Rails 8.1 answers #985's probe confirmed: a "draft" record
/// with every conditional attribute blank is valid (every guard is
/// false or negated-true), and a "live" one with the same blanks fails
/// every one of them, in this order.
#[test]
fn lambda_and_symbol_conditions_guard_the_validation_like_rails() {
    app().run_ruby(r##"
draft = Widget.new(status: "draft")
raise "draft should be valid: #{draft.errors.inspect}" unless draft.valid?
raise "draft errors: #{draft.errors.inspect}" unless draft.errors == []

live = Widget.new(status: "live")
raise "live should be invalid" if live.valid?
expected = ["Name can't be blank", "Code can't be blank", "Color can't be blank", "Label can't be blank", "Note can't be blank"]
raise "live errors: #{live.errors.inspect}" unless live.errors == expected

# Filling every conditionally-required attribute passes on "live".
filled = Widget.new(status: "live", name: "n", code: "c", color: "red", label: "l", note: "x")
raise "filled live should be valid: #{filled.errors.inspect}" unless filled.valid?

puts "lambda conditions passed"
"##)
        .assert_passes();
}

#[test]
fn lambda_conditions_report_no_gap_warnings() {
    let warnings = app().ingest_warnings();
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// An arity>0 lambda (`->(w) { … }`) is not the zero-arity shape
/// `callback_condition` accepts (its body would splice an unbound
/// parameter into `def validate`), so the whole `validates` call stays
/// the reported gap it already was — not silently promoted to an
/// always-on check, and not silently dropped either: the attribute
/// saves blank on every status, same as before this build understood
/// any lambda condition.
const ARITY_ONE_MODEL: &str = r#"class Widget < ApplicationRecord
  validates :name, presence: true, if: ->(w) { w.live? }

  def live?
    status == "live"
  end
end
"#;

#[test]
fn arity_one_lambda_condition_stays_a_reported_gap() {
    let warnings = emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", ARITY_ONE_MODEL)
        .ingest_warnings();
    assert!(
        warnings.iter().any(|m| m.contains("if:")),
        "missing if: gap warning in {warnings:?}"
    );

    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", ARITY_ONE_MODEL)
        .run_ruby(r##"
live = Widget.new(status: "live", name: "")
raise "arity>0 lambda condition should stay a gap (unconditional pass)" unless live.valid?
puts "arity-1 lambda stays a gap"
"##)
        .assert_passes();
}

/// A parameter of a block nested in the lambda is bound by that block
/// after splicing. The guard runs; it is not reported as a gap.
const NESTED_BLOCK_MODEL: &str = r#"class Widget < ApplicationRecord
  validates :name, presence: true, if: -> { tags.any? { |tag| tag == "need" } }

  def tags
    [status]
  end
end
"#;

#[test]
fn nested_block_parameter_in_a_lambda_condition_runs() {
    let warnings = emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", NESTED_BLOCK_MODEL)
        .ingest_warnings();
    assert!(warnings.is_empty(), "{warnings:?}");

    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", NESTED_BLOCK_MODEL)
        .run_ruby(r##"
idle = Widget.new(status: "draft", name: "")
raise "nested block guard should skip presence: #{idle.errors.inspect}" unless idle.valid?

need = Widget.new(status: "need", name: "")
raise "nested block guard should require name: #{need.errors.inspect}" if need.valid?
raise "need errors: #{need.errors.inspect}" unless need.errors == ["Name can't be blank"]

filled = Widget.new(status: "need", name: "n")
raise "filled need should be valid: #{filled.errors.inspect}" unless filled.valid?
puts "nested block parameter passed"
"##)
        .assert_passes();
}

/// A lambda that reads a class-body local would splice that name into
/// `def validate`, where it is not defined. The call stays a gap: the
/// attribute saves blank instead of raising NameError on `valid?`.
const CAPTURED_LOCAL_MODEL: &str = r#"class Widget < ApplicationRecord
  required = true
  validates :name, presence: true, if: -> { required }
end
"#;

#[test]
fn captured_local_lambda_condition_stays_a_reported_gap() {
    let warnings = emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", CAPTURED_LOCAL_MODEL)
        .ingest_warnings();
    assert!(
        warnings.iter().any(|m| m.contains("if:")),
        "missing if: gap warning in {warnings:?}"
    );

    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", CAPTURED_LOCAL_MODEL)
        .run_ruby(r##"
live = Widget.new(status: "live", name: "")
raise "captured-local lambda condition should stay a gap (unconditional pass)" unless live.valid?
puts "captured local stays a gap"
"##)
        .assert_passes();
}

/// Every backend target this build ships sees the lowered guard
/// (`validates`'s `if_cond`/`unless_cond`, built the same way a
/// callback's condition is) and emits it cleanly — not just the Ruby
/// target the runtime tests above exercise.
#[test]
fn lambda_conditions_emit_cleanly_on_every_backend_target() {
    use roundhouse::project::BuildTarget::*;
    for target in [
        Ruby, Rust, Typescript, Go, Python, CSharp, Kotlin, Swift, Crystal, Elixir, Roda,
    ] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}
