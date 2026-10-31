//! PostgreSQL: counters and scope attributes, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE owners (
    id bigint NOT NULL,
    name character varying NOT NULL,
    CONSTRAINT owners_pkey PRIMARY KEY (id)
);

CREATE TABLE snapshots (
    id bigint NOT NULL,
    owner_id bigint NOT NULL,
    island integer NOT NULL,
    label character varying NOT NULL,
    CONSTRAINT snapshots_pkey PRIMARY KEY (id)
);

CREATE TABLE widgets (
    id bigint NOT NULL,
    name character varying NOT NULL,
    hits integer,
    misses integer DEFAULT 0 NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    touched_at timestamp(6) without time zone,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_counters_scope
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const ISLAND_RECORD: &str = r#"class IslandRecord < ApplicationRecord
  self.abstract_class = true
  default_scope { where(island: 0) }
end
"#;

const OWNER: &str = "class Owner < ApplicationRecord\n  has_many :snapshots\nend\n";
const SNAPSHOT: &str = "class Snapshot < IslandRecord\n  belongs_to :owner\nend\n";
const WIDGET: &str = "class Widget < ApplicationRecord\nend\n";

const PROBE: &str = r##"class CountersScopeProbe
  OLD = Time.utc(2020, 1, 1)

  def self.widget(id, hits)
    Widget.create!(id: id, name: "w#{id}", hits: hits, created_at: OLD, updated_at: OLD)
  end

  def self.row(id, label)
    { id: id, label: label }
  end

  def self.lines
    out = []

    owner = Owner.create!(id: 1, name: "o")
    owner.snapshots.insert_all!([row(10, "a"), row(11, "b")])
    out << "snapshots: #{Snapshot.order(:id).pluck(:id, :owner_id, :island).inspect}"
    owner.snapshots.insert_all!([{ id: 12, label: "c", island: 7 }])
    out << "scope wins: #{Snapshot.unscoped.where(id: 12).pluck(:island).inspect}"

    w = widget(1, nil)
    out << "update_counters: #{Widget.update_counters(1, hits: 2)}"
    out << "null counted as 0: #{Widget.find(1).hits}"
    Widget.update_counters(1, hits: 3, misses: -1)
    out << "two columns: #{Widget.find(1).attributes.values_at("hits", "misses").inspect}"
    out << "no touch: #{Widget.find(1).updated_at.year}"
    Widget.update_counters(1, hits: 1, touch: true)
    out << "touch: true: #{Widget.find(1).updated_at.year > 2020}"
    Widget.update_counters(1, hits: 1, touch: :touched_at)
    t = Widget.find(1)
    out << "touch: :col: #{t.touched_at.nil? ? "nil" : t.touched_at.year > 2020} #{t.updated_at.year > 2020}"

    Widget.increment_counter(:hits, 1, by: 10)
    Widget.decrement_counter(:hits, 1)
    out << "increment/decrement_counter: #{Widget.find(1).hits}"
    out << "relation: #{Widget.where(name: "w1").update_counters(hits: -4, touch: true)} #{Widget.find(1).hits}"
    out << "no rows: #{Widget.update_counters(999, hits: 1)}"

    w = Widget.find(1)
    out << "increment returns self: #{w.increment(:hits, 2).equal?(w)} #{w.hits} #{w.changed?}"
    w = Widget.find(1)
    w.increment!(:hits, 5)
    out << "increment!: #{w.hits} #{w.changed?} #{Widget.find(1).hits}"
    w.decrement!(:hits)
    out << "decrement!: #{w.hits} #{Widget.find(1).hits}"
    stale = Widget.find(1)
    Widget.increment_counter(:hits, 1, by: 100)
    stale.increment!(:hits)
    out << "concurrent bump kept: #{stale.hits} #{Widget.find(1).hits}"
    fresh = widget(2, nil)
    fresh.increment!(:hits, 4, touch: true)
    out << "null increment!: #{Widget.find(2).hits} #{Widget.find(2).updated_at.year > 2020}"
    out
  end
end
"##;

const EXPECTED: [&str; 16] = [
    "snapshots: [[10, 1, 0], [11, 1, 0]]",
    "scope wins: [0]",
    "update_counters: 1",
    "null counted as 0: 2",
    "two columns: [5, -1]",
    "no touch: 2020",
    "touch: true: true",
    "touch: :col: true true",
    "increment/decrement_counter: 16",
    "relation: 1 12",
    "no rows: 0",
    "increment returns self: true 14 true",
    "increment!: 17 false 17",
    "decrement!: 16 16",
    "concurrent bump kept: 17 117",
    "null increment!: 4 true",
];

const LOADS: [&str; 6] = [
    "app/models/application_record",
    "app/models/island_record",
    "app/models/owner",
    "app/models/snapshot",
    "app/models/widget",
    "app/models/counters_scope_probe",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/island_record.rb", ISLAND_RECORD),
    ("app/models/owner.rb", OWNER),
    ("app/models/snapshot.rb", SNAPSHOT),
    ("app/models/widget.rb", WIDGET),
    ("app/models/counters_scope_probe.rb", PROBE),
];

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn counter_writes_and_relation_scope_attributes_run_like_native_rails_on_postgresql() {
    let files = FILES.to_vec();
    let overlay = files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "counters-scope",
        &files,
        &emitted,
        "rh_spec_counters_scope",
        "CountersScopeProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
