//! `csrf_meta_tag` is Rails' alias of `csrf_meta_tags`
//! (`ActionView::Helpers::CsrfHelper`). The view lowering knew only the
//! plural, so a layout calling the alias kept it as a bare call, which
//! the emitted view has no method for: every page under that layout
//! raised NameError.

use super::emit_and_run;

const TEST: &str = "test/controllers/csrf_meta_tag_alias_controller_test.rb";

/// The blog with its layout rendering the alias beside the plural.
fn both_in_the_layout(test: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/views/layouts/application.html.erb",
            "    <%= csrf_meta_tags %>\n",
            "    <%= csrf_meta_tag %>\n    <%= csrf_meta_tags %>\n",
        )
        .write(TEST, test)
}

#[test]
fn the_alias_renders_the_meta_tags_the_plural_does() {
    both_in_the_layout(
        r#"require "test_helper"

class CsrfMetaTagAliasControllerTest < ActionDispatch::IntegrationTest
  setup do
    @previous = ActionController::Base.allow_forgery_protection
    ActionController::Base.allow_forgery_protection = true
  end

  teardown do
    ActionController::Base.allow_forgery_protection = @previous
  end

  test "both helpers render the param and a token" do
    get articles_url
    assert_response :success
    pairs = response.body.scan(%r{<meta name="csrf-param" content="authenticity_token" />\n<meta name="csrf-token" content="[^"]+" />})
    assert_equal 2, pairs.size
  end
end
"#,
    )
    .run_test(TEST)
    .assert_passes();
}

/// With forgery protection off, as Rails' test environment has it, the
/// alias renders nothing, like the plural.
#[test]
fn the_alias_renders_nothing_while_forgery_protection_is_off() {
    both_in_the_layout(
        r#"require "test_helper"

class CsrfMetaTagAliasControllerTest < ActionDispatch::IntegrationTest
  test "neither helper renders a tag" do
    get articles_url
    assert_response :success
    refute_includes response.body, "csrf-param"
    refute_includes response.body, "csrf-token"
  end
end
"#,
    )
    .run_test(TEST)
    .assert_passes();
}
