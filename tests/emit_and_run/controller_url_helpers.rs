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
