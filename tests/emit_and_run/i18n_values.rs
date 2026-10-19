//! `t` landing on an Array, a number or a boolean answers the value as it
//! is, from the first locale of the fallback chain that has it: forem's
//! calendar iterates `t("views.calendar.weekdays")` and passes the number
//! `t("views.tags.moderator.code")` to `tag.code`, mastodon maps
//! `I18n.t("date.order")`, an Array of Symbols in most of rails-i18n's
//! locales and of Strings in `en`. Each was an error. A key whose value
//! differs in kind between locales stays one. Expected values are i18n
//! 1.15.2's for the same files.

use super::emit_and_run;

const EN_YML: &str = "en:
  probe:
    days:
      - \"Mon\"
      - \"Tue\"
    order:
      - :year
      - :month
      - :day
    limit: 123
    ratio: 1.5
    enabled: true
";

const PL_YML: &str = "pl:
  probe:
    days:
      - \"Pn\"
      - \"Wt\"
    order:
      - day
      - month
      - year
";

fn values_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/locales/en.yml", EN_YML)
        .write("config/locales/pl.yml", PL_YML)
        .edit(
            "config/application.rb",
            "    # config.time_zone",
            "    config.i18n.available_locales = %i[en pl]\n    config.i18n.default_locale = :en\n    config.i18n.fallbacks = [ :en ]\n    # config.time_zone",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.days\n    [I18n.t(\"probe.days\"), I18n.with_locale(:pl) { I18n.t(\"probe.days\") }]\n  end\n\n  def self.orders\n    [I18n.t(\"probe.order\"), I18n.with_locale(:pl) { I18n.t(\"probe.order\") }, I18n.with_locale(:pl) { I18n.t(\"probe.order\").map(&:to_sym) }]\n  end\n\n  def self.scalars\n    [I18n.t(\"probe.limit\") + 1, I18n.t(\"probe.ratio\") * 2, I18n.t(\"probe.enabled\"), I18n.with_locale(:pl) { I18n.t(\"probe.limit\") }]\n  end\n",
        )
        .edit(
            "app/views/articles/index.html.erb",
            "<h1",
            "<p id=\"days\"><% t(\"probe.days\").each do |day| %>[<%= day %>]<% end %></p><p id=\"limit\"><%= t(\"probe.limit\") %></p>\n<h1",
        )
        .write(
            "test/controllers/values_controller_test.rb",
            "require \"test_helper\"\n\nclass ValuesControllerTest < ActionDispatch::IntegrationTest\n  test \"a template's values\" do\n    get articles_url\n    assert_includes response.body, \"<p id=\\\"days\\\">[Mon][Tue]</p>\"\n    assert_includes response.body, \"<p id=\\\"limit\\\">123</p>\"\n  end\nend\n",
        )
}

const ASSERTIONS: &str = r##"got = Article.days
raise got.inspect unless got == [["Mon", "Tue"], ["Pn", "Wt"]]
got = Article.orders
raise got.inspect unless got == [[:year, :month, :day], ["day", "month", "year"], [:day, :month, :year]]
got = Article.scalars
raise got.inspect unless got == [124, 3.0, true, 123]
puts "values passed"
"##;

#[test]
fn an_array_a_number_and_a_boolean_answer_as_they_are() {
    values_app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_array_a_number_and_a_boolean_answer_as_they_are_on_spinel() {
    values_app().run_spinel(ASSERTIONS).assert_passes();
}

#[test]
fn a_template_s_t_answers_a_value_as_it_is() {
    values_app().run_test("test/controllers/values_controller_test.rb").assert_passes();
}

#[test]
fn a_value_whose_kind_differs_between_locales_stays_an_error() {
    let (_emitted, errors) = values_app()
        .write("config/locales/pl.yml", "pl:\n  probe:\n    limit: true\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("`probe.limit` is not a String")), "{errors:#?}");
}
