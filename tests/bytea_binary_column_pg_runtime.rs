//! PostgreSQL: bytea binary column, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE blobs (
    id bigserial NOT NULL,
    name character varying,
    payload bytea,
    note bytea NOT NULL DEFAULT '\x'::bytea,
    CONSTRAINT blobs_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_bytea_binary
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const BLOB: &str = r#"class Blob < ApplicationRecord
end
"#;

const PROBE: &str = r##"class ByteaProbe
  def self.lines
    out = []
    json = '{"attributes":{"k":"v"}}'
    bytes = [0, 200, 1, 255].pack("C*")
    digest = "2c26b46b68ffc68ff99b453c1c30413413308e1d".b
    a = Blob.create!(name: digest, payload: json, note: "n1")
    b = Blob.create!(name: "plain", payload: bytes)
    out << "insert: #{a.reload.payload.inspect} #{a.payload.encoding} #{a.name.inspect} #{b.reload.payload.inspect} #{b.note.inspect}"
    a.update!(payload: json + " changed", note: bytes)
    out << "update: #{Blob.find(a.id).payload.inspect} #{Blob.find(a.id).note.inspect}"
    out << "where: #{Blob.where(payload: json + " changed").count} #{Blob.where(payload: bytes).count} #{Blob.where(payload: json).count}"
    out << "varchar: #{Blob.where(name: digest).count} #{Blob.where(name: digest.dup.force_encoding("UTF-8")).count} #{Blob.where(name: "plain".b).count}"
    Blob.upsert_all([{ id: a.id, name: "up", payload: json }, { id: 900, name: digest, payload: bytes }], unique_by: :id)
    out << "upsert: #{Blob.find(a.id).payload.inspect} #{Blob.find(a.id).name.inspect} #{Blob.find(900).payload.inspect} #{Blob.find(900).name.inspect}"
    Blob.insert_all([{ name: "bulk", payload: json }])
    bulk = Blob.find_by!(name: "bulk")
    out << "insert_all: #{bulk.payload.inspect} #{bulk.note.inspect}"
    Blob.where(id: bulk.id).update_all(payload: bytes)
    out << "update_all: #{Blob.find(bulk.id).payload.inspect}"
    b.payload = nil
    b.save!
    out << "nil: #{b.reload.payload.inspect} #{Blob.where(payload: nil).count}"
    data = ActiveModel::Type::Binary::Data.new(bytes)
    out << "quote: #{Blob.connection.quoted_binary(data)} #{Blob.connection.quote(data)}"
    out
  end
end
"##;

const EXPECTED: [&str; 9] = [
    r#"insert: "{\"attributes\":{\"k\":\"v\"}}" ASCII-8BIT "2c26b46b68ffc68ff99b453c1c30413413308e1d" "\x00\xC8\x01\xFF" """#,
    r#"update: "{\"attributes\":{\"k\":\"v\"}} changed" "\x00\xC8\x01\xFF""#,
    "where: 1 1 0",
    "varchar: 1 1 1",
    r#"upsert: "{\"attributes\":{\"k\":\"v\"}}" "up" "\x00\xC8\x01\xFF" "2c26b46b68ffc68ff99b453c1c30413413308e1d""#,
    r#"insert_all: "{\"attributes\":{\"k\":\"v\"}}" """#,
    r#"update_all: "\x00\xC8\x01\xFF""#,
    "nil: nil 1",
    r#"quote: '\x00c801ff' '\x00c801ff'"#,
];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/blob.rb", BLOB),
    ("app/models/bytea_probe.rb", PROBE),
];

const LOADS: [&str; 3] = [
    "app/models/application_record",
    "app/models/blob",
    "app/models/bytea_probe",
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The compiler side: the binary column writes through the bytea escape,
/// the varchar column through the text one.
#[test]
#[ignore = "fork spec: postgresql"]
fn the_column_type_picks_the_bytea_escape() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let blob = std::fs::read_to_string(emitted.join("app/models/blob.rb")).unwrap();
    assert!(blob.contains("Db.escape_binary_opt(@payload)"), "payload escapes as bytea:\n{blob}");
    assert!(blob.contains("Db.escape_binary(@note)"), "NOT NULL binary escapes as bytea:\n{blob}");
    assert!(blob.contains("Db.escape_string_opt(@name)"), "name stays text:\n{blob}");
    assert!(!blob.contains("Db.escape_string_opt(@payload)"), "payload never escapes as text:\n{blob}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn binary_and_varchar_columns_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "bytea-binary",
        &FILES,
        &emitted,
        "rh_spec_bytea_binary",
        "ByteaProbe",
        &LOADS,
    ) else { return };
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
