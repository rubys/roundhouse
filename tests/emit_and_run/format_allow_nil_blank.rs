use super::emit_and_run;

/// `format:` with `allow_nil:` (in its own hash, or call-level, or
/// `validates_format_of`'s) and call-level `allow_blank:`. Rails skips the
/// validator for a nil value under allow_nil and for a blank one under
/// allow_blank; the validator's own hash wins over the call-level option
/// (`tag` keeps rejecting nil, as `nil.to_s` does not match). These were
/// reported gaps that refused the whole call. Answers are Rails 8.1.4's
/// for the same model and records.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"handles\", force: :cascade do |t|\n    t.string \"slug\"\n    t.string \"code\"\n    t.string \"tag\"\n    t.string \"site\"\n  end\nend\n",
        )
        .write(
            "app/models/handle.rb",
            r#"class Handle < ApplicationRecord
  validates :slug, format: { with: /\A[a-z]+\z/, allow_nil: true }
  validates :code, format: { with: /\A[A-Z]{3}\z/ }, allow_blank: true
  validates :tag, format: { with: /\A#\w+\z/, allow_nil: false }, allow_nil: true
  validates_format_of :site, with: /\Ahttps:/, allow_nil: true
end
"#,
        )
}

#[test]
fn format_skips_nil_and_blank_like_rails() {
    app()
        .run_ruby(
            r##"rows = [
  { slug: nil, code: nil, tag: "#a", site: nil },
  { slug: "", code: "", tag: "#a", site: "x" },
  { slug: "ab", code: "   ", tag: nil, site: "https://a" },
  { slug: "Ab", code: "ABC", tag: "#b", site: nil },
  { slug: "ab", code: "ab", tag: "#c", site: nil },
  { slug: "ab", code: " \t\n", tag: "#c", site: nil },
]
got = rows.map do |attrs|
  h = Handle.new
  attrs.each { |k, v| h.public_send(:"#{k}=", v) }
  h.valid?
  h.errors.to_a
end
want = [[], ["Slug is invalid", "Site is invalid"], ["Tag is invalid"], ["Slug is invalid"], ["Code is invalid"], []]
raise "handle errors: #{got.inspect}" unless got == want
puts "format allow_nil/allow_blank passed"
"##,
        )
        .assert_passes();
}

/// The strict targets emit the skip too, with no emit error. A String
/// column's blank test there is the narrowed value's `strip.empty?`.
#[test]
fn strict_targets_emit_the_skip_without_errors() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Go, BuildTarget::Crystal] {
        let (emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
        if target == BuildTarget::Rust {
            let handle = std::fs::read_to_string(emitted.join("src/models/handle.rs")).expect("handle.rs");
            let validate = &handle[handle.find("pub fn validate(").expect("validate")..];
            let validate = &validate[..validate.find("\n    }\n").unwrap_or(validate.len())];
            assert!(validate.contains("self.slug.is_none()"), "{validate}");
            assert!(validate.contains("trim()"), "{validate}");
        }
    }
}

/// The Roda/Sequel emit passes the flags to `validates_format`. Its
/// `allow_nil:` skips nil, and its `allow_blank:` skips Sequel's
/// `blank_object?` values, which is Rails' `blank?`. `tag`'s own
/// `allow_nil: false` wins, so it carries no option.
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
                if text.contains("class Handle") && text.contains("validates_") {
                    model = Some(text);
                }
            }
        }
    }
    let model = model.expect("an emitted Handle model with Sequel validations");
    for line in [
        r"validates_format /\A[a-z]+\z/, :slug, allow_nil: true",
        r"validates_format /\A[A-Z]{3}\z/, :code, allow_blank: true",
        "validates_format /\\A#\\w+\\z/, :tag\n",
        r"validates_format /\Ahttps:/, :site, allow_nil: true",
    ] {
        assert!(model.contains(line), "{line:?} in:\n{model}");
    }
}

/// A Boolean column's `false` is blank to Rails (`false.blank?`), so
/// `allow_blank:` skips the format check for it; `allow_nil:` does not.
/// Rails 8.1.4: `on` (allow_blank) passes with `false` and nil, `off`
/// (allow_nil) fails with `false`. On a strict target the field is a
/// Boolean, never a String, so its blank test is the narrowed `== false`
/// beside `nil?`.
fn flags() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"flags\", force: :cascade do |t|\n    t.boolean \"on\"\n    t.boolean \"off\"\n  end\nend\n",
        )
        .write(
            "app/models/flag.rb",
            "class Flag < ApplicationRecord\n  validates :on, format: { with: /\\Atrue\\z/ }, allow_blank: true\n  validates :off, format: { with: /\\Atrue\\z/ }, allow_nil: true\nend\n",
        )
}

#[test]
fn a_boolean_false_is_blank_like_rails() {
    flags()
        .run_ruby(
            r##"rows = [{ on: false, off: false }, { on: nil, off: nil }]
got = rows.map do |attrs|
  f = Flag.new
  attrs.each { |k, v| f.public_send(:"#{k}=", v) }
  f.valid?
  f.errors.to_a
end
want = [["Off is invalid"], []]
raise "flag errors: #{got.inspect}" unless got == want
puts "boolean allow_blank passed"
"##,
        )
        .assert_passes();
}

#[test]
fn strict_targets_skip_a_boolean_false_under_allow_blank() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Go, BuildTarget::Crystal] {
        let (emitted, errors) = flags().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
        if target == BuildTarget::Rust {
            let flag = std::fs::read_to_string(emitted.join("src/models/flag.rs")).expect("flag.rs");
            let validate = &flag[flag.find("pub fn validate(").expect("validate")..];
            let validate = &validate[..validate.find("\n    }\n").unwrap_or(validate.len())];
            let on = validate.lines().find(|l| l.contains("self.on.is_none()")).expect("the on skip");
            assert!(on.contains("false"), "{on}");
            assert!(!validate.lines().any(|l| l.contains("self.off.is_none()") && l.contains("false")), "{validate}");
        }
    }
}
