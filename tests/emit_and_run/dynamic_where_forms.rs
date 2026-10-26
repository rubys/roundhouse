use super::emit_and_run;

/// A `widgets` table with an integer `score` and rows 3, 7, 5 and NULL,
/// then `script` with `klass` holding the model: a class held in a
/// variable is the dynamic call site no lowering resolves, so `where`
/// and `find_by` reach the runtime fallbacks and Relation. Every
/// expected value in the scripts below is what Rails 8.1.4 answers on
/// the same rows.
fn run_on_widgets(script: &str) {
    let setup = r##"[3, 7, 5, nil].each do |s|
  w = Widget.new
  w.score = s
  w.save!
end
klass = Widget
def scores(rel)
  rel.to_a.map(&:score).sort_by { |s| s.nil? ? -1 : s }
end
def expect(label, got, want)
  raise "#{label}: got #{got.inspect}, Rails answers #{want.inspect}" unless got == want
end

"##;
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"comments\"",
            "  create_table \"widgets\", force: :cascade do |t|\n    t.integer \"score\"\n  end\n\n  create_table \"comments\"",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .run_ruby(&format!("{setup}{script}"))
        .assert_passes();
}

/// #1129: the class-level `where` fallback forwards every condition
/// form, and `find_by` takes a SQL string, as Rails' do.
#[test]
fn dynamic_where_takes_every_rails_condition_form() {
    run_on_widgets(
        r##"# #1129: the class-level fallback forwards every `where` form.
expect "where(sql)", klass.where("score > 4").count, 2
expect "where(sql, binds)", klass.where("score > ?", 4).count, 2
expect "where(sql, named binds)", klass.where("score > :min", min: 4).count, 2
expect "where(hash)", klass.where(score: 5).count, 1
expect "where(hash) seeds", klass.where(score: 5).find_or_initialize_by(id: -1).score, 5
expect "find_by(sql)", klass.find_by("score > 6").score, 7
expect "find_by(hash)", klass.find_by(score: 3).score, 3
"##,
    );
}

/// #1130: a Range value in a hash condition is Rails' comparison, not a
/// string compared equal to the column.
#[test]
fn dynamic_where_renders_a_range_as_a_comparison() {
    run_on_widgets(
        r##"# #1130: a Range value is a comparison, not a string.
expect "a..b", scores(klass.where(score: 5..9)), [5, 7]
expect "a...b", scores(klass.where(score: 5...7)), [5]
expect "a..", scores(klass.where(score: 5..)), [5, 7]
expect "..b", scores(klass.where(score: ..5)), [3, 5]
expect "...b", scores(klass.where(score: ...5)), [3]
expect "a..a", scores(klass.where(score: 3..3)), [3]
expect "b..a", scores(klass.where(score: 9..5)), []
expect "nil..nil", scores(klass.where(score: nil..nil)), [nil, 3, 5, 7]
expect "a..Infinity", scores(klass.where(score: 5..Float::INFINITY)), [5, 7]
expect "-Infinity..b", scores(klass.where(score: -Float::INFINITY..5)), [3, 5]
expect "Relation a..b", scores(klass.all.where(score: 5..9)), [5, 7]
expect "not a..b", scores(klass.all.not(score: 5..9)), [3]
expect "nested a..b", scores(klass.where(widgets: { score: 5..9 })), [5, 7]
expect "find_by a..b", klass.find_by(score: 6..9).score, 7
expect "range seeds nothing", klass.where(score: 5..9).find_or_initialize_by(id: -1).score, nil

# A Range inside an Array is one more OR'd alternative.
expect "[3, a..b]", scores(klass.where(score: [3, 6..9])), [3, 7]
expect "[nil, a..b]", scores(klass.where(score: [nil, 6..9])), [nil, 7]
expect "[a..b, ..c]", scores(klass.where(score: [6..9, ..3])), [3, 7]
expect "[3, 5, a..b]", scores(klass.where(score: [3, 5, 6..9])), [3, 5, 7]

# Range bounds bind like any other hash-condition value.
sql = klass.where(score: 5...9).to_sql
raise "exclusive range SQL: #{sql}" unless sql.include?("widgets.score >= 5") && sql.include?("widgets.score < 9")
"##,
    );
}
