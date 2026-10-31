//! PostgreSQL: sti reflect on association, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;
const STRUCTURE: &str = r#"CREATE TABLE owners (
    id bigserial NOT NULL,
    CONSTRAINT owners_pkey PRIMARY KEY (id)
);

CREATE TABLE rules (
    id bigserial NOT NULL,
    owner_id bigint,
    type character varying,
    CONSTRAINT rules_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_sti_reflect
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const OWNER: &str = r#"class Owner < ApplicationRecord
end
"#;

const RULE: &str = r#"class Rule < ApplicationRecord
  belongs_to :owner, primary_key: :id, foreign_key: :owner_id, inverse_of: false
end
"#;

const CODE_RULE: &str = r#"module Pricing
  class CodeRule < ::Rule
  end
end
"#;

const CODE_APP_RULE: &str = r#"module Pricing
  class CodeAppRule < CodeRule
    after_initialize :set_defaults

    def set_defaults
      @defaults = true
    end
  end
end
"#;

const PROBE: &str = r##"class StiReflectProbe
  def self.lines
    out = []
    [Rule, Pricing::CodeRule, Pricing::CodeAppRule].each do |klass|
      reflection = klass.reflect_on_association(:owner)
      out << "#{klass.name}: #{reflection ? "#{reflection.macro} #{reflection.name} #{reflection.foreign_key}" : "nil"} #{klass.reflect_on_association(:missing).inspect}"
    end
    out
  end
end
"##;

const EXPECTED: [&str; 3] = [
    "Rule: belongs_to owner owner_id nil",
    "Pricing::CodeRule: belongs_to owner owner_id nil",
    "Pricing::CodeAppRule: belongs_to owner owner_id nil",
];

/// What native Rails requires, in order.
const LOADS: [&str; 6] = [
    "app/models/application_record",
    "app/models/owner",
    "app/models/rule",
    "app/models/pricing/code_rule",
    "app/models/pricing/code_app_rule",
    "app/models/sti_reflect_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/owner.rb", OWNER),
    ("app/models/rule.rb", RULE),
    ("app/models/pricing/code_rule.rb", CODE_RULE),
    ("app/models/pricing/code_app_rule.rb", CODE_APP_RULE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text)).write("app/models/sti_reflect_probe.rb", PROBE)
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn sti_subclass_under_an_empty_parent_reflects_the_root_models_associations_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let mut files = FILES.to_vec();
    files.push(("app/models/sti_reflect_probe.rb", PROBE));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "sti-reflect",
        &files,
        &emitted,
        "rh_spec_sti_reflect",
        "StiReflectProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
