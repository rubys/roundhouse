//! PostgreSQL: environment programs, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const APPLICATION: &str = r#"require_relative "boot"

module Probe
  class Application < Rails::Application
    config.x.greeting = "from application"
  end
end
"#;

const TEST_ENV: &str = r#"require "settings"
require "no_such_feature"

Rails.application.configure do
  config.eager_load = false
  config.cache_classes = true
  config.action_mailer.delivery_method = :test
  config.active_record.maintain_test_schema = false

  host = "test.example"
  config.x.greeting = "from test"
  config.app_host = host
  config.paths["log"] = "log/test.log"
  config.colorize_logging = false

  if ENV["NEVER_SET_IN_THIS_TEST"]
    config.cache_classes = false
  end

  config.to_prepare do
    Settings.uri = "https://#{Settings.host}"
  end
end

Settings.host = "test.example"
Settings.internal_uri = Settings.host
"#;

const DEVELOPMENT_ENV: &str = r#"require "settings"

Rails.application.configure do
  config.x.greeting = "from development"
end

Settings.host = "dev.example"
"#;

const SETTINGS: &str = r#"module Settings
  class << self
    attr_accessor :host, :uri, :internal_uri
  end
end
"#;

const REPORT: &str = r#"class Report
  def self.line
    [Settings.host, Settings.uri, Settings.internal_uri, Rails.application.config.x.greeting].inspect
  end

  def self.app_host
    Rails.application.config.app_host
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_environment_programs\n")
        .write("config/application.rb", APPLICATION)
        .write("config/environments/test.rb", TEST_ENV)
        .write("config/environments/development.rb", DEVELOPMENT_ENV)
        .write("lib/settings.rb", SETTINGS)
        .write("lib/report.rb", REPORT)
}

#[test]
#[ignore = "fork spec: postgresql"]
fn environment_files_become_programs_selected_by_rails_env() {
    let (dir, errors) = app().emit(BuildTarget::Ruby);
    let models = std::fs::read_to_string(dir.join("app/models.rb")).expect("app/models.rb");
    let case = models.find("case Rails.env.to_s\nwhen \"development\"").unwrap_or_else(|| panic!("{models}"));
    let test = case + models[case..].find("\nwhen \"test\"\n").unwrap_or_else(|| panic!("{models}"));
    let run = models
        .find("  RoundhouseEnvironmentTest.run_environment\n")
        .unwrap_or_else(|| panic!("{models}"));
    let railties = models
        .find("Rails::Railtie.run_initializers_of_railties(Rails.application, :boot)")
        .unwrap_or_else(|| panic!("{models}"));
    assert!(case < test && test < run && run < railties, "{models}");
    assert!(
        models.contains("  # config/environments/test.rb: 9 carried, 2 consumed, 4 not carried\n"),
        "{models}"
    );
    assert!(models.contains("but config/environments/#{Rails.env}.rb does not exist.\""), "{models}");
    let named: Vec<&str> = models.lines().filter(|l| l.starts_with("  $stderr.puts \"roundhouse: error[environment_statement_not_carried]")).collect();
    assert_eq!(named.len(), 4, "{models}");
    assert!(
        named[0].contains("config/environments/test.rb:2 require \\\"no_such_feature\\\": `require \\\"no_such_feature\\\"`: not a runtime port"),
        "{}",
        named[0]
    );
    assert!(
        named[1].contains(
            "config/environments/test.rb:7 config.action_mailer.delivery_method = :test: `config.action_mailer.delivery_method` is framework configuration"
        ),
        "{}",
        named[1]
    );
    // What `Rails.application.config` answers itself is framework
    // configuration, never lifted as an app key over the runtime's method.
    assert!(named[2].contains("test.rb:13 config.paths[\\\"log\\\"] = \\\"log/test.log\\\": `config.paths` is framework configuration"), "{}", named[2]);
    assert!(named[3].contains("test.rb:14 config.colorize_logging = false: `config.colorize_logging` is framework configuration"), "{}", named[3]);
    assert!(!models.contains("def paths__build") && !std::fs::read_to_string(dir.join("config/application.rb")).unwrap().contains("def paths__build"));
    // The same four, as `check` reports them.
    let gaps: Vec<&String> = errors.iter().filter(|e| e.contains("initializer_unsupported") || e.contains("is framework configuration") || e.contains("no_such_feature")).collect();
    assert_eq!(gaps.len(), 4, "{errors:#?}");
    let program = std::fs::read_to_string(dir.join("app/models/roundhouse_environment_test.rb")).expect("holder");
    assert!(program.contains("Rails.application.x_greeting = \"from test\""), "{program}");
    assert!(program.contains("Rails.application.app_host = host"), "{program}");
    assert!(program.contains("Rails.application.active_record_maintain_test_schema = false"), "{program}");
    assert!(!program.contains("delivery_method"), "{program}");
    assert!(!program.contains("cache_classes"), "{program}");
    assert!(!program.contains("eager_load"), "{program}");
    let helper = std::fs::read_to_string(dir.join("test/test_helper.rb")).expect("test_helper.rb");
    let env = helper.find("ENV[\"RAILS_ENV\"] ||= \"test\"\nrequire_relative \"../boot\"").unwrap_or_else(|| panic!("{helper}"));
    assert!(env < helper.find("DatabaseConfiguration.configure!").unwrap(), "{helper}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn boot_runs_the_selected_environment_program() {
    let (dir, _) = app().emit(BuildTarget::Ruby);
    let run = |env: &str| {
        let out = std::process::Command::new("ruby")
            .args([
                "-e",
                "require \"./boot\"\nputs Report.line\nputs Rails.application.active_record_maintain_test_schema.inspect\n\
                 begin\n  puts Report.app_host\nrescue NoMethodError => e\n  puts \"NoMethodError: #{e.message}\"\nend\n",
            ])
            .current_dir(&*dir)
            .env("RAILS_ENV", env)
            .env_remove("DATABASE_URL")
            .env_remove("ROUNDHOUSE_DATABASE_URL")
            .output()
            .expect("ruby");
        (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    let (ok, stdout, stderr) = run("test");
    assert!(ok, "stdout={stdout}\nstderr={stderr}");
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        [
            "[\"test.example\", \"https://test.example\", \"test.example\", \"from test\"]",
            "false",
            "test.example",
        ],
        "stderr={stderr}"
    );
    let named: Vec<&str> = stderr.lines().filter(|l| l.contains("error[environment_statement_not_carried]")).collect();
    assert_eq!(named.len(), 4, "{stderr}");
    assert!(!stderr.contains("initializer_statement_not_run"), "{stderr}");

    let (ok, stdout, stderr) = run("development");
    assert!(ok, "stdout={stdout}\nstderr={stderr}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "[\"dev.example\", nil, nil, \"from development\"]", "stderr={stderr}");
    assert_eq!(lines[1], "true", "stderr={stderr}");
    assert!(lines[2].starts_with("NoMethodError: undefined method 'app_host'"), "{stdout}");
    assert!(!stderr.contains("environment_statement_not_carried"), "{stderr}");

    let (ok, _, stderr) = run("staging");
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("Rails environment has been set to staging but config/environments/staging.rb does not exist."),
        "{stderr}"
    );
}
