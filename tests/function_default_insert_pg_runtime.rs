//! PostgreSQL: function default insert, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE items (
    id bigserial NOT NULL,
    name character varying(255) NOT NULL,
    directory_id uuid DEFAULT lpad(to_hex(0::bigint), 32, '0')::uuid NOT NULL,
    label character varying(255) DEFAULT lower('ABC'::text) NOT NULL,
    weight integer DEFAULT length('abcd'::text) NOT NULL,
    note character varying(255) DEFAULT 'plain'::character varying NOT NULL,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_function_default
"#;

const ITEM: &str = r#"class Item < ApplicationRecord
end
"#;

const PROBE: &str = r##"class FunctionDefaultProbe
  def self.lines
    out = []
    m = Item.new(name: "a")
    out << "new: #{m.directory_id.inspect} #{m.label.inspect} #{m.weight.inspect} #{m.note.inspect}"
    m.save!
    out << "created: #{m.directory_id.inspect} #{m.label.inspect} #{m.weight.inspect} #{m.note.inspect}"
    m.name = "b"
    m.save!
    out << "updated without reload: #{Item.find(m.id).attributes.slice("name", "directory_id", "label", "weight").inspect}"
    given = Item.create!(name: "c", directory_id: "00000000-0000-0000-0000-000000000007", label: "set", weight: 9)
    out << "assigned: #{given.directory_id.inspect} #{given.label.inspect} #{given.weight.inspect}"
    out << "reloaded: #{Item.find(given.id).attributes.slice("directory_id", "label", "weight").inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 5] = [
    r#"new: nil nil nil "plain""#,
    r#"created: "00000000-0000-0000-0000-000000000000" "abc" 4 "plain""#,
    r#"updated without reload: {"name" => "b", "directory_id" => "00000000-0000-0000-0000-000000000000", "label" => "abc", "weight" => 4}"#,
    r#"assigned: "00000000-0000-0000-0000-000000000007" "set" 9"#,
    r#"reloaded: {"directory_id" => "00000000-0000-0000-0000-000000000007", "label" => "set", "weight" => 9}"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 3] = ["app/models/application_record", "app/models/item", "app/models/function_default_probe"];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
    ("app/models/item.rb", ITEM),
    ("app/models/function_default_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The model natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_function_default_column_takes_the_databases_value_like_native_rails() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "function-default",
        &FILES,
        &emitted,
        "rh_spec_function_default",
        "FunctionDefaultProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// Without a database, the emitted INSERT writes the `DEFAULT` keyword for
/// each function-defaulted column the record left nil and reads the
/// database's values back; the literal-default column is still written.
#[test]
#[ignore = "fork spec: postgresql"]
fn the_insert_leaves_function_defaults_to_the_database() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let model = std::fs::read_to_string(emitted.join("app/models/item.rb")).expect("emitted model");
    assert_eq!(model.matches("\"DEFAULT\"").count(), 6, "three slots in each of two INSERT shapes:\n{model}");
    assert!(
        model.contains("RETURNING CAST(\\\"id\\\" AS TEXT), \\\"directory_id\\\", \\\"label\\\", \\\"weight\\\""),
        "the database's values come back with the key:\n{model}"
    );
    assert!(model.contains("Db.escape_string(@note)"), "a literal default is still written:\n{model}");
}
