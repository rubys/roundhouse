//! PostgreSQL: sti subclass namespaced belongs to reflection, on the Ruby target against native Rails.
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
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_sti_sub_assoc
"#;

const LABEL: &str = r#"module Shipping
  class Label < ApplicationRecord
    self.table_name = "labels"
  end
end
"#;

const ADDRESS: &str = r#"module Shipping
  class LabelAddress < ApplicationRecord
    self.table_name = "label_addresses"
  end
end
"#;

const RETURN_LABEL: &str = r#"module Shipping
  class ReturnLabel < Label
    belongs_to :destination_address, foreign_key: :destination_address_id, class_name: "LabelAddress", optional: true, inverse_of: false
    accepts_nested_attributes_for :destination_address
  end
end
"#;

const PROBE: &str = r##"class StiSubAssocProbe
  def self.lines
    out = []
    out << "reflection: #{Shipping::ReturnLabel.reflect_on_association(:destination_address).class_name} #{Shipping::ReturnLabel.reflect_on_association(:destination_address).klass}"
    out << "parent: #{Shipping::Label.reflect_on_association(:destination_address).inspect}"
    out << "options: #{Shipping::ReturnLabel.nested_attributes_options.keys.inspect}"
    l = Shipping::ReturnLabel.create!(name: "a", destination_address_attributes: { city: "Ottawa" })
    l = Shipping::Label.find(l.id)
    out << "built: #{l.class} #{l.destination_address.city} #{Shipping::LabelAddress.count}"
    l.update(destination_address_attributes: { id: l.destination_address.id, city: "Hull" })
    out << "updated: #{Shipping::LabelAddress.pluck(:city).inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 5] = [
    "reflection: LabelAddress Shipping::LabelAddress",
    "parent: nil",
    "options: [:destination_address]",
    "built: Shipping::ReturnLabel Ottawa 1",
    r#"updated: ["Hull"]"#,
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
    ("app/models/shipping/label.rb", LABEL),
    ("app/models/shipping/label_address.rb", ADDRESS),
    ("app/models/shipping/return_label.rb", RETURN_LABEL),
    ("app/models/sti_sub_assoc_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn subclass_association_feeds_nested_attributes_as_natively_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "sti-sub-assoc",
        &FILES,
        &emitted,
        "rh_spec_sti_sub_assoc",
        "StiSubAssocProbe",
        &[
            "app/models/application_record",
            "app/models/shipping/label",
            "app/models/shipping/label_address",
            "app/models/shipping/return_label",
            "app/models/sti_sub_assoc_probe",
        ],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// The compiler side: the subclass answers a reflection for what it
/// declares whether its superclass is written qualified or not, and a
/// root model's behavior is unchanged.
#[test]
#[ignore = "fork spec: postgresql"]
fn subclass_reflects_its_declared_association_for_either_superclass_spelling() {
    for (spelling, source) in [
        ("unqualified", RETURN_LABEL.to_string()),
        ("qualified", RETURN_LABEL.replace("< Label", "< Shipping::Label")),
    ] {
        let files = FILES.map(|(path, text)| (path, if path.ends_with("return_label.rb") { source.as_str() } else { text }));
        let app = files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
        let (emitted, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
        assert!(real_errors(&errors).is_empty(), "{spelling}: {}", errors.join("\n"));
        let out = std::fs::read_to_string(emitted.join("app/models/shipping/return_label.rb")).expect("return_label.rb");
        let at = |needle: &str| out.find(needle).unwrap_or_else(|| panic!("{spelling}: {needle} in:\n{out}"));
        assert!(at("BelongsToReflection.new(:destination_address") < at("accepts_nested_attributes_for :destination_address"), "{spelling}:\n{out}");
        assert!(out.contains("def destination_address"), "{spelling}:\n{out}");
    }
}
