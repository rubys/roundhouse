//! PostgreSQL: create or find by block, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE charges (
    id bigserial NOT NULL,
    order_id bigint NOT NULL,
    currency character varying NOT NULL,
    note character varying,
    CONSTRAINT charges_pkey PRIMARY KEY (id),
    CONSTRAINT charges_order_id_key UNIQUE (order_id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_create_or_find_by_block
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SERVICE: &str = r#"class ChargeService
  def self.create_or_find(order_id, currency)
    Charge.create_or_find_by!(order_id: order_id) do |charge|
      charge.currency = currency
      charge.note = "made"
    end
  end

  def self.create_or_find_plain(order_id, currency)
    Charge.create_or_find_by(order_id: order_id) do |charge|
      charge.currency = currency
    end
  end

  def self.on_relation(scope, order_id, currency)
    scope.create!(order_id: order_id) do |charge|
      charge.currency = currency
      charge.note = "relation"
    end
  end

  def self.on_relation_plain(scope, order_id, currency)
    scope.create(order_id: order_id) do |charge|
      charge.currency = currency
    end
  end
end
"#;

const PROBE: &str = r##"class CreateOrFindByBlockProbe
  def self.lines
    out = []
    first = ChargeService.create_or_find(1, "USD")
    out << "create: #{first.currency} #{first.note} #{first.persisted?}"
    again = ChargeService.create_or_find(1, "CAD")
    out << "find: #{again.currency} #{again.note} #{again.id == first.id}"
    out << "plain: #{ChargeService.create_or_find_plain(2, "EUR").currency}"
    scope = Charge.where(note: nil)
    out << "relation create!: #{ChargeService.on_relation(Charge.all, 3, "GBP").then { |c| [c.currency, c.note].inspect }}"
    out << "relation create: #{ChargeService.on_relation_plain(Charge.all, 4, "JPY").currency}"
    out << "count: #{Charge.count} #{scope.count}"
    out
  end
end
"##;

const LOADS: [&str; 4] = [
    "app/models/application_record",
    "app/models/charge",
    "app/models/charge_service",
    "app/models/create_or_find_by_block_probe",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/charge.rb", "class Charge < ApplicationRecord\nend\n"),
    ("app/models/charge_service.rb", SERVICE),
    ("app/models/create_or_find_by_block_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_create_block_yields_the_record_before_save_like_native_rails() {
    let (emitted, _errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "create-or-find-by-block",
        &FILES,
        &emitted,
        "rh_spec_create_or_find_by_block",
        "CreateOrFindByBlockProbe",
        &LOADS,
    ) else { return };
    // Not a vacuous agreement: Rails' own answers.
    assert_eq!(native[0], "create: USD made true", "{native:#?}");
    assert_eq!(native[1], "find: USD made true", "{native:#?}");
    assert_eq!(emitted_lines, native, "emitted");
}
