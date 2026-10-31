//! PostgreSQL: autoload on load namespace constant, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const MAKER: &str = r##"module Ns
  class Maker
    #: () -> void
    def initialize
      @settings = nil
      @types = []
    end

    #: (Symbol) -> void
    def register_type(type)
      @types << type
    end

    #: () -> void
    def configure
      @settings = { main: "ready" }
    end

    #: (Symbol) -> String
    def lookup(type)
      "#{@settings.fetch(type)}:#{@types.join(",")}"
    end
  end
end
"##;

const FACTORY: &str = r##"module Ns
  Factory = Maker.new

  Factory.register_type(:first)
  Factory.register_type(:second)
end
"##;

const INITIALIZER: &str = r##"Rails.autoloaders.main.on_load("Ns::Factory") do
  Ns::Factory.configure
end
"##;

const PROBE: &str = r##"class AutoloadNamespaceConstantProbe
  def self.lines
    ["lookup: #{Ns::Factory.lookup(:main)}"]
  end
end
"##;

const STRUCTURE: &str = "CREATE TABLE items (\n    id bigserial NOT NULL,\n    CONSTRAINT items_pkey PRIMARY KEY (id)\n);\n";

const DATABASE_YML: &str =
    "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: rh_spec_on_load_namespace_constant\n";

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

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/ns/maker.rb", MAKER),
    ("app/models/ns/factory.rb", FACTORY),
    ("app/models/autoload_namespace_constant_probe.rb", PROBE),
    ("config/initializers/factory.rb", INITIALIZER),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_file_assigning_the_constant_reports_its_load() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let read = |rel: &str| std::fs::read_to_string(emitted.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let namespace = read("app/models/ns.rb");
    assert!(namespace.contains("Factory = "), "{namespace}");
    assert!(
        namespace.trim_end().ends_with("RoundhouseAutoload.autoloaded(\"Ns::Factory\", __FILE__) if defined?(RoundhouseAutoload)"),
        "{namespace}"
    );
    let models = read("app/models.rb");
    assert!(models.contains("AUTOLOAD_CALLBACK_CPATHS = [\n    \"Ns::Factory\",\n  ].freeze"), "{models}");
}

#[test]
#[ignore = "fork spec: postgresql native control"]
fn an_on_load_callback_runs_when_a_namespace_file_assigns_the_constant() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let native_files: Vec<(&str, &str)> = FILES.iter().copied().chain([("native_boot.rb", NATIVE_BOOT)]).collect();
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "autoload-on-load-namespace-constant",
        &native_files,
        &emitted,
        "rh_spec_on_load_namespace_constant",
        "AutoloadNamespaceConstantProbe",
        &["native_boot"],
    ) else { return };
    assert_eq!(native, ["lookup: ready:first,second"], "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
