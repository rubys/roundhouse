//! PostgreSQL: i18n railtie runtime, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
//
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

const APPLICATION: &str = r#"require "rails"
require "active_record/railtie"
require "action_controller/railtie"
require "action_view/railtie"

module I18nProbe
  class Application < Rails::Application
    config.load_defaults 8.1
    config.i18n.available_locales = [:en, :fr, :"fr-CA"]
    config.i18n.default_locale = :en
    config.i18n.fallbacks = [:en]
    config.i18n.raise_on_missing_translations = true
    config.i18n.locale = false
  end
end
"#;

const LOCK: &str = "PATH\n  remote: engines/widgets\n  specs:\n    widgets (0.1.0)\n\n\
GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n    phonelib (0.10.27)\n\n\
DEPENDENCIES\n  i18n\n  phoneish!\n  phonelib\n  widgets!\n";

const INITIALIZER: &str = r#"Rails.application.config.after_initialize do
  Rails.application.config.i18n.after_initialize_saw = Rails.application.config.i18n.default_locale
end

Rails.application.config.i18n.enforce_available_locales = false
"#;

fn probe() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"i18n\"\ngem \"phoneish\"\ngem \"phonelib\"\ngem \"widgets\", path: \"engines/widgets\"\n")
        .write("Gemfile.lock", LOCK)
        .write("config.ru", "")
        .write("config/application.rb", APPLICATION)
        .write("config/environments/test.rb", "Rails.application.configure do\n  config.eager_load = ENV[\"PROBE_EAGER\"] == \"1\"\nend\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_i18n_railtie_unused\n")
        .write("config/initializers/i18n.rb", INITIALIZER)
        .write("config/locales/en.yml", "en:\n  probe:\n    hello: \"Hello from the app\"\n    shared: \"app wins\"\n")
        .write("config/locales/fr.yml", "fr:\n  probe:\n    hello: \"Bonjour de l'app\"\n")
        .write("config/locales/models/en.yml", "en:\n  probe:\n    model: \"nested app locale\"\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"title\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/probe.rb", "class Probe < ApplicationRecord\n  def self.phone_library\n    Phoneish\n  end\nend\n")
        .write("vendor/gems/phoneish-1.0.0/lib/phoneish.rb", "require \"phonelib\"\n\nmodule Phoneish\nend\n")
        .write("engines/widgets/widgets.gemspec", "Gem::Specification.new do |s|\n  s.name = \"widgets\"\n  s.version = \"0.1.0\"\n  s.summary = \"probe engine\"\n  s.authors = [\"probe\"]\n  s.files = Dir[\"lib/**/*\", \"config/**/*\"]\nend\n")
        .write("engines/widgets/lib/widgets.rb", "require \"widgets/engine\"\n\nmodule Widgets\nend\n")
        .write("engines/widgets/lib/widgets/engine.rb", "require \"rails\"\n\nmodule Widgets\n  class Engine < Rails::Engine\n  end\nend\n")
        .write("engines/widgets/config/locales/widgets.en.yml", "en:\n  widgets:\n    title: \"Widgets\"\n  probe:\n    shared: \"engine loses\"\n")
        .write("engines/widgets/config/locales/fr/widgets.fr.yml", "fr:\n  widgets:\n    title: \"Gadgets\"\n")
}

