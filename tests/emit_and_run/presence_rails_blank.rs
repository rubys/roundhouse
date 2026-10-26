use super::emit_and_run;

/// `presence:` and `absence:` with Rails' `blank?`: a String of Unicode
/// whitespace (`[[:space:]]`, so U+3000 and U+00A0 as well as ASCII) is
/// blank, and so is `false` (a boolean column's `false` fails presence),
/// while `0` is not. The check only treated nil and `""` (and an empty
/// Array) as blank. Answers are Rails 8.1.4's for the same model and records.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"items\", force: :cascade do |t|\n    t.string \"name\"\n    t.boolean \"live\"\n    t.integer \"count\"\n    t.string \"gone\"\n  end\nend\n",
        )
        .write(
            "app/models/item.rb",
            "class Item < ApplicationRecord\n  validates :name, presence: true\n  validates :live, presence: true\n  validates :count, presence: true\n  validates :gone, absence: true\nend\n",
        )
}

#[test]
fn presence_and_absence_use_rails_blank() {
    app()
        .run_ruby(
            r#"rows = [
  [nil, nil, nil, nil],
  ["", false, 0, ""],
  ["  ", true, 5, "  "],
  ["　", false, nil, "　"],
  [" ", true, 1, " "],
  ["x", true, 2, "x"],
]
got = rows.map do |name, live, count, gone|
  r = Item.new
  r.name = name
  r.live = live
  r.count = count
  r.gone = gone
  r.valid?
  r.errors.to_a
end
want = [
  ["Name can't be blank", "Live can't be blank", "Count can't be blank"],
  ["Name can't be blank", "Live can't be blank"],
  ["Name can't be blank"],
  ["Name can't be blank", "Live can't be blank", "Count can't be blank"],
  ["Name can't be blank"],
  ["Gone must be blank"],
]
raise "errors: #{got.inspect}" unless got == want
puts "rails blank passed"
"#,
        )
        .assert_passes();
}

#[test]
fn strict_targets_emit_the_blank_test_without_errors() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Go, BuildTarget::Crystal] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}

/// A nullable `date` or `datetime` column keeps its text in
/// `@<attr>_raw`, so presence and absence read that slot: a set date is
/// present and a set time is not absent. Answers are Rails 8.1.4's.
fn temporal_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"tasks\", force: :cascade do |t|\n    t.date \"due_on\"\n    t.datetime \"closed_at\"\n  end\nend\n",
        )
        .write(
            "app/models/task.rb",
            "class Task < ApplicationRecord\n  validates :due_on, presence: true\n  validates :closed_at, absence: true\nend\n",
        )
}

#[test]
fn nullable_temporal_columns_read_their_stored_text() {
    temporal_app()
        .run_ruby(
            r#"rows = [[nil, nil], ["2026-10-21", "2026-10-21 10:00:00"], ["2026-10-21", nil]]
got = rows.map do |due_on, closed_at|
  t = Task.new
  t.due_on = due_on
  t.closed_at = closed_at
  t.valid?
  t.errors.to_a
end
want = [["Due on can't be blank"], ["Closed at must be blank"], []]
raise "errors: #{got.inspect}" unless got == want
puts "temporal presence/absence passed"
"#,
        )
        .assert_passes();
}

/// The same checks on datetime columns only, for the strict targets: the
/// rust target refuses Date-only values outright.
fn datetime_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"tasks\", force: :cascade do |t|\n    t.datetime \"opened_at\"\n    t.datetime \"closed_at\"\n  end\nend\n",
        )
        .write(
            "app/models/task.rb",
            "class Task < ApplicationRecord\n  validates :opened_at, presence: true\n  validates :closed_at, absence: true\nend\n",
        )
}

/// The strict targets bake the same checks; no emit error.
#[test]
fn strict_targets_emit_temporal_presence_and_absence_without_errors() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Go, BuildTarget::Crystal] {
        let (_emitted, errors) = datetime_app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}

/// Elixir dispatches `empty?` by its receiver's type. The strict String
/// arm asks it of `strip`'s result, which is typed as the String it is,
/// so the check reads `String.trim(x) == ""` and never falls back to a
/// struct dispatch (`.__struct__.empty?`), which raised KeyError on a
/// String at runtime.
#[test]
fn elixir_reads_a_stripped_string_as_a_string() {
    use roundhouse::project::BuildTarget;
    let (emitted, errors) = app().emit(BuildTarget::Elixir);
    assert!(errors.is_empty(), "{errors:#?}");
    let mut model = None;
    let mut stack = vec![emitted.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == "item.ex") {
                model = std::fs::read_to_string(&path).ok();
            }
        }
    }
    let model = model.expect("an emitted item.ex");
    assert!(model.contains("String.trim(record.name) == \"\""), "{model}");
    assert!(!model.contains("__struct__.empty?"), "{model}");
}

/// A String column can hold a non-String value on the ruby family (the
/// writer does not cast). Rails 8.1.4 casts it first (`5` is `"5"`,
/// `false` is `"f"`, `[1]` is `"[1]"`), so none is blank: presence
/// passes and absence fails. The whitespace test used to raise
/// TypeError on the regexp for each of them.
#[test]
fn a_non_string_value_on_a_string_column_is_present_like_rails() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"notes\", force: :cascade do |t|\n    t.string \"title\"\n    t.string \"gone\"\n  end\nend\n",
        )
        .write("app/models/note.rb", "class Note < ApplicationRecord\n  validates :title, presence: true\n  validates :gone, absence: true\nend\n")
        .run_ruby(
            r##"got = [5, false, [1], [], true, 0].map do |v|
  n = Note.new
  n.title = v
  n.gone = v
  n.valid?
  n.errors.to_a
end
want = [["Gone must be blank"]] * 6
raise "errors: #{got.inspect}" unless got == want
puts "non-String presence passed"
"##,
        )
        .assert_passes();
}
