use super::emit_and_run;

pub const EN_YML: &str = "en:
  probe:
    greet: \"Hello\"
    hi: \"Hi %{name}\"
    items:
      one: \"%{count} item\"
      other: \"%{count} items\"
    only_en: \"English only\"
";

pub const PL_YML: &str = "pl:
  probe:
    greet: \"Cześć\"
    items:
      one: \"%{count} rzecz\"
      other: \"%{count} rzeczy\"
";

pub const PROBE: &str = "  validates :title, presence: true

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

// Cut for TypeScript, which does not yet emit a method's `&block` parameter
pub const IN_LOCALE: &str = "  # The forwarded-block form, as an around_action writes it.
  def self.in_locale(locale, &block)
    I18n.with_locale(locale, &block)
  end
";

pub fn probe_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/locales/en.yml", EN_YML)
        .write("config/locales/pl.yml", PL_YML)
        .edit(
            "config/application.rb",
            "    # config.time_zone",
            "    config.i18n.available_locales = %i[en pl]\n    config.i18n.default_locale = :en\n    config.i18n.fallbacks = [ :en ]\n    # config.time_zone",
        )
        .edit("app/models/article.rb", "  validates :title, presence: true\n", PROBE)
}

pub fn locales_app() -> emit_and_run::Overlay {
    probe_app()
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

