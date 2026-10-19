//! An `app/helpers` module's methods run in the view, so their bare `t`
//! is ActionView's: it folds from the catalog, and an `_html` key
//! answers html with its interpolation values escaped. Left as a bare
//! `t`, the emitted module raised `undefined method 't'` the first time
//! a page called it. A controller that includes the helper but never
//! calls the method holds a copy whose `t` is the view's too (mastodon's
//! `RegistrationHelper`); one it calls stays the controller's, where an
//! `_html` key's html-safe mark has nowhere to go.

use super::emit_and_run;

fn helper_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/locales/en.yml", "en:\n  hello: \"Hello world\"\n  probe:\n    agree_html: \"I agree to <a href=\\\"%{path}\\\">the terms</a> of %{name}\"\n")
        .write("app/helpers/agreement_helper.rb", "module AgreementHelper\n  def greeting_label\n    t('hello')\n  end\n\n  def agreement_label\n    t('probe.agree_html', path: '/terms', name: '<b>us</b>')\n  end\nend\n")
        .edit("app/views/articles/index.html.erb", "<h1", "<p id=\"greet\"><%= greeting_label %></p><p id=\"agree\"><%= agreement_label %></p>\n<h1")
        .write(
            "test/controllers/agreement_controller_test.rb",
            "require \"test_helper\"\n\nclass AgreementControllerTest < ActionDispatch::IntegrationTest\n  test \"helper translations\" do\n    get articles_url\n    assert_includes response.body, \"<p id=\\\"greet\\\">Hello world</p>\"\n    assert_includes response.body, \"<p id=\\\"agree\\\">I agree to <a href=\\\"/terms\\\">the terms</a> of &lt;b&gt;us&lt;/b&gt;</p>\"\n  end\nend\n",
        )
}

#[test]
fn a_helper_s_t_is_action_view_s() {
    helper_app().run_test("test/controllers/agreement_controller_test.rb").assert_passes();
}

fn included(app: emit_and_run::Overlay) -> emit_and_run::Overlay {
    app.edit("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n", "class ArticlesController < ApplicationController\n  include AgreementHelper\n")
}

#[test]
fn a_controller_s_uncalled_copy_takes_the_view_s_t() {
    included(helper_app()).run_test("test/controllers/agreement_controller_test.rb").assert_passes();
}

#[test]
fn a_controller_s_called_copy_keeps_the_controller_s_t() {
    let (_emitted, errors) = included(helper_app())
        .edit("app/controllers/articles_controller.rb", "  def index\n", "  def index\n    @label = agreement_label\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("an `_html` key in a controller")), "{errors:#?}");
}

fn with_defaults(app: emit_and_run::Overlay) -> emit_and_run::Overlay {
    app.edit(
        "app/helpers/agreement_helper.rb",
        "  def greeting_label\n",
        "  def framed_label(label = t('probe.agree_html', path: '/t', name: '<n>'))\n    label\n  end\n\n  def framed_title(title: t('probe.agree_html', path: '/k', name: '<k>'))\n    title\n  end\n\n  def framed_link(text = link_to('x', '/y'), suffix = greeting_label)\n    text + suffix\n  end\n\n  def greeting_label\n",
    )
    .edit("app/views/articles/index.html.erb", "<h1", "<p id=\"fl\"><%= framed_label %></p><p id=\"ft\"><%= framed_title %></p><p id=\"fk\"><%= framed_link %></p>\n<h1")
    .edit(
        "test/controllers/agreement_controller_test.rb",
        "    get articles_url\n",
        "    get articles_url\n    assert_includes response.body, \"<p id=\\\"fl\\\">I agree to <a href=\\\"/t\\\">the terms</a> of &lt;n&gt;</p>\"\n    assert_includes response.body, \"<p id=\\\"ft\\\">I agree to <a href=\\\"/k\\\">the terms</a> of &lt;k&gt;</p>\"\n    assert_includes response.body, \"<p id=\\\"fk\\\"><a href=\\\"/y\\\">x</a>Hello world</p>\"\n",
    )
}

/// A parameter default is helper code as the body is: its `t` is
/// ActionView's in the module and in a copy no controller calls, and
/// its view helpers and sibling helpers resolve as the body's do. Each
/// was left a bare call the emitted module function does not answer,
/// and the copy's defaults were never checked, so nothing reported it.
#[test]
fn an_uncalled_copy_s_parameter_defaults_take_the_view_s_t() {
    with_defaults(included(helper_app())).run_test("test/controllers/agreement_controller_test.rb").assert_passes();
}

#[test]
fn a_called_copy_s_parameter_default_keeps_the_controller_s_t() {
    let (_emitted, errors) = with_defaults(included(helper_app()))
        .edit("app/controllers/articles_controller.rb", "  def index\n", "  def index\n    @label = framed_title\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("an `_html` key in a controller")), "{errors:#?}");
}

/// A name in a controller outside the copy's family does not call it.
#[test]
fn only_the_copy_s_own_family_calls_it() {
    let (_emitted, errors) = helper_app()
        .edit("app/controllers/comments_controller.rb", "class CommentsController < ApplicationController\n", "class CommentsController < ApplicationController\n  include AgreementHelper\n")
        .edit("app/controllers/articles_controller.rb", "  def index\n", "  def index\n    @probe = respond_to?(:agreement_label)\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("an `_html` key in a controller")), "{errors:#?}");
}

#[test]
fn a_keyword_default_counts_as_a_call() {
    let (_emitted, errors) = included(helper_app())
        .edit("app/controllers/articles_controller.rb", "  def index
", "  def labelled(label: agreement_label)
    label
  end

  def index
")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("an `_html` key in a controller")), "{errors:#?}");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_helper_s_t_is_action_view_s_on_spinel() {
    included(helper_app())
        .edit("app/models/article.rb", "  validates :title, presence: true\n", "  validates :title, presence: true\n\n  def self.labels\n    [AgreementHelper.greeting_label, AgreementHelper.agreement_label]\n  end\n")
        .run_spinel("got = Article.labels\nraise got.inspect unless got == [\"Hello world\", \"I agree to <a href=\\\"/terms\\\">the terms</a> of &lt;b&gt;us&lt;/b&gt;\"]\nputs \"helper t passed\"\n")
        .assert_passes();
}
