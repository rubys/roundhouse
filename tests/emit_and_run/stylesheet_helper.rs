//! #1593: `stylesheet_link_tag` inside a helper method. A helper module's
//! methods never reach the view walker, so the call stayed bare, with no
//! module defining it. It now lowers to the same
//! `ViewHelpers.stylesheet_link_tag(name, opts)` the view spelling does.

use super::emit_and_run;

const THEME_HELPER: &str = "module ThemeHelper\n  def theme_stylesheet(name)\n    stylesheet_link_tag(name, data: { turbo_track: \"reload\" })\n  end\nend\n";

/// real-blog's layout, linking the same stylesheet through the helper and
/// directly.
fn app(helper: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/helpers/theme_helper.rb", helper)
        .edit(
            "app/views/layouts/application.html.erb",
            "  </head>\n",
            "    <%= theme_stylesheet(\"theme\") %>\n    <%= stylesheet_link_tag \"theme\", data: { turbo_track: \"reload\" } %>\n  </head>\n",
        )
}

const TEST: &str = r#"require "test_helper"

class ThemeStylesheetControllerTest < ActionDispatch::IntegrationTest
  test "the helper's link is the view spelling's" do
    get articles_url
    assert_response :success
    assert_select "link[rel=stylesheet][href='/assets/theme.css'][data-turbo-track=reload]", 2
  end
end
"#;

#[test]
fn a_helper_stylesheet_link_tag_renders_as_the_view_spelling() {
    app(THEME_HELPER)
        .write("test/controllers/theme_stylesheet_controller_test.rb", TEST)
        .run_test("test/controllers/theme_stylesheet_controller_test.rb")
        .assert_passes();
}

/// In a view a Symbol is a stylesheet group (`:app`) expanded from the
/// view's stylesheet list, which a helper doesn't have; a splat may be
/// anything. Both are refused in a helper.
#[test]
fn a_helper_stylesheet_link_tag_rails_resolves_otherwise_is_refused() {
    for body in ["stylesheet_link_tag(:app)", "stylesheet_link_tag(*[name])"] {
        let helper = format!("module ThemeHelper\n  def theme_stylesheet(name)\n    {body}\n  end\nend\n");
        let errors = app(&helper).check_errors();
        assert!(errors.iter().any(|e| e.contains("stylesheet_link_tag not supported")), "{body}: {errors:?}");
    }
}

/// An app's own `stylesheet_link_tag` (here an `ApplicationHelper` method)
/// is what Rails calls: neither rewritten nor refused.
#[test]
fn an_app_defined_stylesheet_link_tag_is_left_to_the_app() {
    let (emitted, errors) = app("module ThemeHelper\n  def theme_stylesheet(name)\n    stylesheet_link_tag(:app)\n  end\nend\n")
        .write("app/helpers/application_helper.rb", "module ApplicationHelper\n  def stylesheet_link_tag(name)\n    name.to_s\n  end\nend\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("stylesheet_link_tag")), "{errors:?}");
    let helper = std::fs::read_to_string(emitted.join("app/models/theme_helper.rb")).unwrap();
    assert!(!helper.contains("ViewHelpers.stylesheet_link_tag"), "{helper}");
}

/// The same layout in the spinel binary, where #1593 was found.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_helper_stylesheet_link_tag_renders_on_spinel() {
    app(THEME_HELPER)
        .run_spinel(concat!(
            "Db.configure(\":memory:\")\n",
            "Schema.statements.each { |sql| Db.exec(sql) }\n",
            "ActiveRecord.adapter = SqliteAdapter\n",
            "controller = ArticlesController.new\n",
            "controller.request_method = \"GET\"\n",
            "controller.process_action(:index)\n",
            "body = controller.body\n",
            "link = '<link rel=\"stylesheet\" href=\"/assets/theme.css\" data-turbo-track=\"reload\">'\n",
            "raise body unless body.scan(link).length == 2\n",
            "puts \"stylesheet helper passed\"\n",
        ))
        .assert_passes();
}

/// A class's own `stylesheet_link_tag` (here a model method, not exposed
/// as a `helper_method`) is what its bare call reaches: neither
/// rewritten nor refused (review, tag_builder.rs:157).
#[test]
fn a_class_owned_stylesheet_link_tag_is_left_to_the_class() {
    let (emitted, errors) = app(THEME_HELPER)
        .write(
            "app/models/badge.rb",
            "class Badge\n  def stylesheet_link_tag(name)\n    \"own:\" + name\n  end\n\n  def styled\n    stylesheet_link_tag(\"theme\")\n  end\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let badge = std::fs::read_to_string(emitted.join("app/models/badge.rb")).unwrap();
    assert!(badge.contains("stylesheet_link_tag(\"theme\")") && !badge.contains("ViewHelpers.stylesheet_link_tag"), "{badge}");
}

/// A block (Rails' helper takes none) and a parameter default are refused
/// like the body's other unlowered shapes (review, diagnostics.rs:131
/// and :167).
#[test]
fn a_helper_stylesheet_link_tag_with_a_block_or_in_a_default_is_refused() {
    for helper in [
        "module ThemeHelper\n  def theme_stylesheet(name)\n    stylesheet_link_tag(name) { name }\n  end\nend\n",
        "module ThemeHelper\n  def theme_stylesheet(name = stylesheet_link_tag(:app))\n    name\n  end\nend\n",
    ] {
        let errors = app(helper).check_errors();
        assert!(errors.iter().any(|e| e.contains("stylesheet_link_tag not supported")), "{helper}: {errors:?}");
    }
}
