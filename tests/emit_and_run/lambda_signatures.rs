//! A lambda keeps its optional, keyword and keyword-rest parameters, so
//! the emitted program answers what plain Ruby answers.

use super::lambda_signatures_contract as contract;

#[test]
fn lambdas_with_optional_and_keyword_parameters_run() {
    contract::overlay().run_ruby(&contract::assertions()).assert_passes();
}

/// A form builder block with an optional parameter has no lowering; the
/// emit reports it instead of rendering the form as nothing.
#[test]
fn a_form_block_with_an_optional_parameter_is_reported_not_dropped() {
    let (_tree, errors) = super::emit_and_run::real_blog()
        .edit("app/views/articles/_form.html.erb", "do |form| %>", "do |form, extra = 1| %>")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(
        errors.iter().any(|e| e.contains("builder block with optional or keyword parameters")),
        "{errors:#?}"
    );
}


/// A block callback whose parameters all have defaults (`|key: 7|`) has
/// arity 0, so Rails runs it with no argument and the default applies:
/// the spliced hook binds `key = 7` ahead of the body.
#[test]
fn a_block_callback_with_keyword_defaults_runs_with_them() {
    super::emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  before_save { |key: 7| self.body = \"#{body} #{key}\" }\n",
        )
        .run_ruby("a = Article.create!(title: \"T\", body: \"long enough body\")\nraise \"got #{a.body.inspect}\" unless a.body == \"long enough body 7\"\nputs \"ok\"\n")
        .assert_passes();
}

/// Folding callbacks into one hook must not make nested lambdas from
/// different callbacks capture the same generated default local.
#[test]
fn callbacks_keep_distinct_captured_default_locals() {
    super::emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  before_save { |a: \"first\", key: a| @callback = -> { key } }\n  before_save { |key: \"second\"| self.body = @callback.call }\n",
        )
        .run_ruby("article = Article.create!(title: \"T\", body: \"long enough body\")\nraise \"expected first callback's captured value, got #{article.body.inspect}\" unless article.body == \"first\"\nputs \"ok\"\n")
        .assert_passes();
}

/// A required keyword (`|key:|`) has no default to bind: the block is not
/// spliced into a hook body with `key` unbound.
#[test]
fn a_block_callback_with_a_required_keyword_is_not_spliced_unbound() {
    let (tree, errors) = super::emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  before_save { |key:| self.body = \"#{body} #{key}\" }\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let model = std::fs::read_to_string(tree.join("app/models/article.rb")).unwrap();
    assert!(!model.contains("#{key}"), "{model}");
}

/// A default reads the method's parameter after `param_rebind` renamed a
/// reassignment of it: `->(m = n)` must see the reassigned `n`.
#[test]
fn a_lambda_default_reads_a_rebound_method_parameter() {
    super::emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def self.rebound_default(n)\n    n = n.to_i + 1\n    f = ->(m = n) { m }\n    f.call\n  end\n",
        )
        .run_ruby("got = Article.rebound_default(1)\nraise \"expected 2, got #{got}\" unless got == 2\nputs \"ok\"\n")
        .assert_passes();
}

/// A view lambda's default (`->(label = @article.title)`) reads the view
/// local the action's ivar became, like the lambda's body does.
#[test]
fn a_view_lambda_default_reads_the_action_ivar() {
    super::emit_and_run::real_blog()
        .edit(
            "app/views/articles/show.html.erb",
            "<% content_for :title, \"Showing article\" %>\n",
            "<% content_for :title, \"Showing article\" %>\n<% shout = ->(label = @article.title) { label.upcase } %>\n<p id=\"shout\"><%= shout.call %></p>\n",
        )
        .write(
            "test/controllers/shouts_controller_test.rb",
            r##"require "test_helper"

class ShoutsControllerTest < ActionDispatch::IntegrationTest
  test "the default reads the article" do
    article = Article.create!(title: "Quiet", body: "A body long enough.")
    get "/articles/#{article.id}"
    assert_response :success
    assert_includes response.body, "QUIET"
  end
end
"##,
        )
        .run_test("test/controllers/shouts_controller_test.rb")
        .assert_passes();
}

/// A destructured parameter beside an optional one: the destructuring
/// binds through a temp (`destructure_block_params`) and the default
/// stays in the signature, in a `->` literal and in a block alike.
#[test]
fn a_destructured_parameter_beside_a_default_runs() {
    super::emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def self.destructured_defaults\n    pair = ->((a, b), scale = 10) { (a.to_i + b.to_i) * scale.to_i }\n    [pair.call([1, 2]), pair.call([1, 2], 2), [[3, 4]].map { |(a, b), bonus = 1| a.to_i * b.to_i + bonus.to_i }.sum].join(\"|\")\n  end\n",
        )
        .run_ruby("got = Article.destructured_defaults\nraise \"expected 30|6|4, got #{got}\" unless got == \"30|6|4\"\nputs \"ok\"\n")
        .assert_passes();
}
