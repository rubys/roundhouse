//! #1680: `stylesheet_link_tag`'s source. Rails 8.1.4 (`asset_path`)
//! leaves a URL and an absolute path off the pipeline and adds `.css` only
//! when the name doesn't already end in it; the runtime put every source
//! under `/assets/` and added `.css` again.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/views/articles/index.html.erb",
        "<%= turbo_stream_from \"articles\" %>\n",
        concat!(
            "<%= turbo_stream_from \"articles\" %>\n",
            "<%= stylesheet_link_tag \"https://cdn.example.org/theme.css\", id: \"url\" %>\n",
            "<%= stylesheet_link_tag \"/static/theme.css\", id: \"absolute\" %>\n",
            "<%= stylesheet_link_tag \"/static/theme\", id: \"absolute-bare\" %>\n",
            "<%= stylesheet_link_tag \"theme.css\", id: \"named\" %>\n",
        ),
    )
}

const TEST: &str = r#"require "test_helper"

class StylesheetSourceControllerTest < ActionDispatch::IntegrationTest
  test "each source links as Rails does" do
    get articles_url
    assert_response :success
    assert_select "link#url[rel=stylesheet][href='https://cdn.example.org/theme.css']"
    assert_select "link#absolute[href='/static/theme.css']"
    assert_select "link#absolute-bare[href='/static/theme.css']"
    assert_select "link#named[href='/assets/theme.css']"
  end
end
"#;

#[test]
fn a_stylesheet_source_links_as_rails_does() {
    app()
        .write("test/controllers/stylesheet_source_controller_test.rb", TEST)
        .run_test("test/controllers/stylesheet_source_controller_test.rb")
        .assert_passes();
}