#[test]
#[ignore = "fork spec: postgresql layout"]
fn config_i18n_engine_locales_and_a_gem_railtie_boot_like_rails() {
    let (tree, _) = probe().emit(BuildTarget::Ruby);
    let app = tree.parent().expect("scratch").join("app");
    let out = emit_and_run::ruby()
        .arg(root().join("tests/i18n_railtie_runtime.rb"))
        .args(["compare".as_ref(), app.as_os_str(), tree.as_os_str()])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("Native Rails and emitted I18n::Railtie agree"),
        "I18n::Railtie diverged from Rails\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    // Not a vacuous agreement: each source reached I18n, in Rails' order,
    // and eager loading ran the namespaces' eager_load!.
    for expected in [
        "load_path: [\"phonelib-0.10.27/lib/phonelib/locale/en.yml\", \"active_support/locale/en.yml\", \"active_support/locale/en.rb\", \"active_model/locale/en.yml\", \"active_record/locale/en.yml\", \"action_view/locale/en.yml\", \"engines/widgets/config/locales/fr/widgets.fr.yml\", \"engines/widgets/config/locales/widgets.en.yml\", \"config/locales/en.yml\", \"config/locales/fr.yml\", \"config/locales/models/en.yml\"]",
        "eager_load_namespaces: [\"I18n\", \"Widgets::Engine\", \"Phonelib\"]",
        "t probe.shared: \"\\\"app wins\\\"\"",
        "t widgets.title fr-CA: \"\\\"Gadgets\\\"\"",
        "t disabled: \"I18n::Disabled:",
        // The initializer's write, applied: no InvalidLocale for :de.
        "enforce_available_locales: false\n",
        "t de: \"\\\"Hello from the app\\\"\"",
        "config.i18n keys: [:railties_load_path, :load_path, :available_locales, :default_locale, :raise_on_missing_translations, :locale, :after_initialize_saw]",
        "t missing: \"I18n::MissingTranslationData:",
        "== eager=\"1\"\nbackend initialized: true\nphonelib data loaded: true\n",
        "== eager=nil\nbackend initialized: false\nphonelib data loaded: false\n",
    ] {
        assert!(stdout.contains(expected), "missing {expected:?}:\n{stdout}");
    }
}

#[test]
#[ignore = "fork spec: postgresql layout"]
fn the_tree_loads_the_railtie_before_the_gems_and_writes_config_i18n_before_boot() {
    let (tree, errors) = probe().emit(BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("i18n")), "{errors:#?}");
    let read = |path: &str| std::fs::read_to_string(tree.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    for (emitted, carried) in [
        ("runtime/active_support_i18n_railtie.rb", "runtime/spinel/active_support_i18n_railtie.rb"),
        ("runtime/rails_framework_locales.rb", "runtime/spinel/rails_framework_locales.rb"),
        ("runtime/active_model/locale/en.yml", "runtime/spinel/active_model/locale/en.yml"),
        ("runtime/active_record/locale/en.yml", "runtime/spinel/active_record/locale/en.yml"),
        ("runtime/action_view/locale/en.yml", "runtime/spinel/action_view/locale/en.yml"),
    ] {
        assert_eq!(read(emitted), std::fs::read_to_string(root().join(carried)).unwrap(), "{emitted}");
    }
    assert!(read("runtime/features/active_support/i18n_railtie.rb").contains("require_relative \"../../active_support_i18n_railtie\""));
    let facades = read("runtime/gem_facades.rb");
    let at = |needle: &str| facades.find(needle).unwrap_or_else(|| panic!("{needle:?} not in\n{facades}"));
    let i18n = at("require_relative \"active_support_i18n\"\n");
    let railtie = at("require_relative \"rails_railtie\"\n");
    let engine = at("require_relative \"rails_engine\"\n");
    let i18n_railtie = at("require_relative \"active_support_i18n_railtie\"\n");
    let locales = at("require_relative \"rails_framework_locales\"\n");
    let phonelib = at("[\"phonelib\"].each do |feature|");
    assert!(i18n < railtie && railtie < engine && engine < i18n_railtie && i18n_railtie < locales && locales < phonelib, "{facades}");
    let models = read("app/models.rb");
    let writes = "Rails.application.config.i18n.available_locales = Rails.application.i18n_available_locales\n\
                  Rails.application.config.i18n.default_locale = Rails.application.i18n_default_locale\n\
                  Rails.application.config.i18n.fallbacks = Rails.application.i18n_fallbacks\n\
                  Rails.application.config.i18n.raise_on_missing_translations = Rails.application.i18n_raise_on_missing_translations\n\
                  Rails.application.config.i18n.locale = Rails.application.i18n_locale\n\
                  Rails.application.config.i18n.enforce_available_locales = Rails.application.i18n_enforce_available_locales\n";
    let block = models.find(writes).unwrap_or_else(|| panic!("{models}"));
    let boot = models.find("Rails::Railtie.run_initializers_of_railties(Rails.application, :boot)").expect("boot phase");
    assert!(block < boot, "{models}");
}
