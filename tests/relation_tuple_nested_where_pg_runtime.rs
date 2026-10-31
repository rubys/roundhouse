//! PostgreSQL: relation tuple nested where, on the Ruby target against native Rails.
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
CREATE TABLE gadgets (
    id bigserial NOT NULL,
    widget_id bigint,
    shop_id integer,
    key character varying,
    CONSTRAINT gadgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_relation_tuple_nested
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const WIDGET: &str = "class Widget < ApplicationRecord\n  has_many :gadgets\nend\n";
const GADGET: &str = "class Gadget < ApplicationRecord\n  belongs_to :widget, optional: true\n  belongs_to :part, foreign_key: :widget_id, optional: true\nend\n";
// The same table under a default scope: its join carries the scope's
// conditions, against the alias the where key gives it.
const PART: &str = "class Part < ApplicationRecord\n  self.table_name = \"widgets\"\n  default_scope { where.not(size: 99) }\nend\n";

const PROBE: &str = r##"class RelationTupleNestedProbe
  def self.lines
    out = []
    a = Widget.create!(name: "a", size: 1)
    b = Widget.create!(name: "b", size: 2)
    Gadget.create!(widget_id: a.id, shop_id: 1, key: "x")
    Gadget.create!(widget_id: a.id, shop_id: 2, key: "y")
    Gadget.create!(widget_id: b.id, shop_id: 1, key: "y")
    out << "tuples: #{Gadget.where([:shop_id, :key] => [[1, "x"], [2, "y"]]).order(:id).map(&:id).size}"
    out << "one tuple: #{Gadget.where([:shop_id, :key] => [[1, "y"]]).map { |g| g.widget_id == b.id }.inspect}"
    bad = begin
      Gadget.where([:shop_id, :key] => [1, "y"]).to_a
      "none"
    rescue ArgumentError => e
      "ArgumentError"
    end
    out << "bare tuple: #{bad}"
    out << "tuple not: #{Gadget.where.not([:shop_id, :key] => [[1, "x"]]).count}"
    out << "nested singular: #{Gadget.joins(:widget).where(widget: { name: "a" }).count}"
    out << "nested plural: #{Widget.joins(:gadgets).where(gadgets: { shop_id: 1 }).order(:id).map(&:name).inspect}"
    out << "includes where: #{Gadget.includes(:widget).where(widget: { name: "a" }).order(:id).map { |g| g.widget.name }.inspect}"
    out << "where includes: #{Gadget.where(widgets: { name: "b" }).includes(:widget).order(:id).map { |g| g.widget.name }.inspect}"
    out << "eager_load find_by: #{Gadget.eager_load(:widget).find_by(widget: { name: "b" })&.shop_id.inspect}"
    out << "includes scoped: #{Gadget.includes(:part).where(part: { name: "a" }).order(:id).map { |g| g.part.size }.inspect}"
    out << "includes count: #{Gadget.includes(:widget).where(widget: { name: "a" }).count}"
    out << "nested own: #{Widget.where(widgets: { name: "b" }).map(&:size).inspect}"
    out << "affected: #{ActiveRecord::Base.connection.exec_query("UPDATE widgets SET size = 7 WHERE size < 5").affected_rows}"
    out << "affected none: #{ActiveRecord::Base.connection.exec_query("UPDATE widgets SET size = 7 WHERE size < 0").affected_rows}"
    out
  end
end
"##;

const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/widget",
    "app/models/part",
    "app/models/gadget",
    "app/models/relation_tuple_nested_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/widget.rb", WIDGET),
    ("app/models/part.rb", PART),
    ("app/models/gadget.rb", GADGET),
    ("app/models/relation_tuple_nested_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

#[test]
#[ignore = "fork spec: postgresql"]
fn tuple_and_nested_association_conditions_match_native_rails() {
    let (emitted, _errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "relation-tuple-nested",
        &FILES,
        &emitted,
        "rh_spec_relation_tuple_nested",
        "RelationTupleNestedProbe",
        &LOADS,
    ) else { return };
    // Not a vacuous agreement: Rails' own answers.
    assert_eq!(native[0], "tuples: 2", "{native:#?}");
    assert_eq!(native[1], "one tuple: [true]", "{native:#?}");
    assert_eq!(native[2], "bare tuple: ArgumentError", "{native:#?}");
    assert_eq!(native[3], "tuple not: 2", "{native:#?}");
    assert_eq!(native[4], "nested singular: 2", "{native:#?}");
    assert_eq!(native[6], "includes where: [\"a\", \"a\"]", "{native:#?}");
    assert_eq!(native[9], "includes scoped: [1, 1]", "{native:#?}");
    assert_eq!(native[8], "eager_load find_by: 1", "{native:#?}");
    assert_eq!(native[10], "includes count: 2", "{native:#?}");
    assert_eq!(native[5], "nested plural: [\"a\", \"b\"]", "{native:#?}");
    assert_eq!(native[12], "affected: 2", "{native:#?}");
    assert_eq!(native[13], "affected none: 0", "{native:#?}");
    assert_eq!(emitted_lines, native, "emitted");
}
