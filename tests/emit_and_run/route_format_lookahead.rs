//! A requirement that stops a lazy segment before listed extensions
//! (`/([^\/]+?)(?=\.json|\.rss|$|\/)/`, lobsters' DOMAINS_IDENTIFIER) is
//! served: the segment takes the whole dotted value, a listed extension at
//! its end is the format, and any other extension stays in the value, as
//! Rails' lookahead reads it. The router refused it as lookahead, so
//! lobsters' /domains/... pages and the moderator's domain ban were 404s.

use super::emit_and_run;

const TEST_PATH: &str = "test/controllers/sites_controller_test.rb";

#[test]
fn a_lookahead_requirement_splits_only_its_listed_formats() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            concat!(
                "  root \"articles#index\"\n",
                "  constraints id: /([^\\/]+?)(?=\\.json|\\.rss|$|\\/)/ do\n",
                "    get \"/sites/:id(.:format)\" => \"sites#show\"\n",
                "    get \"/sites/:id/page/:page\" => \"sites#show\"\n",
                "  end\n",
            ),
        )
        .write(
            "app/controllers/sites_controller.rb",
            "class SitesController < ApplicationController\n  def show\n    render plain: \"#{params[:id]}|#{params[:format]}|#{params[:page]}\"\n  end\nend\n",
        )
        .write(
            TEST_PATH,
            r#"require "test_helper"

class SitesControllerTest < ActionDispatch::IntegrationTest
  test "the segment keeps its dots and only a listed extension is the format" do
    get "/sites/example.com"
    assert_equal "example.com||", response.body
    get "/sites/example.com.rss"
    assert_equal "example.com|rss|", response.body
    get "/sites/example.com.xml"
    assert_equal "example.com.xml||", response.body
    get "/sites/example.com/page/2"
    assert_equal "example.com||2", response.body
  end
end
"#,
        )
        .run_test(TEST_PATH)
        .assert_passes();
}
