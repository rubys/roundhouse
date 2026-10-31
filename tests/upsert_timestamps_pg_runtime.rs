//! PostgreSQL: upsert timestamps, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE rates (
    id bigserial NOT NULL,
    code character varying NOT NULL,
    value integer,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    CONSTRAINT rates_pkey PRIMARY KEY (id)
);
CREATE UNIQUE INDEX index_rates_on_code ON rates USING btree (code);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_upsert_timestamps
"#;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n";

const RATE: &str = "class Rate < ApplicationRecord\nend\n";

const PROBE: &str = r##"class UpsertTimestampsProbe
  def self.lines
    out = []
    Rate.upsert({ code: "usd", value: 1 }, unique_by: :code)
    first = Rate.find_by(code: "usd")
    out << "insert: #{first.created_at.nil?} #{first.updated_at.nil?}"
    sleep 0.01
    Rate.upsert({ code: "usd", value: 1 }, unique_by: :code)
    same = Rate.find_by(code: "usd")
    out << "unchanged: #{same.updated_at == first.updated_at}"
    sleep 0.01
    Rate.upsert({ code: "usd", value: 2 }, unique_by: :code)
    moved = Rate.find_by(code: "usd")
    out << "changed: #{moved.value} created #{moved.created_at == first.created_at} touched #{moved.updated_at > first.updated_at}"
    Rate.upsert_all([{ code: "usd", value: 3 }, { code: "eur", value: 4 }], unique_by: :code)
    out << "upsert_all: #{Rate.order(:code).map { |r| [r.code, r.value, r.created_at.nil?] }.inspect}"
    stamp = Time.utc(2020, 1, 2, 3, 4, 5)
    Rate.upsert({ code: "gbp", value: 5, created_at: stamp, updated_at: stamp }, unique_by: :code)
    given = Rate.find_by(code: "gbp")
    out << "given: #{given.created_at.year} #{given.updated_at.year}"
    # A row keyed by the strings of a record's `attributes`, with a symbol
    # key merged over it (GenerateOid's `.merge(id: oid)`), names each
    # column once.
    Rate.upsert_all([{ "code" => "chf", "value" => 7 }.merge(value: 8)], unique_by: :code)
    out << "mixed keys: #{Rate.find_by(code: "chf").value}"
    out
  end
end
"##;

const EXPECTED: [&str; 6] = [
    "insert: false false",
    "unchanged: true",
    "changed: 2 created true touched true",
    r#"upsert_all: [["eur", 4, false], ["usd", 3, false]]"#,
    "given: 2020 2020",
    "mixed keys: 8",
];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/rate.rb", RATE),
    ("app/models/upsert_timestamps_probe.rb", PROBE),
];

#[test]
fn upsert_stamps_timestamps_as_natively_on_postgresql() {
    let overlay = FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "upsert-timestamps",
        &FILES,
        &emitted,
        "rh_spec_upsert_timestamps",
        "UpsertTimestampsProbe",
        &["app/models/application_record", "app/models/rate", "app/models/upsert_timestamps_probe"],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    let real: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
