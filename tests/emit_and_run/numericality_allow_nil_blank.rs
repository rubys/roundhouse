use super::emit_and_run;

/// `numericality:` with `allow_nil:` (in its own hash, or call-level) and
/// call-level `allow_blank:`. Rails skips the validator for a nil value
/// under allow_nil and for a blank one under allow_blank; the validator's
/// own hash wins over the call-level option (`rank` keeps rejecting nil).
/// These were reported gaps that refused the whole call. Answers are
/// Rails 8.1.4's for the same model and records. The records hold values
/// that need no type cast (Integers, Floats, nil, "", a whitespace-only
/// String and `false`, both blank to Rails): a String number on a typed
/// column is #1158. On an untyped attribute the options stay a gap
/// (tests/numericality_allow_options.rs).
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"policies\", force: :cascade do |t|\n    t.integer \"min_favs\"\n    t.integer \"age\"\n    t.integer \"rank\"\n  end\nend\n",
        )
        .write(
            "app/models/policy.rb",
            r#"class Policy < ApplicationRecord
  validates :min_favs, numericality: { greater_than_or_equal_to: 1, allow_nil: true }
  validates :age, numericality: { only_integer: true }, allow_blank: true
  validates :rank, numericality: { allow_nil: false }, allow_nil: true
end
"#,
        )
}

#[test]
fn numericality_skips_nil_and_blank_like_rails() {
    app()
        .run_ruby(
            r##"rows = [
  { min_favs: nil, age: nil, rank: 1 },
  { min_favs: 5, age: "", rank: 1 },
  { min_favs: 0, age: 3, rank: nil },
  { min_favs: 2, age: 5.5, rank: 1 },
  { min_favs: 2, age: "   ", rank: 1 },
  { min_favs: 2, age: false, rank: 1 },
]
got = rows.map do |attrs|
  p = Policy.new
  attrs.each { |k, v| p.public_send(:"#{k}=", v) }
  p.valid?
  p.errors.to_a
end
want = [[], [], ["Min favs must be greater than or equal to 1", "Rank is not a number"], ["Age must be an integer"], [], []]
raise "policy errors: #{got.inspect}" unless got == want
puts "numericality allow_nil/allow_blank passed"
"##,
        )
        .assert_passes();
}

/// The strict targets emit the skip too, with no emit error. Their numeric
/// field is never a String, so `allow_blank`'s skip is the `nil?` test
/// there: no `is_empty()` on an `i64` field (rustc E0599) the way the
/// presence check's generic blank test would emit it.
#[test]
fn strict_targets_emit_the_skip_without_errors() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Go, BuildTarget::Crystal] {
        let (emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
        if target == BuildTarget::Rust {
            let policy = std::fs::read_to_string(emitted.join("src/models/policy.rs")).expect("policy.rs");
            let validate = &policy[policy.find("pub fn validate(").expect("validate")..];
            let validate = &validate[..validate.find("\n    }\n").unwrap_or(validate.len())];
            assert!(!validate.contains("is_empty"), "{validate}");
            assert!(validate.contains("self.age.is_none()"), "{validate}");
        }
    }
}

/// The Roda/Sequel emit passes the flags to Sequel's validation helpers.
/// Their `allow_nil:` skips nil, and their `allow_blank:` skips Sequel's
/// `blank_object?` values (nil, false, whitespace-only Strings, empty
/// collections), which is Rails' `blank?`. Without the options Sequel
/// validated values Rails skips. `rank`'s own `allow_nil: false` wins, so
/// it carries no option.
#[test]
fn the_roda_emit_passes_allow_nil_and_allow_blank_to_sequel() {
    use roundhouse::project::BuildTarget;
    let (emitted, errors) = app().emit(BuildTarget::Roda);
    assert!(errors.is_empty(), "{errors:#?}");
    let mut model = None;
    let mut stack = vec![emitted.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                if text.contains("class Policy") && text.contains("validates_") {
                    model = Some(text);
                }
            }
        }
    }
    let model = model.expect("an emitted Policy model with Sequel validations");
    for line in [
        "validates_numeric :min_favs, allow_nil: true",
        "validates_operator :>=, 1, :min_favs, allow_nil: true",
        "validates_integer :age, allow_blank: true",
        "validates_numeric :rank\n",
    ] {
        assert!(model.contains(line), "{line:?} in:\n{model}");
    }
}
