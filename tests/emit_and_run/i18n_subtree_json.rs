//! `t("term.js").to_json` in a template: the key names a node of
//! Strings, which I18n answers as a nested Hash, and the template hands
//! its JSON to a script (hindmost's terminal page). The text is the one
//! Rails writes: keys in the files' order, an empty mapping as `{}`,
//! `<`, `>` and `&` escaped, U+2028 left alone under `load_defaults`
//! 8.1 and escaped where the app turns `escape_js_separators_in_json`
//! back on. Per locale, the first locale of the fallback chain that has
//! the node answers it whole. Expected values are Rails 8.1.3's
//! (ActionView rendering the same template over the same files).

use super::emit_and_run;

const EN_YML: &str = "en:
  articles:
    index:
      labels:
        second: \"2\"
        first: \"1\"
  term:
    js:
      zeta: \"Z <b>&amp;</b>\"
      alpha: \"Connecting to %{p}…\"
      nested:
        y: \"why\"
        b: \"bee\"
      empty: {}
      sep: \"a\\Lb\"
    only_en:
      hi: \"Hi\"
";

const PL_YML: &str = "pl:
  term:
    js:
      alpha: \"Łączenie\"
";

const VIEW: &str = r#"<script id="t">const T = <%= raw(t("term.js").to_json) %>;</script>
<p id="esc"><%= t("term.js.nested").to_json %></p>
<script id="only">const O = <%= raw(t("term.only_en").to_json) %>;</script>
<script id="lazy">const L = <%= raw(t(".labels").to_json) %>;</script>
<h1"#;

const TEST: &str = r##"require "test_helper"

class SubtreeJsonControllerTest < ActionDispatch::IntegrationTest
  ESC = ->(c) { "\\u%04x" % c }

  test "a subtree's to_json is the JSON Rails writes" do
    lt, gt, amp = ESC.(0x3c), ESC.(0x3e), ESC.(0x26)
    sep = ENV["SEP"] == "escaped" ? ESC.(0x2028) : [0x2028].pack("U")
    get articles_url
    assert_includes response.body, %(<script id="t">const T = {"zeta":"Z #{lt}b#{gt}#{amp}amp;#{lt}/b#{gt}","alpha":"Connecting to %{p}…","nested":{"y":"why","b":"bee"},"empty":{},"sep":"a#{sep}b"};</script>)
    assert_includes response.body, %(<p id="esc">{&quot;y&quot;:&quot;why&quot;,&quot;b&quot;:&quot;bee&quot;}</p>)
    assert_includes response.body, %(<script id="only">const O = {"hi":"Hi"};</script>)
    assert_includes response.body, %(<script id="lazy">const L = {"second":"2","first":"1"};</script>)
    I18n.with_locale(:pl) { get articles_url }
    assert_includes response.body, %(<script id="t">const T = {"alpha":"Łączenie"};</script>)
    assert_includes response.body, %(<p id="esc">{&quot;y&quot;:&quot;why&quot;,&quot;b&quot;:&quot;bee&quot;}</p>)
    assert_includes response.body, %(<script id="only">const O = {"hi":"Hi"};</script>)
  end
end
"##;

fn subtree_json_app(separators: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/locales/en.yml", EN_YML)
        .write("config/locales/pl.yml", PL_YML)
        .edit(
            "config/application.rb",
            "    # config.time_zone",
            &format!("    config.i18n.available_locales = %i[en pl]\n    config.i18n.default_locale = :en\n    config.i18n.fallbacks = [ :en ]\n{separators}    # config.time_zone"),
        )
        .edit("app/views/articles/index.html.erb", "<h1", VIEW)
        .write("test/controllers/subtree_json_controller_test.rb", TEST)
}

#[test]
fn a_subtree_s_to_json_is_the_json_rails_writes() {
    subtree_json_app("").run_test("test/controllers/subtree_json_controller_test.rb").assert_passes();
}

#[test]
fn the_app_s_escape_js_separators_setting_is_read() {
    let escaped = "    config.active_support.escape_js_separators_in_json = true\n";
    subtree_json_app(escaped)
        .edit("test/controllers/subtree_json_controller_test.rb", "ENV[\"SEP\"] == \"escaped\"", "true")
        .run_test("test/controllers/subtree_json_controller_test.rb")
        .assert_passes();
}
