//! PostgreSQL: autoload on load subclass, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_on_load_subclass
"#;

const STRUCTURE: &str = "CREATE TABLE units (\n    id bigserial NOT NULL,\n    CONSTRAINT units_pkey PRIMARY KEY (id)\n);\n";

const BASE: &str = r#"class BaseUnit
  def self.registry
    @registry ||= []
  end
end
"#;
const SPECIAL: &str = "class SpecialUnit < BaseUnit\n  def self.label = \"special\"\nend\n";
/// Warnings on from boot, collected: the callback may run as soon as the
/// constant loads, before any probe could turn them on.
const WARNING_LOG: &str = r#"module UnitWarningLog
  def self.messages
    @messages ||= []
  end

  def warn(message, **options)
    UnitWarningLog.messages << message
  end
end
"#;
const INITIALIZER: &str = r#"Warning.singleton_class.prepend(UnitWarningLog)
$VERBOSE = true

Rails.autoloaders.main.on_load("BaseUnit") do
  BaseUnit.registry << "base loaded"
  SpecialUnit.label
  BaseUnit.registry << "special loaded"
end
"#;

/// Reads what boot collected: a circular require anywhere in the load is
/// counted.
const PROBE: &str = r##"class OnLoadSubclassProbe
  def self.lines
    [
      "registry: #{BaseUnit.registry.inspect}",
      "special: #{SpecialUnit.label} #{SpecialUnit.superclass}",
      "circular requires: #{UnitWarningLog.messages.grep(/circular require/).size}"
    ]
  end
end
"##;

const NATIVE_BOOT: &str = r#"require "zeitwerk"

module Rails
  def self.autoloaders
    @autoloaders ||= Struct.new(:main).new(Zeitwerk::Loader.new)
  end
end

loader = Rails.autoloaders.main
loader.push_dir(File.join(__dir__, "app/models"))
loader.setup
Dir[File.join(__dir__, "config/initializers/*.rb")].sort.each { |file| load file }
"#;

const EXPECTED: [&str; 3] = [
    r#"registry: ["base loaded", "special loaded"]"#,
    "special: special BaseUnit",
    "circular requires: 0",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/base_unit.rb", BASE),
    ("app/models/special_unit.rb", SPECIAL),
    ("app/models/on_load_subclass_probe.rb", PROBE),
    ("app/models/unit_warning_log.rb", WARNING_LOG),
    ("config/initializers/units.rb", INITIALIZER),
];

#[test]
#[ignore = "fork spec: postgresql native control"]
fn an_on_load_callback_requiring_a_subclass_matches_native_zeitwerk() {
    let overlay = FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(roundhouse::project::BuildTarget::Ruby);
    let native_files: Vec<(&str, &str)> = FILES.iter().copied().chain([("native_boot.rb", NATIVE_BOOT)]).collect();
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "on-load-subclass",
        &native_files,
        &emitted,
        "rh_spec_on_load_subclass",
        "OnLoadSubclassProbe",
        &["native_boot"],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    let real: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
