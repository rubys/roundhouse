//! A parameter default runs as its method's code: a default reads the
//! parameters declared before it, a call it makes is a call site of
//! its callee, and a helper module's default reads the controller's
//! assigns as its body does.

use super::emit_and_run;

#[test]
fn a_default_reads_the_defaults_before_it() {
    let errors = emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "  def index\n",
            "  def labels(first = \"Hello\", second = first, third: second.no_such_label)\n    [first, second, third]\n  end\n\n  def index\n    @labels = labels\n",
        )
        .check_errors();
    assert!(errors.iter().any(|e| e.contains("`no_such_label`")), "{errors:#?}");
}

#[test]
fn a_call_in_a_default_types_its_callee_s_params() {
    let errors = emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "  def index\n",
            "  def prefix_for(article)\n    article.no_such_prefix\n  end\n\n  def tagged(label = prefix_for(Article.first))\n    label\n  end\n\n  def index\n    @tagged = tagged\n",
        )
        .check_errors();
    assert!(errors.iter().any(|e| e.contains("`no_such_prefix`")), "{errors:#?}");
}

#[test]
fn a_helper_default_reads_the_controller_s_assign() {
    emit_and_run::real_blog()
        .write("app/helpers/visitor_helper.rb", "module VisitorHelper\n  def visited_by(name = @visitor)\n    \"by #{name}\"\n  end\nend\n")
        .edit(
            "app/controllers/articles_controller.rb",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n    @visitor = \"Ann\"\n",
        )
        .edit("app/views/articles/index.html.erb", "<h1", "<p id=\"visitor\"><%= visited_by %></p>\n<h1")
        .write(
            "test/controllers/visitor_controller_test.rb",
            "require \"test_helper\"\n\nclass VisitorControllerTest < ActionDispatch::IntegrationTest\n  test \"a helper default reads the assign\" do\n    get articles_url\n    assert_includes response.body, \"<p id=\\\"visitor\\\">by Ann</p>\"\n  end\nend\n",
        )
        .run_test("test/controllers/visitor_controller_test.rb")
        .assert_passes();
}
