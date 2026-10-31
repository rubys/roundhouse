//! PostgreSQL: suid column macro, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE fences (
    id uuid NOT NULL,
    label character varying NOT NULL,
    CONSTRAINT fences_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_suid_column_macro
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SUID_TYPE: &str = r#"class SuidType < ActiveRecord::Type::String
  def cast(value)
    value.nil? ? nil : value.to_s.downcase
  end

  def serialize(value)
    value.nil? ? nil : value.to_s.downcase
  end
end
"#;

const SUID_INITIALIZER: &str = "ActiveRecord::Type.register(:suid, SuidType, adapter: :postgresql)\n";

const CONCERN: &str = r#"module GenerateSuid
  extend ActiveSupport::Concern

  module ClassMethods
    def suid_column(column)
      attribute column, :suid, default: -> { SecureRandom.uuid }
      attr_readonly(column.to_s)
    end
  end
end
"#;

const FENCE: &str = "class Fence < ApplicationRecord\n  include GenerateSuid\n  suid_column :id\nend\n";

const PROBE: &str = r##"class SuidColumnMacroProbe
  def self.lines
    out = []
    fence = Fence.new(label: "a")
    out << "new id: #{fence.id.to_s.length}"
    fence.save!
    out << "saved: #{fence.persisted?} #{fence.id.to_s.length}"
    out << "found: #{Fence.find(fence.id).label}"
    other = Fence.create!(label: "b")
    out << "distinct: #{other.id != fence.id}"
    out << "given: #{Fence.create!(id: "11111111-1111-4111-8111-111111111111", label: "c").id}"
    out << "count: #{Fence.count}"
    out
  end
end
"##;

const LOADS: [&str; 6] = [
    "app/models/application_record",
    "app/models/suid_type",
    "config/initializers/suid",
    "app/models/generate_suid",
    "app/models/fence",
    "app/models/suid_column_macro_probe",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/suid_type.rb", SUID_TYPE),
    ("config/initializers/suid.rb", SUID_INITIALIZER),
    ("app/models/generate_suid.rb", CONCERN),
    ("app/models/fence.rb", FENCE),
    ("app/models/suid_column_macro_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_suid_column_macro_defaults_the_primary_key_like_native_rails() {
    let (emitted, _errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "suid-column-macro",
        &FILES,
        &emitted,
        "rh_spec_suid_column_macro",
        "SuidColumnMacroProbe",
        &LOADS,
    ) else { return };
    // Not a vacuous agreement: Rails' own answers.
    assert_eq!(native[0], "new id: 36", "{native:#?}");
    assert_eq!(native[4], "given: 11111111-1111-4111-8111-111111111111", "{native:#?}");
    assert_eq!(emitted_lines, native, "emitted");
}
