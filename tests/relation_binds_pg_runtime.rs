//! PostgreSQL: relation binds, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE widgets (
    id bigserial NOT NULL,
    name character varying,
    size integer,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_relation_binds
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const WIDGET: &str = "class Widget < ApplicationRecord\nend\n";

const PROBE: &str = r##"class RelationBindsProbe
  def self.lines
    out = []
    odd = "o\u0001d\u0002d'x?"
    a = Widget.create!(name: "a", size: 1)
    b = Widget.create!(name: "b'b", size: 2)
    c = Widget.create!(name: odd, size: 3)
    seen = []
    listener = ActiveSupport::Notifications.subscribe("sql.active_record") { |*args| seen << args.last[:sql] }
    out << "name: #{Widget.where(name: "a").map(&:size).inspect}"
    out << "quote: #{Widget.where(name: "b'b").map(&:size).inspect}"
    out << "marks: #{Widget.where(name: odd).map(&:size).inspect}"
    out << "int: #{Widget.where(size: 2).map(&:name).inspect}"
    out << "or: #{Widget.where(name: "a").or(Widget.where(size: 3)).order(:id).map(&:size).inspect}"
    out << "not: #{Widget.where.not(name: "a").order(:id).map(&:size).inspect}"
    out << "count: #{Widget.where(name: "a").count} #{Widget.where(size: 9).exists?}"
    out << "find: #{Widget.find(b.id).name}"
    out << "update_all: #{Widget.where(name: "a").update_all(size: 10)} #{Widget.find(a.id).size}"
    out << "delete_all: #{Widget.where(name: "b'b").delete_all} #{Widget.count}"
    ActiveSupport::Notifications.unsubscribe(listener)
    # A bound read, Rails' shape. The text never carries a mark; only the
    # literal that held the mark bytes itself (`odd`, unmarked) may.
    out << "no marks sent: #{seen.reject { |sql| sql.include?("o\u0001d") }.none? { |sql| sql.include?("\u0001") }}"
    out << "bound: #{seen.any? { |sql| sql.include?("\"name\" = $1") }}"
    out
  end
end
"##;

const LOADS: [&str; 3] = ["app/models/application_record", "app/models/widget", "app/models/relation_binds_probe"];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/widget.rb", WIDGET),
    ("app/models/relation_binds_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

#[test]
#[ignore = "fork spec: postgresql"]
fn relation_hash_conditions_bind_on_postgresql_like_native_rails() {
    let (emitted, _errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "relation-binds",
        &FILES,
        &emitted,
        "rh_spec_relation_binds",
        "RelationBindsProbe",
        &LOADS,
    ) else { return };
    // Not a vacuous agreement: Rails' own answers.
    assert_eq!(native[0], "name: [1]", "{native:#?}");
    assert_eq!(native[2], "marks: [3]", "{native:#?}");
    assert_eq!(native[9], "delete_all: 1 2", "{native:#?}");
    assert_eq!(native[10], "no marks sent: true", "{native:#?}");
    assert_eq!(native[11], "bound: true", "{native:#?}");
    assert_eq!(emitted_lines, native, "emitted");
}
