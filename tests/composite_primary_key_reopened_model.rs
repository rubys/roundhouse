//! PostgreSQL: composite primary key reopened model, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const STRUCTURE: &str = r#"CREATE TABLE events (
    shop_id bigint NOT NULL,
    id bigint NOT NULL,
    island integer DEFAULT 0 NOT NULL,
    name character varying,
    CONSTRAINT events_pkey PRIMARY KEY (shop_id, id, island)
);
"#;

const DATABASE_YML: &str = "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: rh_probe_reopened_composite\n";

const RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

fn emitted_event(declaring: &str, reopening: &str) -> String {
    let (emitted, _errors) = [
        ("roundhouse.yml", ""),
        ("config/database.yml", DATABASE_YML),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("db/structure.sql", STRUCTURE),
        ("app/models/application_record.rb", RECORD),
        ("app/models/event.rb", declaring),
        ("app/models/event/mentioned.rb", reopening),
    ]
    .iter()
    .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
    .emit(roundhouse::project::BuildTarget::Ruby);
    std::fs::read_to_string(emitted.join("app/models/event.rb")).expect("emitted event")
}

#[test]
fn a_single_key_declaration_survives_a_reopen_without_one() {
    let event = emitted_event(
        "class Event < ApplicationRecord\n  self.primary_key = :id\nend\n",
        "class Event < ApplicationRecord\n  def mentioned? = true\nend\n",
    );
    assert!(!event.contains("def self.composite_primary_key?"), "{event}");
    assert!(!event.contains("[self.shop_id, self.id_value"), "{event}");
}

#[test]
fn a_reopen_may_carry_the_single_key_declaration() {
    let event = emitted_event(
        "class Event < ApplicationRecord\nend\n",
        "class Event < ApplicationRecord\n  self.primary_key = :id\nend\n",
    );
    assert!(!event.contains("def self.composite_primary_key?"), "{event}");
}

/// No declaration anywhere: Rails keys the model on the table's columns.
#[test]
#[ignore = "fork spec: postgresql"]
fn an_undeclared_key_stays_the_tables_composite_across_a_reopen() {
    let event = emitted_event(
        "class Event < ApplicationRecord\nend\n",
        "class Event < ApplicationRecord\n  def mentioned? = true\nend\n",
    );
    assert!(event.contains("def self.composite_primary_key?"), "{event}");
}

/// `to_param` answers the declared key, even when only a reopen declares it.
#[test]
fn to_param_reads_the_key_a_reopen_declares() {
    let event = emitted_event(
        "class Event < ApplicationRecord\nend\n",
        "class Event < ApplicationRecord\n  self.primary_key = :shop_id\nend\n",
    );
    let to_param = event.split("def to_param").nth(1).expect("a to_param");
    let body = to_param.split("\n  end").next().unwrap_or_default();
    assert!(body.contains("@shop_id.to_s"), "{body}");
}
