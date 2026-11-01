//! A `.+` requirement spans segments, as Rails' regex reads it: lobsters'
//! ORIGINS_IDENTIFIER, `/(.+)(?=\.json|\.rss|$|\/)/`, takes
//! `github.com/user` whole, and its greedy body keeps a trailing `.rss` as
//! identifier text rather than the format. The lazy body (`.+?`) splits
//! a listed extension off; a span followed by `/edit` stops at the last
//! one. Every expectation is what ActionDispatch 8.1's `recognize_path`
//! answers for the same table. The router refused these as lookahead, so
//! lobsters' /origins/... pages and the moderator's origin edit and ban
//! were 404s.

use super::emit_and_run;

const TEST_PATH: &str = "test/controllers/sites_controller_test.rb";

#[test]
fn a_dot_requirement_spans_segments_in_declaration_order() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            concat!(
                "  root \"articles#index\"\n",
                "  constraints id: /(.+)(?=\\.json|\\.rss|$|\\/)/ do\n",
                "    get \"/sites/:id(.:format)\" => \"sites#show\"\n",
                "    get \"/edits/:id/edit\" => \"sites#edit\"\n",
                "    get \"/both/:id(.:format)\" => \"sites#show\"\n",
                "    get \"/both/:id/edit\" => \"sites#edit\"\n",
                "  end\n",
                "  get \"/lazy/:id(.:format)\" => \"sites#show\", constraints: { id: /(.+?)(?=\\.json|\\.rss|$|\\/)/ }\n",
                "  get \"/plain/:id(.:format)\" => \"sites#show\", constraints: { id: /.+/ }\n",
                "  get \"/files/*id/edit\" => \"sites#edit\"\n",
            ),
        )
        .write(
            "app/controllers/sites_controller.rb",
            "class SitesController < ApplicationController\n  def show\n    render plain: \"show:#{params[:id]}|#{params[:format]}\"\n  end\n\n  def edit\n    render plain: \"edit:#{params[:id]}|#{params[:format]}\"\n  end\nend\n",
        )
        .write(
            TEST_PATH,
            r##"require "test_helper"

class SitesControllerTest < ActionDispatch::IntegrationTest
  def body_of(path)
    get path
    "#{response.status} #{response.body}"
  end

  test "a greedy span takes the rest of the path, extension and all" do
    assert_equal "200 show:github.com|", body_of("/sites/github.com")
    assert_equal "200 show:github.com.rss|", body_of("/sites/github.com.rss")
    assert_equal "200 show:github.com/user|", body_of("/sites/github.com/user")
    assert_equal "200 show:github.com/user.json|", body_of("/sites/github.com/user.json")
    assert_equal "200 show:github.com/user|", body_of("/sites/github.com%2Fuser")
  end

  test "a span followed by a literal stops at its last match" do
    assert_equal "200 edit:github.com/user|", body_of("/edits/github.com/user/edit")
    assert_equal "200 edit:a/edit|", body_of("/edits/a/edit/edit")
    assert_equal "200 edit:x|rss", body_of("/edits/x/edit.rss")
    assert_equal "200 edit:github.com/user.rss|", body_of("/edits/github.com/user.rss/edit")
  end

  test "the first declared route wins, the extension staying in its span" do
    assert_equal "200 show:x/edit|", body_of("/both/x/edit")
    assert_equal "200 show:x/edit.rss|", body_of("/both/x/edit.rss")
  end

  test "a lazy span splits off only a listed extension" do
    assert_equal "200 show:github.com/user|rss", body_of("/lazy/github.com/user.rss")
    assert_equal "200 show:a.json|rss", body_of("/lazy/a.json.rss")
    assert_equal "200 show:a.xml|", body_of("/lazy/a.xml")
    assert_equal "200 show:a/b|", body_of("/lazy/a/b")
  end

  test "a plain dot requirement and a middle glob span too" do
    assert_equal "200 show:a/b.json|", body_of("/plain/a/b.json")
    assert_equal "200 edit:a/b|", body_of("/files/a/b/edit")
  end
end
"##,
        )
        .run_test(TEST_PATH)
        .assert_passes();
}
