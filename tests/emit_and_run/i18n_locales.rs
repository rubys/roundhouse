//! `I18n.locale` is per request, as Rails has it: the app's
//! `config.i18n.available_locales`, `default_locale` and `fallbacks`
//! decide what `I18n.t`, a template's `t` and a controller's `t` answer
//! in each locale, and `I18n.locale =` / `I18n.with_locale` switch it
//! when the app runs. Expected values are i18n 1.15.2's for the same
//! translations and settings on CRuby 4.0.5.

use super::emit_and_run;

const EN_YML: &str = "en:
  probe:
    greet: \"Hello\"
    hi: \"Hi %{name}\"
    items:
      one: \"%{count} item\"
      other: \"%{count} items\"
    only_en: \"English only\"
";

const PL_YML: &str = "pl:
  probe:
    greet: \"Cześć\"
    items:
      one: \"%{count} rzecz\"
      other: \"%{count} rzeczy\"
";

const PROBE: &str = "  validates :title, presence: true

  def self.lookups
    [I18n.t(\"probe.greet\"), I18n.t(\"probe.hi\", name: \"<x>\"), I18n.t(\"probe.items\", count: 1), I18n.t(\"probe.items\", count: 2), I18n.t(\"probe.nope\"), I18n.t(:greet, scope: :probe, locale: :pl), I18n.t(\"probe.nope\", default: \"fallback %{n}\", n: 3), I18n.locale.to_s]
  end

  def self.polish
    I18n.with_locale(:pl) do
      [I18n.t(\"probe.items\", count: 1), I18n.t(\"probe.items\", count: 5), I18n.t(\"probe.only_en\"), I18n.t(\"probe.greet\"), I18n.t(\"probe.nope\"), probe_key(\"greet\"), I18n.t(\"probe.hi\", name: \"Ola\"), I18n.locale.to_s]
    end
  end

  def self.probe_key(key)
    I18n.t(\"probe.#{key}\")
  end

  def self.only_key(key)
    I18n.t(\"probe.only_#{key}\")
  end

  def self.named(locale)
    I18n.t(\"probe.greet\", locale: locale)
  end

  # The forwarded-block form, as an around_action writes it.
  def self.in_locale(locale, &block)
    I18n.with_locale(locale, &block)
  end
";

/// The probe model, two locales with English as the fallback, and an
/// `around_action` that runs each request in the `locale` it asks for.
pub(super) fn locales_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/locales/en.yml", EN_YML)
        .write("config/locales/pl.yml", PL_YML)
        .edit(
            "config/application.rb",
            "    # config.time_zone",
            "    config.i18n.available_locales = %i[en pl]\n    config.i18n.default_locale = :en\n    config.i18n.fallbacks = [ :en ]\n    # config.time_zone",
        )
        .edit("app/models/article.rb", "  validates :title, presence: true\n", PROBE)
        .edit(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n",
            "class ApplicationController < ActionController::Base\n  around_action :switch_locale\n\n  def switch_locale(&action)\n    I18n.with_locale(params[:locale] || I18n.default_locale, &action)\n  end\n",
        )
        .edit(
            "app/views/articles/index.html.erb",
            "<h1",
            "<p id=\"lang\"><%= I18n.locale %></p><p id=\"greet\"><%= t(\"probe.greet\") %></p><p id=\"only-en\"><%= t(\"probe.only_en\") %></p><p id=\"missing\"><%= t(\"probe.missing_key\") %></p>\n<h1",
        )
        .write(
            "test/controllers/locale_requests_controller_test.rb",
            "require \"test_helper\"

class LocaleRequestsControllerTest < ActionDispatch::IntegrationTest
  test \"each request renders in its locale\" do
    get articles_url(locale: \"pl\")
    assert_includes response.body, \"<p id=\\\"lang\\\">pl</p><p id=\\\"greet\\\">Cześć</p><p id=\\\"only-en\\\">English only</p>\"
    assert_includes response.body, \"<p id=\\\"missing\\\"><span class=\\\"translation_missing\\\" title=\\\"translation missing: pl.probe.missing_key\\\">Missing Key</span></p>\"
    get articles_url
    assert_includes response.body, \"<p id=\\\"lang\\\">en</p><p id=\\\"greet\\\">Hello</p>\"
    assert_includes response.body, \"title=\\\"translation missing: en.probe.missing_key\\\"\"
  end
end
",
        )
}

const LOCALE_ASSERTIONS: &str = r##"expect = lambda { |what, got, want| raise "#{what}: #{got.inspect}" unless got == want }
expect.call("lookups", Article.lookups, ["Hello", "Hi <x>", "1 item", "2 items", "Translation missing: en.probe.nope", "Cześć", "fallback 3", "en"])
expect.call("polish", Article.polish, ["1 rzecz", "5 rzeczy", "English only", "Cześć", "Translation missing: pl.probe.nope", "Cześć", "Hi Ola", "pl"])
expect.call("restored", I18n.locale, :en)
expect.call("dynamic miss", Article.in_locale(:pl) { [Article.only_key("en"), Article.only_key("xx")] }, ["English only", "Translation missing: pl.probe.only_xx"])
expect.call("settings", [I18n.available_locales, I18n.default_locale], [[:en, :pl], :en])
expect.call("named", [Article.named("pl"), Article.named(:en), Article.named(nil)], ["Cześć", "Hello", "Hello"])
expect.call("forwarded", Article.in_locale(:pl) { Article.lookups.first }, "Cześć")
expect.call("nil keeps", Article.in_locale(nil) { I18n.locale }, :en)
begin
  Article.named("de")
  raise "no raise"
rescue I18n::InvalidLocale => e
  expect.call("named de", e.message, "\"de\" is not a valid locale")
end
begin
  I18n.locale = :de
  raise "no raise"
rescue I18n::InvalidLocale => e
  expect.call("locale= de", e.message, ":de is not a valid locale")
end
I18n.locale = "pl"
expect.call("locale=", [I18n.locale, Article.lookups.first], [:pl, "Cześć"])
I18n.locale = :en
begin
  Article.in_locale(:pl) { raise ArgumentError, "inside" }
rescue ArgumentError
end
expect.call("restored after raise", I18n.locale, :en)
puts "locales passed"
"##;

#[test]
fn i18n_answers_in_the_locale_set_when_it_runs() {
    locales_app().run_ruby(LOCALE_ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn i18n_answers_in_the_locale_set_when_it_runs_on_spinel() {
    locales_app().run_spinel(LOCALE_ASSERTIONS).assert_passes();
}

#[test]
fn each_request_renders_in_its_locale() {
    locales_app().run_test("test/controllers/locale_requests_controller_test.rb").assert_passes();
}

/// An environment's own `config.i18n.fallbacks` wins there: with
/// `false` in development a Polish lookup no longer falls back to
/// English, and with `true` it falls back to the default locale.
#[test]
fn an_environment_s_fallbacks_win_there() {
    locales_app()
        .edit(
            "config/environments/development.rb",
            "Rails.application.configure do\n",
            "Rails.application.configure do\n  config.i18n.fallbacks = false\n",
        )
        .run_ruby(
            "raise Article.polish[2] unless Article.polish[2] == \"Translation missing: pl.probe.only_en\"\nputs \"env fallbacks passed\"\n",
        )
        .assert_passes();
    locales_app()
        .edit("config/application.rb", "    config.i18n.fallbacks = [ :en ]\n", "")
        .edit(
            "config/environments/development.rb",
            "Rails.application.configure do\n",
            "Rails.application.configure do\n  config.i18n.fallbacks = true\n",
        )
        .run_ruby("raise Article.polish[2] unless Article.polish[2] == \"English only\"\nputs \"env fallbacks passed\"\n")
        .assert_passes();
}

/// `config.i18n.fallbacks = [I18n.default_locale]` in an environment's
/// file reads the gem's `:en`: the file runs before the i18n railtie
/// applies the app's `default_locale` (here `:pl`), so a Polish lookup
/// still falls back to English.
#[test]
fn a_fallback_to_i18n_default_locale_reads_the_gem_s_default() {
    locales_app()
        .edit("config/application.rb", "    config.i18n.default_locale = :en
    config.i18n.fallbacks = [ :en ]
", "    config.i18n.default_locale = :pl
")
        .edit(
            "config/environments/development.rb",
            "Rails.application.configure do\n",
            "Rails.application.configure do\n  config.i18n.fallbacks = [I18n.default_locale]\n",
        )
        .run_ruby(
            "got = [I18n.default_locale, Article.polish[2]]\nraise got.inspect unless got == [:pl, \"English only\"]\nputs \"default fallback passed\"\n",
        )
        .assert_passes();
}

/// `config.i18n.available_locales` computed at boot: the locales a
/// translation can run in are not known at compile time, so a
/// translation the app's locale files give in several locales stays an
/// error naming that setting, instead of folding the default locale alone.
#[test]
fn a_computed_available_locales_stays_an_error() {
    let (_emitted, errors) = locales_app()
        .edit(
            "config/application.rb",
            "    config.i18n.available_locales = %i[en pl]\n",
            "    config.i18n.available_locales = ENV.fetch(\"LOCALES\", \"en pl\").split.map(&:to_sym)\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("`config.i18n.available_locales`: `available_locales = ENV.fetch")), "{errors:?}");
}
