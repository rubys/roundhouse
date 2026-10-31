//! PostgreSQL: active record noop save, on the Ruby target against native Rails.
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
  database: rh_spec_probe_r4_setup_mutations
"#;

const STRUCTURE: &str = "CREATE TABLE gadgets (\n    id bigserial NOT NULL,\n    name character varying,\n    note character varying,\n    stock integer DEFAULT 0,\n    created_at timestamp(6) without time zone NOT NULL,\n    updated_at timestamp(6) without time zone NOT NULL,\n    CONSTRAINT gadgets_pkey PRIMARY KEY (id)\n);\n";

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const GADGET: &str = r##"class Gadget < ApplicationRecord
  before_save { trail << "before_save" }
  before_update { trail << "before_update" }
  after_update { trail << "after_update" }
  after_save { trail << "after_save" }

  def trail
    @trail ||= []
  end
end
"##;

const PROBE: &str = r##"class NoopSaveProbe
  def self.updates
    @updates ||= []
  end

  # The columns each UPDATE sets, from its SET clause.
  def self.set_columns(sql)
    sql[/\AUPDATE "gadgets" SET (.*) WHERE/m, 1].to_s.scan(/"(\w+)" =/).flatten.sort
  end

  def self.lines
    out = []
    ActiveSupport::Notifications.subscribe("sql.active_record") do |*, payload|
      updates << set_columns(payload[:sql].to_s) if payload[:sql].to_s.start_with?("UPDATE")
    end
    g = Gadget.create!(name: "a", note: "n", stock: 1)
    created_at = g.reload.updated_at
    updates.clear

    g = Gadget.find(g.id)
    g.trail.clear
    out << "save: #{g.save} #{updates.size} #{g.trail.inspect} #{g.saved_changes.to_h.inspect} #{g.saved_changes?} #{g.updated_at == created_at}"

    g.update!(name: "a")
    out << "same value: #{updates.size} #{g.saved_changes.to_h.inspect}"

    g.name = "a"
    g.note = "n"
    out << "reassigned: #{g.changed?} #{g.save} #{updates.size}"

    g.name = "b"
    out << "changed: #{g.changed.inspect}"
    g.save!
    out << "after change: #{updates.size} #{updates.last.inspect} #{g.saved_changes.keys.sort.inspect} #{g.updated_at == created_at}"

    updates.clear
    g.save!
    out << "again: #{updates.size} #{g.saved_changes.to_h.inspect} #{g.changed?} #{g.previous_changes.to_h.inspect}"

    g.update!(stock: 2, note: "m")
    out << "two columns: #{updates.last.inspect} #{g.saved_changes.keys.sort.inspect}"

    updates.clear
    fresh = Gadget.find(g.id)
    fresh.touch
    out << "touch: #{updates.size} #{updates.last.inspect}"
    out << "reloaded: #{Gadget.find(g.id).attributes.slice("name", "note", "stock").inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 9] = [
    "save: true 0 [\"before_save\", \"before_update\", \"after_update\", \"after_save\"] {} false true",
    "same value: 0 {}",
    "reassigned: false true 0",
    "changed: [\"name\"]",
    "after change: 1 [\"name\", \"updated_at\"] [\"name\", \"updated_at\"] false",
    "again: 0 {} false {}",
    "two columns: [\"note\", \"stock\", \"updated_at\"] [\"note\", \"stock\", \"updated_at\"]",
    "touch: 1 [\"updated_at\"]",
    "reloaded: {\"name\" => \"b\", \"note\" => \"m\", \"stock\" => 2}",
];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/gadget.rb", GADGET),
];

const LOADS: [&str; 3] = ["app/models/application_record", "app/models/gadget", "app/models/noop_save_probe"];

fn files() -> Vec<(&'static str, &'static str)> {
    FILES.iter().copied().chain([("app/models/noop_save_probe.rb", PROBE)]).collect()
}

fn overlay() -> emit_and_run::Overlay {
    files().iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The save path skips the write when nothing is pending, and the
/// emitted UPDATE names only the changed columns.
#[test]
#[ignore = "fork spec: postgresql layout"]
fn postgresql_tree_skips_the_update_of_an_unchanged_record() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let callbacks = std::fs::read_to_string(emitted.join("runtime/active_record_callbacks.rb")).unwrap();
    assert!(
        callbacks.contains("if has_changes_to_save?\n                  fill_timestamps(false)\n                  _runtime_update_changed"),
        "the update chain writes only a record with changes:\n{callbacks}"
    );
    let gadget = std::fs::read_to_string(emitted.join("app/models/gadget.rb")).unwrap();
    assert!(gadget.contains("def _adapter_update_columns(columns)"), "{gadget}");
    assert!(gadget.contains("columns.include?(\"name\")"), "{gadget}");
    assert!(gadget.contains("def _adapter_update\n"), "the whole-row write stays for update_columns: {gadget}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn noop_and_partial_saves_match_native_rails() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "noop-save",
        &files(),
        &emitted,
        "rh_spec_probe_r4_setup_mutations",
        "NoopSaveProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
