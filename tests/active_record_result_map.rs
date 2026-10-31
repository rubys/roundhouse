#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const CONTRACT: &str = r#"
first = Entry.create!(session_id: 7, key: "old")
ids = Entry.capture([{session_id: 7, key: "old"}, {session_id: 7, key: "new"}])
raise "inserted IDs" unless ids.length == 1 && ids[0].is_a?(Integer) && ids[0] != first.id
raise "row count" unless Entry.count == 2
empty = Entry.capture([{session_id: 7, key: "old"}])
raise "empty result" unless empty == []
rows = ActiveRecord::Result.new([{"id" => 3}, {"id" => 5}])
raise "map values" unless rows.map { |row| Integer(row.fetch("id")) * 2 } == [6, 10]
raise "map changed rows" unless rows.to_a == [{"id" => 3}, {"id" => 5}]
updated = Entry.upsert_all([{session_id: 7, key: "new"}], unique_by: %i[session_id key]).map { |row| Integer(row.fetch("id")) }
raise "upsert result" unless updated == []
puts "result map passed"
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n")
        .write("app/models/entry.rb", "class Entry < ApplicationRecord\n  def self.capture(rows)\n    insert_all(rows, unique_by: %i[session_id key], returning: :id).map { |row| Integer(row.fetch(\"id\")) }\n  end\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :entries do |t|\n    t.integer :session_id, null: false\n    t.string :key, null: false\n    t.index [:session_id, :key], unique: true\n  end\nend\n")
}

#[test]
fn insert_all_result_maps_inserted_row_hashes() {
    app().run_ruby(CONTRACT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn insert_all_result_maps_with_native_rbs() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{CONTRACT}"
    );
    app().run_spinel_with_rbs(&script).assert_passes();
}
