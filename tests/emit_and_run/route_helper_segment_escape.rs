//! A route helper writes a path param through Rails'
//! `Journey::Router::Utils.escape_segment` (actionpack 8.1): `/`, `?`,
//! `#`, `%`, a space and non-ASCII bytes are percent-escaped, the
//! sub-delims, `:` and `@` are not. The helpers interpolated the value as
//! it was, so lobsters' `origin_path("github.com/user")` linked to
//! `/origins/github.com/user`, and a `?` in a value ended the path.

use super::emit_and_run;

const TEST_PATH: &str = "test/controllers/sites_controller_test.rb";

#[test]
fn a_path_param_is_escaped_as_a_segment() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/sites/:name\" => \"sites#show\", as: :site\n",
        )
        .write(
            "app/controllers/sites_controller.rb",
            "class SitesController < ApplicationController\n  def show\n    render plain: params[:name]\n  end\nend\n",
        )
        .write(
            TEST_PATH,
            r#"require "test_helper"

class SitesControllerTest < ActionDispatch::IntegrationTest
  test "each value is written as Rails writes it, and reads back" do
    assert_equal "/sites/a%2Fb", site_path("a/b")
    assert_equal "/sites/a%20b", site_path("a b")
    assert_equal "/sites/a%3Fb%23c", site_path("a?b#c")
    assert_equal "/sites/x%25y", site_path("x%y")
    assert_equal "/sites/jos%C3%A9", site_path("josé")
    assert_equal "/sites/a+b:c@d!$&'()*,;=~._-", site_path("a+b:c@d!$&'()*,;=~._-")
    assert_equal "/sites/plain", site_path("plain")
    get site_path("a?b#c")
    assert_equal "a?b#c", response.body
  end
end
"#,
        )
        .run_test(TEST_PATH)
        .assert_passes();
}
