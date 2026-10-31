//! PostgreSQL: sql event names, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_probe_r5_sql_event_names
"#;

const STRUCTURE: &str = "CREATE TABLE gadgets (\n    id bigserial NOT NULL,\n    name character varying,\n    note character varying,\n    stock integer DEFAULT 0,\n    type character varying,\n    created_at timestamp(6) without time zone NOT NULL,\n    updated_at timestamp(6) without time zone NOT NULL,\n    CONSTRAINT gadgets_pkey PRIMARY KEY (id)\n);\n";

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const GADGET: &str = "class Gadget < ApplicationRecord\nend\n";

// An STI subclass saves under its own name (reads that reach the base
// class through a compile-time STI rewrite keep the base name; see the
// task notes).
const SPROCKET: &str = "class Sprocket < Gadget\nend\n";

const PROBE: &str = r##"class SqlEventNamesProbe
  # The `sql.active_record` event names a block issues, schema lookups aside.
  def self.names
    events = []
    sub = ActiveSupport::Notifications.subscribe("sql.active_record") do |*, payload|
      events << payload[:name] unless payload[:name] == "SCHEMA"
    end
    yield
    events
  ensure
    ActiveSupport::Notifications.unsubscribe(sub)
  end

  def self.lines
    out = []
    g = nil
    out << "create: #{names { g = Gadget.create!(name: "a", note: "n", stock: 1) }.inspect}"
    Gadget.create!(name: "b", note: "m", stock: 2)
    id = g.id
    out << "find: #{names { Gadget.find(id) }.inspect}"
    out << "find_by: #{names { Gadget.find_by(name: "a") }.inspect}"
    out << "where to_a: #{names { Gadget.where(name: "a").to_a }.inspect}"
    out << "all to_a: #{names { Gadget.all.to_a }.inspect}"
    out << "first: #{names { Gadget.order(:id).first }.inspect}"
    out << "exists: #{names { Gadget.where(name: "a").exists? }.inspect}"
    out << "exists id: #{names { Gadget.exists?(id) }.inspect}"
    out << "count: #{names { Gadget.where(stock: 1).count }.inspect}"
    out << "sum: #{names { Gadget.all.sum(:stock) }.inspect}"
    out << "maximum: #{names { Gadget.all.maximum(:stock) }.inspect}"
    out << "pluck: #{names { Gadget.where(name: "a").pluck(:name) }.inspect}"
    out << "ids: #{names { Gadget.where(name: "a").ids }.inspect}"
    out << "find_by_sql: #{names { Gadget.find_by_sql("SELECT * FROM gadgets") }.inspect}"
    out << "reload: #{names { g.reload }.inspect}"
    out << "update: #{names { g.update!(name: "z") }.inspect}"
    out << "transaction: #{names { Gadget.transaction { Gadget.count } }.inspect}"
    out << "destroy: #{names { g.destroy }.inspect}"
    out << "sti create: #{names { Sprocket.create!(name: "s", stock: 3) }.inspect}"
    out << "payload: #{payload_shape}"
    out
  end

  # The payload keys and types an event carries.
  def self.payload_shape
    seen = nil
    sub = ActiveSupport::Notifications.subscribe("sql.active_record") { |*, payload| seen ||= payload if payload[:name] == "Gadget Load" }
    Gadget.where(name: "b").to_a
    ActiveSupport::Notifications.unsubscribe(sub)
    [seen[:name], seen[:async], seen[:cached].inspect, seen[:row_count], seen[:binds].class, seen[:connection].nil?].inspect
  end
end
"##;

const EXPECTED: [&str; 20] = [
    "create: [\"TRANSACTION\", \"Gadget Create\", \"TRANSACTION\"]",
    "find: [\"Gadget Load\"]",
    "find_by: [\"Gadget Load\"]",
    "where to_a: [\"Gadget Load\"]",
    "all to_a: [\"Gadget Load\"]",
    "first: [\"Gadget Load\"]",
    "exists: [\"Gadget Exists?\"]",
    "exists id: [\"Gadget Exists?\"]",
    "count: [\"Gadget Count\"]",
    "sum: [\"Gadget Sum\"]",
    "maximum: [\"Gadget Maximum\"]",
    "pluck: [\"Gadget Pluck\"]",
    "ids: [\"Gadget Pluck\"]",
    "find_by_sql: [\"Gadget Load\"]",
    "reload: [\"Gadget Load\"]",
    "update: [\"TRANSACTION\", \"Gadget Update\", \"TRANSACTION\"]",
    "transaction: [\"TRANSACTION\", \"Gadget Count\", \"TRANSACTION\"]",
    "destroy: [\"TRANSACTION\", \"Gadget Destroy\", \"TRANSACTION\"]",
    "sti create: [\"TRANSACTION\", \"Sprocket Create\", \"TRANSACTION\"]",
    "payload: [\"Gadget Load\", false, \"nil\", 1, Array, false]",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/gadget.rb", GADGET),
    ("app/models/sprocket.rb", SPROCKET),
];

const LOADS: [&str; 4] = ["app/models/application_record", "app/models/gadget", "app/models/sprocket", "app/models/sql_event_names_probe"];

fn files() -> Vec<(&'static str, &'static str)> {
    FILES.iter().copied().chain([("app/models/sql_event_names_probe.rb", PROBE)]).collect()
}

fn overlay() -> emit_and_run::Overlay {
    files().iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The emitted model's reads, writes and aggregates hand the driver the
/// name Rails gives them.
#[test]
#[ignore = "fork spec: postgresql"]
fn postgresql_tree_emits_the_query_names_to_the_driver() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let gadget = std::fs::read_to_string(emitted.join("app/models/gadget.rb")).unwrap();
    assert!(gadget.contains("self.class.name + \" Create\")"), "{gadget}");
    assert!(gadget.contains("self.class.name + \" Update\")"), "{gadget}");
    assert!(gadget.contains("self.class.name + \" Destroy\")"), "{gadget}");
    assert!(gadget.contains("\"Gadget Load\")"), "{gadget}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn sql_event_names_match_native_rails() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "sql-event-names",
        &files(),
        &emitted,
        "rh_spec_probe_r5_sql_event_names",
        "SqlEventNamesProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
