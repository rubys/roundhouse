//! rails-i18n's `rails/locale/<locale>.yml` load beneath the app's own
//! translations, for each available locale, read from where Bundler
//! installed the gem. Expected values are i18n 1.15.2's with rails-i18n
//! 8.1.0's `pl.yml` and the same app file on CRuby 3.4.

use super::emit_and_run;

/// The keys these checks read, as rails-i18n 8.1.0's `pl.yml` has them.
const GEM_PL_YML: &str = "pl:
  errors:
    format: \"%{attribute} %{message}\"
    messages:
      blank: nie może być puste
      taken: zostało już zajęte
  helpers:
    submit:
      create: Utwórz %{model}
";

const PROBE: &str = "  validates :title, presence: true

  def self.polish_messages
    I18n.with_locale(:pl) do
      [I18n.t(\"errors.messages.blank\"), I18n.t(\"errors.messages.taken\"), I18n.t(\"helpers.submit.create\", model: \"Artykuł\")]
    end
  end

  def self.english_blank
    I18n.t(\"errors.messages.blank\")
  end
";

fn rails_i18n_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write("vendor/bundle/ruby/3.4.0/gems/rails-i18n-8.1.0/rails/locale/pl.yml", GEM_PL_YML)
        .write("config/locales/pl.yml", "pl:\n  errors:\n    messages:\n      taken: \"już zajęte (app)\"\n")
        .edit("Gemfile.lock", "    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n", "    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n    rails-i18n (8.1.0)\n      i18n (>= 0.7, < 2)\n")
        .edit(
            "config/application.rb",
            "    # config.time_zone",
            "    config.i18n.available_locales = %i[en pl]\n    config.i18n.default_locale = :en\n    config.i18n.fallbacks = [ :en ]\n    # config.time_zone",
        )
        .edit("app/models/article.rb", "  validates :title, presence: true\n", PROBE)
}

const ASSERTIONS: &str = r##"expect = lambda { |what, got, want| raise "#{what}: #{got.inspect}" unless got == want }
expect.call("pl", Article.polish_messages, ["nie może być puste", "już zajęte (app)", "Utwórz Artykuł"])
expect.call("en", Article.english_blank, "can't be blank")
puts "rails-i18n locales passed"
"##;

#[test]
fn rails_i18n_s_translations_answer_beneath_the_app_s() {
    rails_i18n_app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn rails_i18n_s_translations_answer_beneath_the_app_s_on_spinel() {
    rails_i18n_app().run_spinel(ASSERTIONS).assert_passes();
}

/// Not installed, its translations cannot be read: a warning says so
/// rather than every key it defines answering as missing unannounced.
#[test]
fn a_rails_i18n_that_is_not_installed_is_reported() {
    let warnings = rails_i18n_app().remove("vendor/bundle/ruby/3.4.0/gems/rails-i18n-8.1.0/rails/locale/pl.yml").remove(".bundle/config").ingest_warnings();
    assert!(warnings.iter().any(|w| w.contains("rails-i18n 8.1.0 is in Gemfile.lock but installed in no gem root")), "{warnings:#?}");
    assert!(!rails_i18n_app().ingest_warnings().iter().any(|w| w.contains("rails-i18n")));
}

/// Its plural rules are 8.1.0's, which another version is told.
#[test]
fn another_rails_i18n_version_is_told_its_rules_are_8_1_0_s() {
    let warnings = rails_i18n_app()
        .edit("Gemfile.lock", "    rails-i18n (8.1.0)\n", "    rails-i18n (7.0.10)\n")
        .ingest_warnings();
    assert!(warnings.iter().any(|w| w.contains("rails-i18n 7.0.10: plural rules are read as rails-i18n 8.1.0's")), "{warnings:#?}");
}
