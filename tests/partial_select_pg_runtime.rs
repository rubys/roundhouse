//! PostgreSQL: partial select, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE shelves (
    id bigserial NOT NULL,
    CONSTRAINT shelves_pkey PRIMARY KEY (id)
);

CREATE TABLE docs (
    id bigserial NOT NULL,
    shelf_id bigint,
    name character varying,
    kind character varying,
    pages integer,
    CONSTRAINT docs_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_partial_select
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHELF: &str = "class Shelf < ApplicationRecord\n  has_many :docs\nend\n";

const DOC: &str = "class Doc < ApplicationRecord\n  belongs_to :shelf, optional: true\nend\n";

const PROBE: &str = r##"class PartialSelectProbe
  def self.lines
    out = []
    shelf = Shelf.create!
    Doc.create!(shelf: shelf, name: "a", kind: "k", pages: 2)
    Doc.create!(shelf: shelf, name: "a", kind: "k", pages: 3)
    grouped = Doc.group(:shelf_id, :name).select(:shelf_id, :name, "SUM(docs.pages) AS total")
    row = grouped.to_a.first
    out << "grouped: #{row.name} #{row.shelf_id == shelf.id} #{row['total']} #{row.id.inspect}"
    missing = begin
      row.kind.inspect
    rescue NoMethodError => e
      e.class.name
    end
    out << "missing: #{missing} #{row.has_attribute?(:kind)}"
    joined = grouped.left_joins(:shelf).select("MAX(shelves.id) AS top")
    row = joined.to_a.first
    out << "joined: #{row.name} #{row.shelf_id == shelf.id} #{row['total']} #{row['top'] == shelf.id} #{row.id.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 3] = [
    "grouped: a true 5 nil",
    "missing: ActiveModel::MissingAttributeError false",
    "joined: a true 5 true nil",
];

const LOADS: [&str; 4] =
    ["app/models/application_record", "app/models/shelf", "app/models/doc", "app/models/partial_select_probe"];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shelf.rb", SHELF),
    ("app/models/doc.rb", DOC),
    ("app/models/partial_select_probe.rb", PROBE),
    ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

#[test]
#[ignore = "fork spec: postgresql"]
fn partial_select_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "partial-select",
        &FILES,
        &emitted,
        "rh_spec_partial_select",
        "PartialSelectProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    let real: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
