//! A `t` the lowering cannot fold — its key, scope, default or options
//! known only when it runs — reads the app's whole catalog at run time,
//! as the gem does. Expected values are i18n 1.15.2's for the same
//! translations and calls on CRuby 3.4.

use super::emit_and_run;
use super::i18n_locales_app::locales_app;

const RUNTIME: &str = "  def self.full_key(key)
    I18n.t(key)
  end

  def self.scoped(scope)
    I18n.t(:greet, scope: scope)
  end

  def self.with_default(default)
    I18n.t(\"probe.nope\", default: default)
  end

  def self.with_options(options)
    I18n.t(\"probe.hi\", **options)
  end

  def self.counted(key, count)
    I18n.t(key, count: count)
  end

  def self.lookups
";

fn runtime_app() -> emit_and_run::Overlay {
    locales_app()
        .edit("app/models/article.rb", "  def self.lookups\n", RUNTIME)
        .edit(
            "app/views/articles/index.html.erb",
            "<h1",
            "<p id=\"runtime\"><%= t(params[:key] || \"probe.greet\", name: \"<b>\") %></p>\n<h1",
        )
        .write(
            "test/controllers/runtime_translate_controller_test.rb",
            "require \"test_helper\"

class RuntimeTranslateControllerTest < ActionDispatch::IntegrationTest
  test \"a key from the request is looked up when it runs\" do
    get articles_url(key: \"probe.hi\")
    assert_includes response.body, \"<p id=\\\"runtime\\\">Hi &lt;b&gt;</p>\"
    get articles_url(locale: \"pl\")
    assert_includes response.body, \"<p id=\\\"runtime\\\">Cześć</p>\"
  end
end
",
        )
}

const RUNTIME_ASSERTIONS: &str = r##"expect = lambda { |what, got, want| raise "#{what}: #{got.inspect}" unless got == want }
expect.call("en", [Article.full_key("probe.greet"), Article.full_key("probe.missing"), Article.scoped("probe"), Article.scoped([:probe]), Article.with_default("plain"), Article.with_default(:"probe.greet"), Article.with_default([:"probe.nope2", "last"]), Article.with_options({ name: "Al" }), Article.counted("probe.items", 1), Article.counted("probe.items", 3)],
  ["Hello", "Translation missing: en.probe.missing", "Hello", "Hello", "plain", "Hello", "last", "Hi Al", "1 item", "3 items"])
expect.call("pl", Article.in_locale(:pl) { [Article.full_key("probe.only_en"), Article.counted("probe.items", 1), Article.full_key("probe.missing")] },
  ["English only", "1 rzecz", "Translation missing: pl.probe.missing"])
expect.call("considered", Article.with_default([:"probe.nope2", :"probe.nope3"]), "Translation missing. Options considered were:\n- en.probe.nope\n- en.probe.nope2\n- en.probe.nope3")
begin
  Article.with_options({ other: 1 })
  raise "no raise"
rescue I18n::MissingInterpolationArgument => e
  expect.call("missing argument", e.message, "missing interpolation argument :name in \"Hi %{name}\" ({other: 1} given)")
end
puts "runtime passed"
"##;

#[test]
fn a_translation_known_only_when_it_runs_reads_the_catalog() {
    runtime_app().run_ruby(RUNTIME_ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_translation_known_only_when_it_runs_reads_the_catalog_on_spinel() {
    runtime_app().run_spinel(RUNTIME_ASSERTIONS).assert_passes();
}

#[test]
fn a_template_key_from_the_request_is_looked_up_and_escaped() {
    runtime_app().run_test("test/controllers/runtime_translate_controller_test.rb").assert_passes();
}

/// A strict target carries no catalog to read, so the call stays
/// unsupported there rather than emit a call to nothing.
#[test]
fn a_translation_known_only_when_it_runs_is_unsupported_on_strict_targets() {
    let (_emitted, errors) = runtime_app().emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.iter().any(|e| e.contains("this target's runtime carries the locale-chosen table, not the catalog")), "{errors:#?}");
}
