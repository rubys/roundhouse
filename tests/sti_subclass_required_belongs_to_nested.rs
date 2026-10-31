//! PostgreSQL: sti subclass required belongs to nested, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE labels (
    id bigserial NOT NULL,
    type character varying,
    name character varying,
    destination_address_id bigint,
    CONSTRAINT labels_pkey PRIMARY KEY (id)
);
CREATE TABLE label_addresses (
    id bigserial NOT NULL,
    city character varying,
    CONSTRAINT label_addresses_pkey PRIMARY KEY (id)
);
CREATE TABLE label_events (
    id bigserial NOT NULL,
    label_id bigint,
    note character varying,
    CONSTRAINT label_events_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_sti_sub_req
"#;

const LABEL: &str = "class Label < ApplicationRecord\n  self.table_name = \"labels\"\nend\n";
const ADDRESS: &str = "class LabelAddress < ApplicationRecord\n  self.table_name = \"label_addresses\"\nend\n";
const EVENT: &str = "class LabelEvent < ApplicationRecord\n  self.table_name = \"label_events\"\nend\n";

const RETURN_LABEL: &str = r#"class ReturnLabel < Label
  belongs_to :destination_address, foreign_key: :destination_address_id, class_name: "LabelAddress", optional: false, inverse_of: false
  has_many :events, class_name: "LabelEvent", foreign_key: :label_id, inverse_of: false
  accepts_nested_attributes_for :destination_address
end
"#;

const PROBE: &str = r##"class StiSubReqProbe
  def self.lines
    out = []
    bare = ReturnLabel.new(name: "bare")
    out << "missing: #{bare.valid?} #{bare.errors.full_messages.inspect}"
    l = ReturnLabel.new(name: "a", destination_address_attributes: { city: "Ottawa" })
    out << "nested: #{l.valid?} #{l.save} #{LabelAddress.count}"
    l = Label.find(l.id)
    out << "found: #{l.class} #{l.destination_address.city}"
    held = ReturnLabel.new(name: "b", destination_address: LabelAddress.new(city: "Hull"))
    out << "held: #{held.save}"
    stale = ReturnLabel.new(name: "c", destination_address_id: 0)
    out << "zero: #{stale.valid?}"
    r = ReturnLabel.find(l.id)
    LabelEvent.create!(label_id: r.id, note: "x")
    out << "events: #{r.events.map(&:note).inspect}"
    LabelEvent.create!(label_id: r.id, note: "y")
    r.reload
    out << "reloaded: #{r.events.map(&:note).inspect} #{r.destination_address.city}"
    out << "includes: #{ReturnLabel.includes(:destination_address, :events).to_a.map { |x| x.destination_address&.city }.inspect}"
    out
  end
end
"##;

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
    ("app/models/label.rb", LABEL),
    ("app/models/label_address.rb", ADDRESS),
    ("app/models/label_event.rb", EVENT),
    ("app/models/return_label.rb", RETURN_LABEL),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
        .write("app/models/sti_sub_req_probe.rb", PROBE)
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn subclass_required_belongs_to_accepts_a_held_target_as_natively_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let mut files = FILES.to_vec();
    files.push(("app/models/sti_sub_req_probe.rb", PROBE));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "sti-sub-req",
        &files,
        &emitted,
        "rh_spec_sti_sub_req",
        "StiSubReqProbe",
        &[
            "app/models/application_record",
            "app/models/label",
            "app/models/label_address",
            "app/models/label_event",
            "app/models/return_label",
            "app/models/sti_sub_req_probe",
        ],
    ) else { return };
    assert!(native[0].starts_with("missing: false"), "native: {native:?}");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// The compiler side: the subclass's required check yields to a held
/// target, and its reload / preload / collection passes ran.
#[test]
#[ignore = "fork spec: postgresql"]
fn subclass_required_belongs_to_check_yields_to_a_held_target() {
    let app = overlay();
    let (emitted, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let out = std::fs::read_to_string(emitted.join("app/models/return_label.rb")).expect("return_label.rb");
    assert!(out.contains("@destination_address_loaded != true || @destination_address_cache.nil?"), "{out}");
    assert!(out.contains("def _adapter_reload"), "{out}");
    assert!(out.contains("@events_loaded = false") && out.contains("@destination_address_loaded = false"), "{out}");
    assert!(out.contains("def self._preload_dispatch"), "{out}");
    assert!(!out.contains("def self.preload_associations"), "{out}");
}
