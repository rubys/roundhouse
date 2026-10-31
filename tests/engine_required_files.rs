//! PostgreSQL: engine required files, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const ENGINE: &str = r#"require "rails"

module Widgets
  class Engine < Rails::Engine
    if Rails.env.test?
      paths.add("test/support", load_path: true)

      require_relative "../../app/services/widget_service"
      require_relative "../../test/support/widget_service_mock"
      require_relative "../../test/support/helpers/widget_helper"
    end
  end
end
"#;

const SERVICE: &str = r#"class WidgetService
  def self.price
    10
  end
end
"#;

const MOCK: &str = r#"class WidgetService
  class << self
    alias_method :price_from_service, :price

    def price
      price_from_service + 1
    end
  end
end
"#;

const HELPER: &str = r##"require_relative "../widget_constants"

module WidgetHelper
  def self.label
    "#{WIDGET_PREFIX}-#{WidgetService.price}"
  end
end
"##;

const CONSTANTS: &str = "WIDGET_PREFIX = \"w\"\n";

const FILES: &[(&str, &str)] = &[
    ("engines/widgets/lib/widgets/engine.rb", ENGINE),
    ("engines/widgets/app/services/widget_service.rb", SERVICE),
    ("engines/widgets/test/support/widget_service_mock.rb", MOCK),
    ("engines/widgets/test/support/helpers/widget_helper.rb", HELPER),
    ("engines/widgets/test/support/widget_constants.rb", CONSTANTS),
];

const PATH_GEMS: &str = "PATH\n  remote: engines/widgets\n  specs:\n    widgets (0.1.0)\n\nDEPENDENCIES\n  widgets!\n";

/// Boot the emitted tree as rails_engine_port.rs does (the full boot
/// needs the pg gem): Rails, the railtie and engine ports, then
/// app/models, which loads the engine.
fn boot(dir: &std::path::Path, env: &str) -> std::process::Output {
    emit_and_run::ruby()
        .current_dir(dir)
        .args([
            "-e",
            &format!(
                "require_relative 'runtime/active_support_lazy_load_hooks'\n\
                 $LOAD_PATH.unshift(File.expand_path('runtime/features'))\n\
                 require_relative 'runtime/module_delegate'\n\
                 require_relative 'runtime/active_support_inflector'\n\
                 require_relative 'runtime/rails'\n\
                 Rails.env_name = {env:?}\n\
                 require_relative 'runtime/rails_railtie'\n\
                 require_relative 'runtime/rails_engine'\n\
                 require_relative 'runtime/active_support_deprecation'\n\
                 require_relative 'app/models'\n\
                 puts WidgetService.price\n\
                 puts defined?(WidgetHelper).inspect\n\
                 puts WidgetHelper.label if defined?(WidgetHelper)\n"
            ),
        ])
        .output()
        .expect("ruby")
}

#[test]
#[ignore = "fork spec: postgresql layout"]
fn an_engine_body_loads_the_emitted_files_its_source_requires_relative() {
    let mut fixture = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_engine_required_files\n")
        .write("Gemfile.lock", PATH_GEMS);
    for (path, text) in FILES {
        fixture = fixture.write(path, text);
    }
    let (emitted, errors) = fixture.emit(BuildTarget::Ruby);
    // The fixture's only error is the PostgreSQL adapter this
    // fixture's emit does not compile in (rails_engine_port.rs boots
    // the same way); nothing about the engine's files.
    let own: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(own.is_empty(), "{own:?}");
    let engine = std::fs::read_to_string(emitted.join("app/models/widgets/engine.rb")).expect("the engine is carried");
    assert!(engine.contains("class Engine < Rails::Engine"), "{engine}");
    assert!(!engine.contains("require_relative \"../../"), "the source's relative paths are resolved:\n{engine}");
    let service = std::fs::read_to_string(emitted.join("app/models/widget_service.rb")).expect("the service");
    assert!(!service.contains("price_from_service"), "the mock is not merged into the service:\n{service}");

    let output = boot(&emitted, "test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "emitted program failed in {}:\n{stdout}\n{stderr}\n{engine}", emitted.display());
    assert!(!stderr.contains("LoadError") && !stderr.contains("roundhouse: error"), "{stderr}");
    assert_eq!(stdout, "11\n\"constant\"\nw-11\n", "{stderr}\n{engine}");
    // Outside the test environment the body requires nothing, and
    // neither the mock nor the helper is defined.
    let output = boot(&emitted, "development");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "emitted program failed in {}:\n{stderr}", emitted.display());
    assert_eq!(stdout, "10\nnil\n", "{stderr}");
}

const DOMAIN_MOCK: &str = r#"module Registrar
  class DomainMock
    MockDomain = Struct.new(:host) do
      def to_s
        self[:host] || ""
      end
    end #: singleton(Struct)

    class Lookup
      def answer
        "found"
      end
    end

    def available?
      [true, DEFAULT_DOMAINS]
    end

    DEFAULT_DOMAINS = [
      MockDomain.new("localhost.com"),
      MockDomain.new("localhost.net"),
    ].freeze #: Array[untyped]
    LOOKUP = Lookup.new
  end
end
"#;

const MOCK_ENGINE: &str = r#"require "rails"

module Widgets
  class Engine < Rails::Engine
    if Rails.env.test?
      require_relative "../../test/support/registrar/domain_mock"
    end
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql layout"]
fn a_required_file_keeps_the_classes_its_class_nests_ahead_of_the_constants_reading_them() {
    let (emitted, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_engine_required_files\n")
        .write("Gemfile.lock", PATH_GEMS)
        .write("engines/widgets/lib/widgets/engine.rb", MOCK_ENGINE)
        .write("engines/widgets/test/support/registrar/domain_mock.rb", DOMAIN_MOCK)
        .emit(BuildTarget::Ruby);
    let own: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(own.is_empty(), "{own:?}");
    let program = std::fs::read_to_string(
        emitted.join("app/models/roundhouse_files/engines/widgets/test/support/registrar/domain_mock.rb"),
    )
    .expect("the required file is carried as a file program");
    assert!(!program.contains("module DomainMock"), "the nested class reopens the class, not a module:\n{program}");
    let output = emit_and_run::ruby()
        .current_dir(&emitted)
        .args([
            "-e",
            "require_relative 'runtime/active_support_lazy_load_hooks'\n\
             $LOAD_PATH.unshift(File.expand_path('runtime/features'))\n\
             require_relative 'runtime/module_delegate'\n\
             require_relative 'runtime/active_support_inflector'\n\
             require_relative 'runtime/rails'\n\
             Rails.env_name = 'test'\n\
             require_relative 'runtime/rails_railtie'\n\
             require_relative 'runtime/rails_engine'\n\
             require_relative 'runtime/active_support_deprecation'\n\
             require_relative 'app/models'\n\
             mock = Registrar::DomainMock\n\
             p mock.new.available?.last.map(&:to_s)\n\
             p [mock::MockDomain.superclass, mock::LOOKUP.answer, mock::Lookup.instance_of?(Class)]\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "emitted program failed in {}:\n{stdout}\n{stderr}\n{program}", emitted.display());
    // Native Ruby on the source prints the same two lines.
    assert_eq!(stdout, "[\"localhost.com\", \"localhost.net\"]\n[Struct, \"found\", true]\n", "{stderr}\n{program}");
}
