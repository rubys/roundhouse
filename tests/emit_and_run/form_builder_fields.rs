//! The `FormBuilder` fields Rails derives from `Tags::TextField` by
//! type alone — `number_field`, `range_field`, `search_field`,
//! `telephone_field` and its alias `phone_field` — on real-blog's
//! article form, and the builder calls the lowering does not expand,
//! which `check` refuses rather than emitting a page that fails at
//! render (a literal `form.date_select` reached the view module's own
//! `form` method: `wrong number of arguments`).

use super::emit_and_run;

fn app(fields: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/views/articles/_form.html.erb",
        "    <%= form.label :body %>\n",
        &format!("{fields}    <%= form.label :body %>\n"),
    )
}

const TEST: &str = r#"require "test_helper"

class FormFieldsControllerTest < ActionDispatch::IntegrationTest
  test "each field renders its input type, named and valued as Rails does" do
    get edit_article_url(articles(:one))
    assert_response :success
    assert_select "input#article_title[type=number][value='Getting Started with Rails']"
    assert_select "input#article_title[type=range][min='1'][max='9']"
    assert_select "input#article_title[type=search][placeholder=Find]"
    assert_select "input#article_title[type=tel]", 2
  end

  test "a new record's fields carry no value" do
    get new_article_url
    assert_response :success
    assert_select "input#article_title[type=number]"
    assert_select "input#article_title[type=number][value]", count: 0
  end
end
"#;

#[test]
fn number_range_search_and_telephone_fields_render() {
    app(concat!(
        "    <%= form.number_field :title %>\n",
        "    <%= form.range_field :title, min: 1, max: 9 %>\n",
        "    <%= form.search_field :title, placeholder: \"Find\" %>\n",
        "    <%= form.telephone_field :title %>\n",
        "    <%= form.phone_field :title %>\n",
    ))
    .write("test/controllers/form_fields_controller_test.rb", TEST)
    .run_test("test/controllers/form_fields_controller_test.rb")
    .assert_passes();
}

#[test]
fn a_builder_method_with_no_lowering_is_refused() {
    for field in [
        "<%= form.date_select :created_at %>",
        "<%= form.date_field :created_at %>",
        "<%= form.collection_select :title, Article.all, :id, :title %>",
        "<%= form.tel_field :title %>",
    ] {
        let errors = app(&format!("    {field}\n")).check_errors();
        assert!(
            errors.iter().any(|e| e.contains("form builder method not supported")),
            "{field}: {errors:?}"
        );
    }
}

#[test]
fn an_option_rails_rewrites_at_render_is_refused() {
    for field in [
        "<%= form.number_field :title, in: 1..9 %>",
        "<%= form.range_field :title, within: 1..9 %>",
        "<%= form.search_field :title, autosave: true %>",
    ] {
        let errors = app(&format!("    {field}\n")).check_errors();
        assert!(
            errors.iter().any(|e| e.contains("form builder option not supported")),
            "{field}: {errors:?}"
        );
    }
}
