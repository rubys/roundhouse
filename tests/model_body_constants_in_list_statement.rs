//! PostgreSQL: model body constants in list statement, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const STRUCTURE: &str = r#"CREATE TABLE settings (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT settings_pkey PRIMARY KEY (id)
);
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", STRUCTURE)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "app/models/setting.rb",
            "class Setting < ApplicationRecord\n  [\n    DISABLED = \"disabled\",\n    NO_LOCATIONS = \"no_locations\",\n  ].freeze\nend\n",
        )
}

#[test]
#[ignore = "fork spec: postgresql"]
fn constants_assigned_inside_a_list_statement_exist_on_a_model() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Ruby);
    let real: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    let setting = std::fs::read_to_string(emitted.join("app/models/setting.rb")).unwrap();
    for expected in ["DISABLED = \"disabled\"", "NO_LOCATIONS = \"no_locations\""] {
        assert!(setting.contains(expected), "missing {expected:?} in:\n{setting}");
    }
}
