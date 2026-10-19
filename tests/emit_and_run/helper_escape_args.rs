//! A helper that hands back its argument (`def framed(v) = v`) is as
//! safe as that argument, as Rails has it: a plain String or a record's
//! attribute passed through is escaped, a tag helper's markup passed
//! through is not. Every app helper was safe before, so the plain value
//! went out unescaped.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/helpers/frame_helper.rb",
            "module FrameHelper\n  def framed(value)\n    value\n  end\n\n  def labelled(value)\n    value || \"<none>\"\n  end\nend\n",
        )
        .edit(
            "app/views/articles/index.html.erb",
            "<h1",
            "<p id=\"plain\"><%= framed(\"<b>x</b>\") %></p><p id=\"markup\"><%= framed(link_to(\"home\", \"/\")) %></p><p id=\"fallback\"><%= labelled(nil) %></p><p id=\"attr\"><%= framed(Article.new(title: \"<i>t</i>\").title) %></p>\n<h1",
        )
        .write(
            "test/controllers/frame_controller_test.rb",
            "require \"test_helper\"\n\nclass FrameControllerTest < ActionDispatch::IntegrationTest\n  test \"helper arguments\" do\n    get articles_url\n    assert_includes response.body, \"<p id=\\\"plain\\\">&lt;b&gt;x&lt;/b&gt;</p>\"\n    assert_includes response.body, \"<p id=\\\"markup\\\"><a href=\\\"/\\\">home</a></p>\"\n    assert_includes response.body, \"<p id=\\\"fallback\\\">&lt;none&gt;</p>\"\n    assert_includes response.body, \"<p id=\\\"attr\\\">&lt;i&gt;t&lt;/i&gt;</p>\"\n  end\nend\n",
        )
}

#[test]
fn a_helper_returning_its_argument_is_as_safe_as_the_argument() {
    app().run_test("test/controllers/frame_controller_test.rb").assert_passes();
}
