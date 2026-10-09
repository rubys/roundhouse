//! Native SQLite contract for schema-backed json and jsonb columns.
//!
//! PostgreSQL type spelling/fidelity is covered by source and DDL tests; this
//! test exercises the shared Ruby JSON column boundary on the compiled target.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// Construct a minimal app with both JSON source types for native execution.
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
  create_table "documents", force: :cascade do |t|
    t.json "payload"
    t.jsonb "settings"
  end
end
"#,
        )
        .write("app/models/document.rb", "class Document < ApplicationRecord\nend\n")
}

/// Compile and run create, read, update and reload through the shared Spinel JSON boundary.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_spinel_json_and_jsonb_columns_round_trip_on_sqlite() {
    let run = app().run_spinel(
        r#"
Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }
ActiveRecord.adapter = SqliteAdapter

payload = {"name" => "Ada", "items" => [1, true, nil]}
settings = {"name" => "Grace", "enabled" => false}
document = Document.create!(payload: payload, settings: settings)
raise "json create was not decoded" unless document.payload == payload
raise "jsonb create was not decoded" unless document.settings == settings

document = Document.find(document.id)
raise "json read was not decoded" unless document.payload == payload
raise "jsonb read was not decoded" unless document.settings == settings

updated_payload = {"name" => "Katherine", "items" => ["updated", false]}
updated_settings = {"name" => "Margaret", "enabled" => true}
document.payload = updated_payload
document.settings = updated_settings
document.save!
document.reload
raise "json update/reload was not decoded" unless document.payload == updated_payload
raise "jsonb update/reload was not decoded" unless document.settings == updated_settings
raise "json value leaked serialized text" if document.payload.is_a?(String)
raise "jsonb value leaked serialized text" if document.settings.is_a?(String)
puts "Spinel json/jsonb SQLite round trip passed"
"#,
    );
    run.assert_passes();
    assert!(run.stdout.contains("Spinel json/jsonb SQLite round trip passed"));
}
