use super::emit_and_run;

/// A literal String `message:` in each validator's own hash, on CRuby.
/// Answers are Rails 8.1.4's for the same model and records: the String
/// replaces the default (`%{value}`, `%{model}`, `%{attribute}`
/// interpolated), the full message stays "Attr <message>", and a rule
/// without its own message keeps the default (`name`'s length).
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"code\"\n    t.string \"name\"\n    t.string \"kind\"\n    t.integer \"score\"\n    t.string \"title\"\n    t.string \"slug\"\n    t.string \"note\"\n  end\nend\n",
        )
        .write(
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  validates :code, format: { with: /\A[A-Z0-9]+\z/, message: "is not a valid code (%{value})" }
  validates :name, presence: { message: "must be given" }, length: { maximum: 3 }
  validates :kind, inclusion: { in: %w[a b], message: "%{value} is not a kind" }
  validates :score, numericality: { greater_than: 1, message: "must exceed the minimum" }
  validates :title, exclusion: { in: %w[admin], message: "is reserved for %{model}" }
  validates :slug, uniqueness: { message: "is used by %{attribute}" }
  validates :note, presence: { message: "is 100%% needed, not %%{shape}" }
end
"#,
        )
}

#[test]
fn custom_messages_render_like_rails() {
    app()
        .run_ruby(
            r##"seed = Widget.new
{ code: "A", name: "x", kind: "a", score: 5, title: "t", slug: "s", note: "n" }.each { |k, v| seed.public_send(:"#{k}=", v) }
seed.save!
rows = [
  { code: "a-1", name: "", kind: "z", score: 0, title: "admin", slug: "s", note: "" },
  { code: "OK", name: "long", kind: "b", score: nil, title: "x", slug: "n", note: "n" },
  { code: "OK", name: "ok", kind: "b", score: 3, title: "x", slug: "n", note: "n" },
]
got = rows.map do |attrs|
  w = Widget.new
  attrs.each { |k, v| w.public_send(:"#{k}=", v) }
  w.valid?
  w.errors.to_a
end
want = [
  ["Code is not a valid code (a-1)", "Name must be given", "Kind z is not a kind", "Score must exceed the minimum", "Title is reserved for Widget", "Slug is used by Slug", "Note is 100% needed, not %{shape}"],
  ["Name is too long (maximum is 3 characters)", "Score must exceed the minimum"],
  [],
]
raise "errors: #{got.inspect}" unless got == want
puts "custom messages passed"
"##,
        )
        .assert_passes();
}

/// The strict targets bake the same text; no emit error.
#[test]
fn strict_targets_emit_custom_messages_without_errors() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Go, BuildTarget::Crystal] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}

/// The Roda/Sequel emit passes a plain `message:` to Sequel's helpers. One
/// that interpolates `%{…}` is not emitted (Sequel interpolates none of
/// Rails' names), and says so, the way a guarded validation does.
#[test]
fn the_roda_emit_passes_a_plain_message_to_sequel() {
    use roundhouse::project::BuildTarget;
    let (emitted, errors) = emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"phone\"\n    t.string \"name\"\n    t.integer \"score\"\n    t.string \"code\"\n  end\nend\n",
        )
        .write(
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  validates :phone, format: { with: /\A\+?\d+\z/, message: "only allows numbers" }
  validates :title, presence: { message: "is 100%% needed" }
  validates :name, presence: { message: "must be given" }
  validates :score, numericality: { greater_than: 1, message: "must exceed the minimum" }
  validates :code, format: { with: /\A[A-Z]+\z/, message: "is not a valid code (%{value})" }
end
"#,
        )
        .emit(BuildTarget::Roda);
    assert!(errors.is_empty(), "{errors:#?}");
    let mut model = None;
    let mut stack = vec![emitted.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                if text.contains("class Widget") && text.contains("validates_") {
                    model = Some(text);
                }
            }
        }
    }
    let model = model.expect("an emitted Widget model with Sequel validations");
    for line in [
        r#", :phone, message: "only allows numbers""#,
        r#"validates_presence :name, message: "must be given""#,
        r#"validates_presence :title, message: "is 100% needed""#,
        r#"validates_numeric :score, message: "must exceed the minimum""#,
        r#"validates_operator :>, 1, :score, message: "must exceed the minimum""#,
        "ROUNDHOUSE-TODO: validation on :code with an interpolated message not emitted",
    ] {
        assert!(model.contains(line), "{line:?} in:\n{model}");
    }
}

/// A single-quoted message holding `#{…}` is literal text: Rails 8.1.4
/// renders "Name has #{x} literally". The emitted Ruby (the model's check
/// and the Roda/Sequel `message:`) must not turn it into an interpolation,
/// which would raise NameError or print other text.
fn literal_interpolation_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  validates :name, presence: { message: 'has #{x} literally' }\nend\n")
}

#[test]
fn a_literal_interpolation_in_a_message_stays_literal() {
    literal_interpolation_app()
        .run_ruby(
            r##"w = Widget.new
w.valid?
raise "errors: #{w.errors.to_a.inspect}" unless w.errors.to_a == ["Name has \#{x} literally"]
puts "literal message passed"
"##,
        )
        .assert_passes();
    let (emitted, errors) = literal_interpolation_app().emit(roundhouse::project::BuildTarget::Roda);
    assert!(errors.is_empty(), "{errors:#?}");
    let mut model = None;
    let mut stack = vec![emitted.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                if text.contains("class Widget") && text.contains("validates_") {
                    model = Some(text);
                }
            }
        }
    }
    let model = model.expect("an emitted Widget model");
    assert!(model.contains(r#"validates_presence :name, message: "has \#{x} literally""#), "{model}");
}
