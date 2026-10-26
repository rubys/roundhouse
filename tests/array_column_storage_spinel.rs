//! A stored PostgreSQL array column on the compiled Spinel target: the
//! JSON text in SQLite, the typed Array the record keeps (`ArrayColumn`),
//! `pluck` decoding it, and an in-place append that the next save writes. The CRuby side,
//! measured against Rails, is `tests/array_column_storage.rs`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "members", force: :cascade do |t|
    t.string "name"
    t.bigint "tag_ids", array: true
    t.string "labels", array: true, default: [], null: false
  end
end
"#,
        )
        .write(
            "app/models/member.rb",
            "class Member < ApplicationRecord\n  def first_label\n    labels.first\n  end\n\n  def tagged?(id)\n    !tag_ids.nil? && tag_ids.include?(id)\n  end\nend\n",
        )
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_spinel_array_columns_round_trip_on_sqlite() {
    let run = app().run_spinel(
        r#"
Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }
ActiveRecord.adapter = SqliteAdapter

fresh = Member.new
raise "nullable array column is not nil" unless fresh.tag_ids.nil?
raise "default [] is not empty" unless fresh.labels == []

member = Member.create!(name: "Ada", tag_ids: [1, 2], labels: ["x", "y"])
member = Member.find(member.id)
raise "integer array did not round-trip" unless member.tag_ids == [1, 2]
raise "string array did not round-trip" unless member.labels == ["x", "y"]
raise "element read" unless member.first_label == "x"
raise "include?" unless member.tagged?(2) && !member.tagged?(3)
raise "pluck" unless Member.where(id: member.id).pluck(:tag_ids) == [[1, 2]]


member.labels << "z"
member.save!
member.reload
raise "in-place append was not saved" unless member.labels == ["x", "y", "z"]

bare = Member.find(Member.create!(name: "Bare").id)
raise "unset column is not nil" unless bare.tag_ids.nil?
raise "unset default is not empty" unless bare.labels == []
puts "Spinel array column SQLite round trip passed"
"#,
    );
    run.assert_passes();
    assert!(run.stdout.contains("Spinel array column SQLite round trip passed"));
}
