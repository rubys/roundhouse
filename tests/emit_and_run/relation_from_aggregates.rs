use super::emit_and_run;

/// Issue #1102: `maximum` / `minimum` / `sum` on a relation with a
/// custom `from(...)` source. Rails 8.1 (`Calculations#aggregate_column`)
/// qualifies the column with the model's table only when the source
/// names that table (`table_name_matches?`), and leaves it bare
/// otherwise, so a subquery source aggregates its own columns:
///
///   Widget.from("(SELECT * FROM widgets WHERE score > 4) AS hi").maximum(:score)
///   # SELECT MAX(score) FROM (SELECT * FROM widgets WHERE score > 4) AS hi  => 7
///
/// An aliased table source (`from("widgets AS recent")`) still names the
/// table, so Rails qualifies with it and the query raises
/// `StatementInvalid` ("no such column: widgets.score"): the build must
/// raise there too, not answer. A `with_recursive(...)` CTE read through
/// `from("name")` is such a source too, so `sum` must keep the WITH
/// clause. Answers below are Rails 8.1.4's on the same rows (scores 3,
/// 7, 5, each the previous one's child).
const MODEL: &str = "class Widget < ApplicationRecord\nend\n";

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.integer "score"
    t.integer "parent_id"
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", MODEL)
}

const ASSERTIONS: &str = r#"w1 = Widget.create!(score: 3)
w2 = Widget.create!(score: 7, parent_id: w1.id)
w3 = Widget.create!(score: 5, parent_id: w2.id)
hi = "(SELECT * FROM widgets WHERE score > 4) AS hi"
got = {
  plain_max: Widget.maximum(:score),
  plain_min: Widget.minimum(:score),
  plain_sum: Widget.sum(:score).to_i,
  sub_max: Widget.from(hi).maximum(:score),
  sub_min: Widget.from(hi).minimum(:score),
  sub_sum: Widget.from(hi).sum(:score).to_i,
  sub_bare_alias_sum: Widget.from("(SELECT * FROM widgets WHERE score > 4) hi").sum(:score).to_i,
  cte_sum: Widget.with_recursive(hi: [Widget.all.where("score > 4")]).from("hi").sum(:score).to_i,
  cte_max: Widget.with_recursive(hi: [Widget.all.where("score > 4")]).from("hi").maximum(:score),
  ancestors_sum: Widget.with_recursive(parents: [Widget.where(id: w3.id), Widget.joins("JOIN parents ON widgets.id = parents.parent_id")]).from("parents").sum(:score).to_i,
}
want = { plain_max: 7, plain_min: 3, plain_sum: 15, sub_max: 7, sub_min: 5, sub_sum: 12, sub_bare_alias_sum: 12, cte_sum: 12, cte_max: 7, ancestors_sum: 15 }
raise "aggregates: #{got.inspect}" unless got == want
aliased = Widget.from("widgets AS recent")
begin
  aliased.maximum(:score)
  raise "aliased maximum answered; Rails raises StatementInvalid"
rescue ActiveRecord::StatementInvalid
end
begin
  aliased.minimum(:score)
  raise "aliased minimum answered; Rails raises StatementInvalid"
rescue ActiveRecord::StatementInvalid
end
begin
  aliased.sum(:score)
  raise "aliased sum answered; Rails raises StatementInvalid"
rescue ActiveRecord::StatementInvalid
end
puts "from aggregates passed"
"#;

#[test]
fn aggregates_over_a_custom_from_source_match_rails() {
    app().run_ruby(ASSERTIONS).assert_passes();
}
