use super::emit_and_run;

/// Rails' `FormatValidator` matches `value.to_s`, so a non-String value
/// is checked by its String form (`true` passes `/\Atrue\z/`, `123`
/// passes `/\A\d{3}\z/`) and nil is checked as `""` (it passes a
/// pattern that matches the empty String). The check used to require
/// `is_a?(String)`, which rejected all of these. Answers are Rails
/// 8.1.4's for the same model and records.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"gadgets\", force: :cascade do |t|\n    t.boolean \"on\"\n    t.integer \"num\"\n    t.string \"opt\"\n    t.string \"req\"\n  end\nend\n",
        )
        .write(
            "app/models/gadget.rb",
            r#"class Gadget < ApplicationRecord
  validates :on, format: { with: /\Atrue\z/ }
  validates :num, format: { with: /\A\d{3}\z/ }
  validates :opt, format: { with: /\A[a-z]*\z/ }
  validates :req, format: { with: /\A[a-z]+\z/ }
end
"#,
        )
}

const SCRIPT: &str = r##"def errors_for(on, num, opt, req)
  g = Gadget.new
  g.on = on
  g.num = num
  g.opt = opt
  g.req = req
  g.valid?
  g.errors.to_a
end
got = [
  errors_for(true, 123, nil, "ab"),
  errors_for(false, 12, "AB", nil),
  errors_for(nil, nil, "", ""),
]
want = [
  [],
  ["On is invalid", "Num is invalid", "Opt is invalid", "Req is invalid"],
  ["On is invalid", "Num is invalid", "Req is invalid"],
]
raise "gadget errors: #{got.inspect}" unless got == want
puts "format value.to_s passed"
"##;

#[test]
fn format_matches_the_value_as_a_string_like_rails() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("format value.to_s passed"), "{}", run.stdout);
}

/// The strict targets emit the String form with no emit error. A String
/// column matches its narrowed value, and nil matches `""`.
#[test]
fn strict_targets_emit_the_string_form_without_errors() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Go, BuildTarget::Crystal] {
        let (emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
        if target == BuildTarget::Rust {
            let gadget = std::fs::read_to_string(emitted.join("src/models/gadget.rs")).expect("gadget.rs");
            let validate = &gadget[gadget.find("pub fn validate(").expect("validate")..];
            let validate = &validate[..validate.find("\n    }\n").unwrap_or(validate.len())];
            assert!(validate.contains("self.num.is_none()"), "{validate}");
            assert!(validate.contains(".to_string()"), "{validate}");
        }
    }
}
