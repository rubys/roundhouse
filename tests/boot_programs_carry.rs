//! PostgreSQL: boot programs carry, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const BOOT: &str = r#"ENV["PROBE_BOOT"] = "1"
ENV["BUNDLE_GEMFILE"] ||= File.expand_path("../Gemfile", __dir__)
gem("bundler", ">= 0.a")
require "bundler/setup"
require_relative "../lib/boot_lib"
ENV["PROBE_BOOT_TEST"] ||= (BootLib::RAILS_ENV == "test").to_s
$LOAD_PATH.unshift(File.expand_path("../app/utils", __dir__))
"#;

const BOOT_LIB: &str = r#"module BootLib
  RAILS_ENV = ENV.fetch("RAILS_ENV", "development")
end
"#;

const APPLICATION: &str = r#"require_relative "boot"

ENV["X"] ||= "y" unless Rails.env.production?
if Rails.env.development? && ENV["YUGABYTE"]
  ENV["NEVER"] ||= "0;1"
end

module Probe
  class Application < Rails::Application
    config.x.greeting = "from application"
  end
end
"#;

const ENVIRONMENT: &str = r#"require_relative "application"

Rails.application.initialize!
"#;

const TEST_ENV: &str = r#"Rails.application.configure do
  config.eager_load = false
  config.consider_all_requests_local = true
  config.x.foo = 1
end
"#;

const INITIALIZER: &str = r#"module M
  LIST = %w[a].freeze
  PATTERN = /'(?:[^'\\]|\\.|'')*'/
end

ActiveSupport::Notifications.subscribe("probe.event") do |*args|
  Report.seen = M::LIST.first
end
"#;

const NESTED_INITIALIZER: &str = r#"module ShopProbe
  module Counter
    TRACKED = %w[shop_images].freeze
    QUOTED = /'(?:[^'\\]|\\.|'')*'/
  end
end

ActiveSupport::Notifications.subscribe("probe.nested") do |*args|
  Report.nested = ShopProbe::Counter::TRACKED.first + ":" + "'a'".gsub(ShopProbe::Counter::QUOTED, "?")
end
"#;

const REPORT: &str = r#"class Report
  class << self
    attr_accessor :seen, :nested
  end

  def self.line
    [ENV["X"], ENV["PROBE_BOOT"], ENV["PROBE_BOOT_TEST"], ENV["NEVER"]].inspect
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_boot_programs_carry\n")
        .write("config/boot.rb", BOOT)
        .write("config/application.rb", APPLICATION)
        .write("config/environment.rb", ENVIRONMENT)
        .write("config/environments/test.rb", TEST_ENV)
        .write("config/initializers/m.rb", INITIALIZER)
        .write("config/initializers/shop_probe_counter.rb", NESTED_INITIALIZER)
        .write("app/models/shop_probe/thing.rb", "module ShopProbe\n  class Thing\n    def self.label\n      \"thing\"\n    end\n  end\nend\n")
        .write("lib/boot_lib.rb", BOOT_LIB)
        .write("lib/report.rb", REPORT)
}

