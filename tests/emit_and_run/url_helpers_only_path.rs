//! `x_url(…, only_path: true)` is Rails asking the URL spelling for a
//! PATH. `url_for` deletes `only_path:` with the rest of
//! `RouteSet::RESERVED_OPTIONS` before it generates, and returns the
//! path with no scheme and no host, whichever class calls it.
//!
//! Every expected value below is Rails 8.1.4's on the same overlay.
//! Before the fix the model, service and mailer spellings raised
//! `ArgumentError` (`only_path:` reached `RouteHelpers.article_path`,
//! which has no parameter for it), the controller's bare call rendered
//! the request's absolute URL, and the controller's chain raised
//! `unknown keyword: :only_path`.

use super::emit_and_run;

const MODEL_METHODS: &str = r##"
  def only_path_link
    Rails.application.routes.url_helpers.article_url(self, only_path: true)
  end

  def only_path_collection_link
    Rails.application.routes.url_helpers.articles_url(only_path: true)
  end
"##;

const SERVICE: &str = r##"class ArticleLinks
  def initialize(article)
    @article = article
  end

  def path
    Rails.application.routes.url_helpers.article_url(@article, only_path: true)
  end
end
"##;

const MAILER: &str = r##"class ArticleMailer < ApplicationMailer
  def link_for(article)
    Rails.application.routes.url_helpers.article_url(article, only_path: true)
  end
end
"##;

const CONTROLLER: &str = r##"class LinksController < ApplicationController
  def show
    article = Article.find(params[:id])
    render plain: "#{article_url(article, only_path: true)} #{Rails.application.routes.url_helpers.article_url(article, only_path: true)}"
  end
end
"##;

const TEST: &str = r##"require "test_helper"

class LinksControllerTest < ActionDispatch::IntegrationTest
  test "a model's url_helpers _url with only_path is the path" do
    article = Article.first
    assert_equal "/articles/#{article.id}", article.only_path_link
    assert_equal "/articles", article.only_path_collection_link
  end

  test "a plain class's url_helpers _url with only_path is the path" do
    article = Article.first
    assert_equal "/articles/#{article.id}", ArticleLinks.new(article).path
  end

  test "a mailer's url_helpers _url with only_path is the path" do
    article = Article.first
    assert_equal "/articles/#{article.id}", ArticleMailer.new.link_for(article)
  end

  test "a controller's bare and chained _url with only_path are the path" do
    article = Article.first
    get "/links/#{article.id}"
    assert_equal "/articles/#{article.id} /articles/#{article.id}", response.body
  end
end
"##;

#[test]
fn url_helper_only_path_is_the_path_in_every_class() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"links/:id\", to: \"links#show\"\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            &format!("{MODEL_METHODS}\n  validates :title, presence: true\n"),
        )
        .write("app/services/article_links.rb", SERVICE)
        .write("app/mailers/article_mailer.rb", MAILER)
        .write("app/controllers/links_controller.rb", CONTROLLER)
        .write("test/controllers/links_controller_test.rb", TEST)
        .run_test("test/controllers/links_controller_test.rb")
        .assert_passes();
}
