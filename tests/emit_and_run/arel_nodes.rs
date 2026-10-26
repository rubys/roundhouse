//! `Arel::Nodes` and `Arel.sql` binds render as SQL the relation can run.
//! A node is not a String subclass: `where` and `select` read `to_s`.
//! Named binds quote a scalar and embed a node. A shorter name does not
//! eat a longer one, and a placeholder inside a quoted string stays.

use super::emit_and_run;

#[test]
fn arel_sql_binds_and_nodes_filter_rows() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "notes", force: :cascade do |t|
    t.string "title"
    t.integer "rank"
  end
end
"#,
        )
        .write(
            "app/models/note.rb",
            r#"class Note < ApplicationRecord
  def self.titled(name)
    where(Arel.sql("title = :name", name: name))
  end

  def self.fragments
    inner = Arel.sql("title = :name", name: "Ada")
    [
      Arel.sql("title = :name AND rank > ?", 1, name: "Ada").to_s,
      Arel.sql("rank > 0 AND :scope", scope: inner).to_s,
      Arel.sql("title = :name AND era = :era_boundary", name: "Ada", era_boundary: 3).to_s,
      Arel.sql("title = 'keep :name' AND title = :name", name: "Ada").to_s,
      Arel::Nodes.build_quoted("O'Brien").to_s,
      Arel::Nodes::Not.new(Arel.sql("title = 'x'")).to_s,
      Arel::Nodes::Case.new.when(Arel.sql("title = 'x'")).then(1).else(nil).to_s,
      Arel::Nodes::NamedFunction.new(:COUNT, [Arel.star]).to_s,
      Arel.sql("(title)::text = :title", title: "Ada").to_s,
      Arel.sql("rank = ? AND title = ?", Arel.sql("COALESCE(rank, ?)"), 7).to_s,
    ]
  end
end
"#,
        )
        .run_ruby(
            r#"
Note.create!(title: "Ada", rank: 2)
Note.create!(title: "Bea", rank: 1)
got = Note.titled("Ada").map(&:title)
raise "titled #{got.inspect}" unless got == ["Ada"]
sql = Note.fragments
want = [
  "title = 'Ada' AND rank > 1",
  "rank > 0 AND title = 'Ada'",
  "title = 'Ada' AND era = 3",
  "title = 'keep :name' AND title = 'Ada'",
  "'O''Brien'",
  "NOT (title = 'x')",
  "CASE WHEN title = 'x' THEN 1 ELSE NULL END",
  "COUNT(*)",
  "(title)::text = 'Ada'",
  "rank = COALESCE(rank, ?) AND title = 7",
]
raise "fragments #{sql.inspect}" unless sql == want
begin
  Arel.sql("title = ?", "Ada", "Bea")
  raise "extra bind was accepted"
rescue Arel::BindError
end
begin
  Arel.sql("title = :missing", name: "Ada")
  raise "missing named bind was accepted"
rescue Arel::BindError
end
quoted_case = Arel::Nodes::Case.new.when("title = 'x'").then("high").else(nil).to_s
raise quoted_case unless quoted_case == "CASE WHEN 'title = ''x''' THEN 'high' ELSE NULL END"
ordered = Note.order(Arel.sql("CASE WHEN title = :name THEN 1 ELSE 0 END", name: "Ada")).map(&:title)
raise "ordered #{ordered.inspect}" unless ordered == ["Bea", "Ada"]
seen = []
ActiveSupport::Notifications.subscribe("sql.active_record") { |*args| seen << args[4][:sql] }
Note.count
raise "sql #{seen.inspect}" if seen.empty?
"#,
        )
        .assert_passes();
}
