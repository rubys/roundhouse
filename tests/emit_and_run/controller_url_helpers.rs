//! A `_url` route helper called in a controller answers an ABSOLUTE
//! URL, as Rails' does: scheme, host, a non-standard port, then the
//! path. `ActionController::UrlFor#url_options` takes the three from
//! the request and lets `default_url_options` override each.
//!
//! The controller lowerer used to fold `articles_url` onto
//! `articles_path`, so `render plain: articles_url` answered
//! `/articles` where Rails answers `http://www.example.com/articles`
//! (the integration test's default host).

use super::emit_and_run;

fn with_urls_controller(controller: &str, test: &str) -> emit_and_run::Run {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            concat!(
                "  root \"articles#index\"\n",
                "  get \"/urls\", to: \"urls#show\"\n",
                "  get \"/urls/away\", to: \"urls#away\"\n",
            ),
        )
        .write("app/controllers/urls_controller.rb", controller)
        .write("test/controllers/urls_controller_test.rb", test)
        .run_test("test/controllers/urls_controller_test.rb")
}

#[test]
fn controller_url_helper_is_absolute_on_the_request_host() {
    with_urls_controller(
        r##"class UrlsController < ApplicationController
  def show
    article = Article.first
    render plain: "#{articles_url} #{article_url(article)} #{articles_path}"
  end

  def away
    redirect_to articles_url
  end
end
"##,
        r#"require "test_helper"

class UrlsControllerTest < ActionDispatch::IntegrationTest
  test "absolute on the request's host" do
    get "/urls"
    id = Article.first.id
    assert_equal "http://www.example.com/articles http://www.example.com/articles/#{id} /articles", response.body
  end

  test "a redirect to a _url names the request's host" do
    get "/urls/away"
    assert_equal "http://www.example.com/articles", response.location
    assert_redirected_to articles_url
    assert_redirected_to "/articles"
    follow_redirect!
    assert_response :success
  end

  test "host! moves the authority" do
    host! "blog.test"
    get "/urls"
    assert_equal "http://blog.test/articles", response.body.split(" ").first
  end

  test "a standard port in the Host names the same redirect as none" do
    host! "blog.test:80"
    get "/urls/away"
    assert_equal "http://blog.test/articles", response.location
    assert_redirected_to "/articles"
  end

  test "an absolute url with a query and no path requests the root" do
    get "http://blog.test?before=6"
    assert_response :success
    assert_equal "/?before=6", request.fullpath
  end

  test "an absolute url's fragment does not reach the router" do
    get "http://blog.test/urls?before=6#section"
    assert_response :success
    assert_equal "/urls?before=6", request.fullpath
  end

  test "an https url is requested over https, and its redirect followed so" do
    get "https://blog.test/urls/away"
    assert_equal "https://blog.test/articles", response.location
    assert_redirected_to "/articles"
    follow_redirect!
    assert_response :success
    get "/urls"
    assert_equal "https://blog.test/articles", response.body.split(" ").first
    get "http://blog.test/urls"
    assert_equal "http://blog.test/articles", response.body.split(" ").first
  end
end
"#,
    )
    .assert_passes();
}

#[test]
fn controller_url_helper_honors_default_url_options() {
    with_urls_controller(
        r##"class UrlsController < ApplicationController
  def show
    render plain: articles_url
  end

  def away
    head :ok
  end

  def default_url_options
    { host: "blog.test", port: 8443, protocol: "https" }
  end
end
"##,
        r#"require "test_helper"

class UrlsControllerTest < ActionDispatch::IntegrationTest
  test "host, port and protocol from default_url_options" do
    get "/urls"
    assert_equal "https://blog.test:8443/articles", response.body
  end
end
"#,
    )
    .assert_passes();
}

#[test]
fn controller_url_helper_drops_the_scheme_standard_port() {
    with_urls_controller(
        r##"class UrlsController < ApplicationController
  def show
    render plain: articles_url
  end

  def away
    head :ok
  end

  def default_url_options
    { protocol: "https://", port: 443 }
  end
end
"##,
        r#"require "test_helper"

class UrlsControllerTest < ActionDispatch::IntegrationTest
  test "443 is https' own port" do
    get "/urls"
    assert_equal "https://www.example.com/articles", response.body
  end
end
"#,
    )
    .assert_passes();
}

/// jbuilder's `json.url article_url(article, format: :json)` — the
/// scaffold's self-link — renders the absolute URL too. The jbuilder
/// lowerer used to fold it onto the path, so every article in
/// `/articles.json` carried `"url":"/articles/1.json"`.
#[test]
fn jbuilder_url_helper_is_absolute_on_the_request_host() {
    emit_and_run::real_blog()
        .write(
            "test/controllers/article_json_urls_controller_test.rb",
            r#"require "test_helper"

class ArticleJsonUrlsControllerTest < ActionDispatch::IntegrationTest
  test "the show self-link names the request's host" do
    article = Article.first
    get "/articles/#{article.id}.json"
    assert_response :success
    assert_includes response.body, %("url":"http://www.example.com/articles/#{article.id}.json")
  end

  test "the index self-links follow host!" do
    host! "blog.test"
    get "/articles.json"
    assert_includes response.body, %("url":"http://blog.test/articles/#{Article.first.id}.json")
    assert_not_includes response.body, %("url":"/articles/)
  end

  # Outside the harness the view reads the request (no session origin);
  # cleared here so that path is what renders.
  test "a standard port in the Host drops out of the self-link" do
    host! "blog.test:80"
    ActionView::ViewHelpers.url_origin = ""
    get "/articles.json"
    assert_includes response.body, %("url":"http://blog.test/articles/#{Article.first.id}.json")
  end

  # campfire's bot API tests compare the two sides:
  # `assert_equal room_message_url(@room, m), json["url"]`. A test
  # body's `_url` is absolute on the session's host, as the view's is.
  test "a test body's _url equals the view's self-link" do
    article = Article.first
    get article_url(article, format: :json)
    assert_equal article_url(article, format: :json), JSON.parse(response.body)["url"]
  end

  test "a test body's _url is on the session host before any request" do
    assert_equal "http://www.example.com/articles", articles_url
    host! "blog.test"
    https!
    assert_equal "https://blog.test/articles", articles_url
  end
end
"#,
        )
        .run_test("test/controllers/article_json_urls_controller_test.rb")
        .assert_passes();
}

/// A key `default_url_options` names with a nil value removes the
/// request's: `port: nil` drops the Host header's `:8080`, as Rails'
/// `url_options` merge does.
#[test]
fn controller_url_helper_nil_option_removes_the_request_port() {
    with_urls_controller(
        r##"class UrlsController < ApplicationController
  def show
    render plain: articles_url
  end

  def away
    head :ok
  end

  def default_url_options
    { port: nil }
  end
end
"##,
        r#"require "test_helper"

class UrlsControllerTest < ActionDispatch::IntegrationTest
  test "port: nil drops the request's port" do
    host! "blog.test:8080"
    get "/urls"
    assert_equal "http://blog.test/articles", response.body
  end
end
"#,
    )
    .assert_passes();
}
