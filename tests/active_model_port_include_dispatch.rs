//! PostgreSQL: active model port include dispatch, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const TICKET: &str = r#"class Ticket
  include ActiveModel::Conversion

  def id
    5
  end

  def persisted?
    true
  end

  def path
    "/tickets/#{to_param}"
  end

  def key_of(other)
    other.to_key
  end
end
"#;

const UNKNOWN: &str = r#"class Stub
  include ActiveModel::Conversion

  def path
    "/stubs/#{some_unknown_conversion_api}"
  end
end
"#;

fn overlay(files: &[(&str, &str)]) -> emit_and_run::Overlay {
    files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

#[test]
#[ignore = "fork spec: postgresql"]
fn an_includer_of_a_runtime_active_model_port_dispatches_its_methods_on_postgresql() {
    let files = [
        ("roundhouse.yml", ""),
        ("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_spec_port_include\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("db/structure.sql", "CREATE TABLE widgets (\n    id bigserial NOT NULL,\n    CONSTRAINT widgets_pkey PRIMARY KEY (id)\n);\n"),
        ("app/models/ticket.rb", TICKET),
        ("app/models/stub.rb", UNKNOWN),
    ];
    let (emitted, errors) = overlay(&files).emit(roundhouse::project::BuildTarget::Ruby);
    let ticket = std::fs::read_to_string(emitted.join("app/models/ticket.rb")).expect("emitted ticket");
    assert!(ticket.contains("#{to_param}"), "no plain to_param in:\n{ticket}");
    assert!(ticket.contains("other.to_key"), "no plain to_key in:\n{ticket}");
    assert!(!ticket.contains("not supported"), "{ticket}");
    assert!(
        !errors.iter().any(|e| e.contains("to_param") || e.contains("to_key")),
        "{errors:#?}"
    );
}

/// Without the PostgreSQL tree the port is not this tree's: `to_param`
/// on the class stays the named gap.
#[test]
fn an_includer_of_a_runtime_active_model_port_stays_a_named_gap_without_postgresql() {
    let files = [
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n"),
        ("app/models/ticket.rb", TICKET),
    ];
    let (_emitted, errors) = overlay(&files).emit(roundhouse::project::BuildTarget::Ruby);
    assert!(
        errors.iter().any(|e| e.contains("to_param lacks a shared runtime/lowering implementation")),
        "{errors:#?}"
    );
}
