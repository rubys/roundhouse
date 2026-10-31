//! PostgreSQL: decimal without scale, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE measures (
    id bigserial NOT NULL,
    whole numeric(10),
    zero_scale numeric(12,0),
    scaled numeric(10,2),
    loose numeric,
    CONSTRAINT measures_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_decimal_without_scale
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const MEASURE: &str = "class Measure < ApplicationRecord\nend\n";

const PROBE: &str = r##"class DecimalWithoutScaleProbe
  def self.lines
    out = []
    %w[whole zero_scale scaled loose].each do |name|
      column = Measure.columns_hash[name]
      type = column.cast_type
      out << "#{name}: #{type.class} #{column.type.inspect} #{column.sql_type} #{[column.limit, column.precision, column.scale].inspect} type=#{type.type.inspect} precision=#{type.precision.inspect} scale=#{type.scale.inspect}"
      out << "#{name} casts: #{["12.7", 12, nil, ""].map { |v| type.cast(v) }.inspect} #{type.deserialize("42").inspect} #{type.serialize("7").inspect} #{type.type_cast_for_schema(5)}"
    end
    out
  end
end
"##;

const FILES: [(&str, &str); 6] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/measure.rb", MEASURE),
];

const PROBE_FILE: (&str, &str) = ("app/models/decimal_without_scale_probe.rb", PROBE);

/// What native Rails requires, in order.
const LOADS: [&str; 3] = [
    "app/models/application_record",
    "app/models/measure",
    "app/models/decimal_without_scale_probe",
];

fn files() -> Vec<(&'static str, &'static str)> {
    FILES.iter().copied().chain([PROBE_FILE]).collect()
}

fn overlay() -> emit_and_run::Overlay {
    files().iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_zero_scale_numeric_column_has_the_decimal_without_scale_type() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let all = files();
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "decimal-without-scale",
        &all,
        &emitted,
        "rh_spec_decimal_without_scale",
        "DecimalWithoutScaleProbe",
        &LOADS,
    ) else { return };
    assert!(native[0].starts_with("whole: ActiveRecord::Type::DecimalWithoutScale "), "{native:#?}");
    assert!(native[2].starts_with("zero_scale: ActiveRecord::Type::DecimalWithoutScale "), "{native:#?}");
    assert!(native[4].contains("OID::Decimal"), "{native:#?}");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