fn read(dir: &std::path::Path, path: &str) -> String {
    std::fs::read_to_string(dir.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Each boot file is a program of its statements and requires, in source
/// order; Bundler's setup and `initialize!` are the emitted boot's own;
/// a `$LOAD_PATH` change is named. Boot loads the programs first.
#[test]
#[ignore = "fork spec: postgresql layout"]
fn boot_files_carry_their_statements_in_source_order() {
    let (dir, errors) = app().emit(BuildTarget::Ruby);
    let boot = read(&dir, "app/models/roundhouse_files/config/boot.rb");
    let at = |text: &str, needle: &str| text.find(needle).unwrap_or_else(|| panic!("{needle} in\n{text}"));
    let write = at(&boot, "ENV[\"PROBE_BOOT\"] = \"1\"");
    let require = at(&boot, "require RoundhouseAutoload.path(\"models/boot_lib\")");
    let reads = at(&boot, "BootLib::RAILS_ENV == \"test\"");
    assert!(write < require && require < reads, "{boot}");
    assert!(boot.contains("error[file_statement_not_carried]: config/boot.rb:7"), "{boot}");
    assert!(boot.contains("a `$LOAD_PATH` change is not carried"), "{boot}");
    assert!(!boot.contains("BUNDLE_GEMFILE") && !boot.contains("bundler"), "{boot}");

    let application = read(&dir, "app/models/roundhouse_files/config/application.rb");
    let boot_first = at(&application, "require RoundhouseAutoload.path(\"models/roundhouse_files/config/boot\")");
    let pod = at(&application, "(ENV[\"X\"] || (ENV[\"X\"] = \"y\"))");
    assert!(boot_first < pod, "{application}");
    assert!(application.contains("if Rails.env.development? && ENV[\"YUGABYTE\"]"), "{application}");
    assert!(!application.contains("x.greeting"), "the class body is the application reopen's: {application}");

    let environment = read(&dir, "app/models/roundhouse_files/config/environment.rb");
    assert!(!environment.contains("initialize!"), "{environment}");

    let models = read(&dir, "app/models.rb");
    let first = at(&models, "  [\"models/roundhouse_files/config/boot\", nil], # config/boot.rb (native boot)\n");
    let second = at(&models, "  [\"models/roundhouse_files/config/application\", nil], # config/application.rb (native boot)\n");
    let lib = at(&models, "  [\"models/boot_lib\", nil], # config/boot.rb:5\n");
    assert!(first < second && second < lib, "{models}");
    // The refused statement, as `check` reports it.
    assert!(
        errors.iter().any(|e| e.contains("`$LOAD_PATH.unshift` does not run") && e.contains("a `$LOAD_PATH` change is not carried")),
        "{errors:#?}"
    );
}

/// A Rails-generated boot (Bundler and Bootsnap setup, requires, the
/// application class, `initialize!`) runs nothing the boot loads do not
/// already: no boot program.
#[test]
fn a_generated_rails_boot_has_no_boot_program() {
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write(
            "config/boot.rb",
            "ENV[\"BUNDLE_GEMFILE\"] ||= File.expand_path(\"../Gemfile\", __dir__)\n\nrequire \"bundler/setup\"\nrequire \"bootsnap/setup\"\n",
        )
        .write(
            "config/application.rb",
            "require_relative \"boot\"\n\nrequire \"rails/all\"\n\nBundler.require(*Rails.groups)\n\nmodule Blog\n  class Application < Rails::Application\n    config.load_defaults 8.0\n  end\nend\n",
        )
        .write("config/environment.rb", "require_relative \"application\"\n\nRails.application.initialize!\n")
        .write("app/models/note.rb", "class Note\n  def self.title\n    \"n\"\n  end\nend\n")
        .emit(BuildTarget::Ruby);
    assert!(!dir.join("app/models/roundhouse_files/config").exists());
    assert!(!read(&dir, "app/models.rb").contains("(native boot)"));
}

/// `config.consider_all_requests_local` is carried as written into the
/// environment program, for the runtime's accessor; `config.x.foo` stays
/// the lifted app key.
#[test]
#[ignore = "fork spec: postgresql layout"]
fn consider_all_requests_local_is_carried_as_written() {
    let (dir, errors) = app().emit(BuildTarget::Ruby);
    let program = read(&dir, "app/models/roundhouse_environment_test.rb");
    assert!(program.contains("Rails.application.config.consider_all_requests_local = true"), "{program}");
    assert!(program.contains("Rails.application.x_foo = 1"), "{program}");
    let models = read(&dir, "app/models.rb");
    assert!(!models.contains("consider_all_requests_local` is framework configuration"), "{models}");
    assert!(!errors.iter().any(|e| e.contains("consider_all_requests_local")), "{errors:#?}");
    let runtime = read(&dir, "runtime/rails_configuration.rb");
    assert!(runtime.contains("def consider_all_requests_local"), "runtime carries the accessor");
}

/// A module an initializer defines and reads itself is library code,
/// loaded at its definition; so is one nested under an app namespace.
#[test]
#[ignore = "fork spec: postgresql layout"]
fn an_initializer_local_module_is_carried_as_library_code() {
    let (dir, errors) = app().emit(BuildTarget::Ruby);
    let m = read(&dir, "app/models/m.rb");
    assert!(m.contains("LIST = %w[a].freeze"), "{m}");
    let counter = read(&dir, "app/models/shop_probe/counter.rb");
    assert!(counter.contains("TRACKED = %w[shop_images].freeze"), "{counter}");
    let program = read(&dir, "app/models/roundhouse_initializer_config_initializers_m.rb");
    let load = program.find("      M\n").unwrap_or_else(|| panic!("{program}"));
    let subscribe = program.find("ActiveSupport::Notifications.subscribe(\"probe.event\")").unwrap_or_else(|| panic!("{program}"));
    assert!(load < subscribe, "{program}");
    let nested = read(&dir, "app/models/roundhouse_initializer_config_initializers_shop_probe_counter.rb");
    assert!(nested.contains("      ShopProbe::Counter\n"), "{nested}");
    assert!(
        !errors.iter().any(|e| e.contains("module` body in an initializer") || e.contains("constant not supported")),
        "{errors:#?}"
    );
}

/// The emitted boot runs the boot files' statements, the environment's
/// framework write and the initializers' modules: ENV["X"] is "y", the
/// configuration answers `consider_all_requests_local` and `x.foo`, and
/// both subscribers read their initializer's constants. Boot loads the
/// `pg` gem (it never connects): run with `--ignored`, `GEM_PATH` and the
/// jemalloc `LD_PRELOAD` that has it.
#[test]
#[ignore = "fork spec: postgresql"]
fn boot_runs_the_boot_files_statements() {
    let (dir, _) = app().emit(BuildTarget::Ruby);
    let out = std::process::Command::new("ruby")
        .args([
            "-e",
            "require \"./boot\"\nputs Report.line\nc = Rails.application.config\n\
             p c.consider_all_requests_local\np c.x.foo\np c.x.greeting\np c.x.unset.class\np M::LIST\n\
             ActiveSupport::Notifications.instrument(\"probe.event\") {}\np Report.seen\n\
             ActiveSupport::Notifications.instrument(\"probe.nested\") {}\np Report.nested\n\
             c.consider_all_requests_local = false\np Rails.application.config.consider_all_requests_local\n",
        ])
        .current_dir(&*dir)
        .env("RAILS_ENV", "test")
        .env_remove("DATABASE_URL")
        .env_remove("ROUNDHOUSE_DATABASE_URL")
        .env_remove("X")
        .env_remove("NEVER")
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        [
            "[\"y\", \"1\", \"true\", nil]",
            "true",
            "1",
            "\"from application\"",
            "ActiveSupport::OrderedOptions",
            "[\"a\"]",
            "\"a\"",
            "\"shop_images:?\"",
            "false",
        ],
        "stderr={stderr}"
    );
    assert!(stderr.contains("error[file_statement_not_carried]: config/boot.rb:7"), "{stderr}");
    assert!(!stderr.contains("environment_statement_not_carried"), "{stderr}");
    assert!(!stderr.contains("initializer_statement_not_run"), "{stderr}");
}
