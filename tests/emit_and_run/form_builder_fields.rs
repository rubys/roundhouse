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

const COLOR_TEST: &str = r#"require "test_helper"

class ColorFieldsControllerTest < ActionDispatch::IntegrationTest
  test "a color field holds Rails' validated color, or the given value" do
    get edit_article_url(articles(:one))
    assert_response :success
    assert_select "input.plain#article_title[type=color][value='#000000']"
    assert_select "input.given#article_title[type=color][value='#FF0000']"
    assert_select "input.nil_given#article_title[type=color][value='#000000']"
    assert_select "input.computed#article_title[type=color][value='#000000']"
  end

  test "a record color is lowercased wherever it sits in the value" do
    get edit_article_url(articles(:two))
    assert_response :success
    assert_select "input.plain#article_title[type=color][value='swatch #abcdef']"
    assert_select "input.computed#article_title[type=color][value='#00FF00']"
  end

  test "a new record's color field is black" do
    get new_article_url
    assert_response :success
    assert_select "input.plain#article_title[type=color][value='#000000']"
  end
end
"#;

/// `Tags::ColorField` (actionview 8.1.4) always writes a value. The
/// record's is kept, lowercased, when it holds `#` and six hex digits
/// anywhere, and is `#000000` otherwise; an explicit `value:` is used as
/// given, and one that is nil at render falls back to the record.
#[test]
fn a_color_field_renders_rails_validated_color() {
    app(concat!(
        "    <%= form.color_field :title, class: \"plain\" %>\n",
        "    <%= form.color_field :title, class: \"given\", value: \"#FF0000\" %>\n",
        "    <%= form.color_field :title, class: \"nil_given\", value: nil %>\n",
        "    <%= form.color_field :title, class: \"computed\", value: (article.title == \"Swatch #ABCDEF\" ? \"#00FF00\" : nil) %>\n",
    ))
    .edit("test/fixtures/articles.yml", "  title: Understanding MVC Architecture\n", "  title: \"Swatch #ABCDEF\"\n")
    .write("test/controllers/color_fields_controller_test.rb", COLOR_TEST)
    .run_test("test/controllers/color_fields_controller_test.rb")
    .assert_passes();
}

const DATETIME_LOCAL_TEST: &str = r#"require "test_helper"

class DatetimeLocalFieldsControllerTest < ActionDispatch::IntegrationTest
  test "a datetime column renders as Rails' datetime-local value" do
    get edit_article_url(articles(:one))
    assert_response :success
    assert_select "input.plain#article_created_at[type=datetime-local][value='2026-11-01T09:30:15']"
    assert_select "input.given#article_created_at[type=datetime-local][value='2026-01-01T00:00']"
    assert_select "input.none#article_created_at[type=datetime-local]"
    assert_select "input.none#article_created_at[value]", count: 0
    assert_select "input.text#article_title[type=datetime-local][value='Getting Started with Rails']"
  end

  test "a new record's datetime-local field carries no value" do
    get new_article_url
    assert_response :success
    assert_select "input.plain#article_created_at[type=datetime-local]"
    assert_select "input.plain#article_created_at[value]", count: 0
  end
end
"#;

/// `Tags::DatetimeLocalField` (actionview 8.1.4) formats a time as
/// `%Y-%m-%dT%T`: the zone's wall time, its fraction dropped. A String
/// (an explicit `value:`, or a string column's value) is written as
/// given, and `value: nil` writes no value.
#[test]
fn a_datetime_local_field_renders_rails_value() {
    app(concat!(
        "    <%= form.datetime_local_field :created_at, class: \"plain\" %>\n",
        "    <%= form.datetime_local_field :created_at, class: \"given\", value: \"2026-01-01T00:00\" %>\n",
        "    <%= form.datetime_local_field :created_at, class: \"none\", value: nil %>\n",
        "    <%= form.datetime_local_field :title, class: \"text\" %>\n",
    ))
    .edit(
        "test/fixtures/articles.yml",
        "  title: Getting Started with Rails\n",
        "  title: Getting Started with Rails\n  created_at: 2026-11-01 09:30:15.5\n",
    )
    .write("test/controllers/datetime_local_fields_controller_test.rb", DATETIME_LOCAL_TEST)
    .run_test("test/controllers/datetime_local_fields_controller_test.rb")
    .assert_passes();
}

/// The values the lowering can't format as Rails does at render: the
/// options Rails reparses, a computed `value:`, a column that isn't a
/// time, date or string, and a datetime shown in a non-UTC zone.
#[test]
fn a_datetime_local_field_rails_reformats_is_refused() {
    for field in [
        "<%= form.datetime_local_field :created_at, min: \"2026-11-01T00:00:00Z\" %>",
        "<%= form.datetime_local_field :created_at, max: Time.current %>",
        "<%= form.datetime_local_field :created_at, include_seconds: false %>",
        "<%= form.datetime_local_field :created_at, value: Time.current %>",
        "<%= form.datetime_local_field :id %>",
    ] {
        let errors = app(&format!("    {field}\n")).check_errors();
        assert!(
            errors.iter().any(|e| e.contains("form builder option not supported") || e.contains("form builder field not supported")),
            "{field}: {errors:?}"
        );
    }
    let errors = app("    <%= form.datetime_local_field :created_at %>\n")
        .edit(
            "config/application.rb",
            "    # config.time_zone = \"Central Time (US & Canada)\"\n",
            "    config.time_zone = \"Central Time (US & Canada)\"\n",
        )
        .check_errors();
    assert!(errors.iter().any(|e| e.contains("config.time_zone")), "{errors:?}");
}

/// A datetime column's value is formatted from the text the record reads
/// it as, which starts `YYYY-MM-DD HH:MM:SS` on the ruby family, Crystal,
/// Go and Python. TypeScript reads a `Date` (`Sun Nov 01 2026 …` in the
/// host's zone), Rust's read doesn't build, and the rest are unmeasured,
/// so they refuse the call. A `value:` the app gives is written as is.
#[test]
fn a_datetime_local_field_on_a_time_column_is_refused_where_unmeasured() {
    use roundhouse::project::BuildTarget;
    let field = "    <%= form.datetime_local_field :created_at %>\n";
    for target in [BuildTarget::Typescript, BuildTarget::Rust, BuildTarget::Kotlin] {
        let (_, errors) = app(field).emit(target);
        assert!(
            errors.iter().any(|e| e.contains("form builder field not supported") && e.contains("datetime_local_field")),
            "{target:?}: {errors:?}"
        );
    }
    // A String field name is the same column.
    let (_, errors) = app("    <%= form.datetime_local_field \"created_at\" %>\n").emit(BuildTarget::Typescript);
    assert!(errors.iter().any(|e| e.contains("form builder field not supported")), "{errors:?}");
    for target in [BuildTarget::Crystal, BuildTarget::Go, BuildTarget::Python] {
        let (_, errors) = app(field).emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:?}");
    }
    let (_, errors) = app("    <%= form.datetime_local_field :created_at, value: \"2026-01-01T00:00\" %>\n")
        .emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
}

const DATE_COLUMN_TEST: &str = r#"require "test_helper"

class DateColumnFieldsControllerTest < ActionDispatch::IntegrationTest
  test "a date column renders at midnight" do
    get edit_article_url(articles(:one))
    assert_response :success
    assert_select "input.date#article_published_on[type=datetime-local][value='2026-11-01T00:00:00']"
  end

  test "a new record's date field carries no value" do
    get new_article_url
    assert_response :success
    assert_select "input.date#article_published_on[type=datetime-local]"
    assert_select "input.date#article_published_on[value]", count: 0
  end
end
"#;

/// `Tags::DatetimeLocalField` formats a Date as midnight
/// (`2026-11-01T00:00:00`, actionview 8.1.4). A Date-only column is
/// supported on the ruby family; the other targets refuse the app's
/// Date column itself (`project::reject_unsupported_dates`).
#[test]
fn a_datetime_local_field_on_a_date_column_renders_midnight() {
    app("    <%= form.datetime_local_field :published_on, class: \"date\" %>\n")
        .edit("db/schema.rb", "    t.string \"title\"\n", "    t.string \"title\"\n    t.date \"published_on\"\n")
        .edit(
            "test/fixtures/articles.yml",
            "  title: Getting Started with Rails\n",
            "  title: Getting Started with Rails\n  published_on: 2026-11-01\n",
        )
        .write("test/controllers/date_column_fields_controller_test.rb", DATE_COLUMN_TEST)
        .run_test("test/controllers/date_column_fields_controller_test.rb")
        .assert_passes();
}

/// Roda converts views from their ERB and translates only `label`,
/// `text_field`, `textarea` and `submit`; a `color_field` or
/// `datetime_local_field` stays in the page as `form.…`, with no form
/// builder behind it. Refused on Roda, naming the target.
#[test]
fn a_color_or_datetime_local_field_is_refused_on_roda() {
    for field in [
        "<%= form.color_field :title %>",
        "<%= form.datetime_local_field :created_at %>",
    ] {
        let (_, errors) = app(&format!("    {field}\n")).emit(roundhouse::project::BuildTarget::Roda);
        assert!(errors.iter().any(|e| e.contains("form builder field not supported") && e.contains("roda")), "{field}: {errors:?}");
    }
}

/// Only a form builder's call is refused: a model method that happens to
/// be named `color_field`, read through a block local, is an ordinary
/// call Roda keeps.
#[test]
fn a_non_builder_color_field_call_is_not_refused_on_roda() {
    let (_, errors) = emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  def color_field\n    \"#336699\"\n  end\n\n  def datetime_local_field\n    \"2026-11-01T09:30\"\n  end\n\n")
        .edit(
            "app/views/articles/index.html.erb",
            "<% content_for :title, \"Articles\" %>\n",
            "<% content_for :title, \"Articles\" %>\n<% @articles.each do |article| %><p><%= article.color_field %> <%= article.datetime_local_field %></p><% end %>\n",
        )
        .emit(roundhouse::project::BuildTarget::Roda);
    assert!(!errors.iter().any(|e| e.contains("form builder field")), "{errors:?}");
}

/// A partial handed the form's builder (`render "fields", form: form`)
/// calls it through that local; its field is refused on Roda too.
#[test]
fn a_color_field_in_a_partial_handed_the_builder_is_refused_on_roda() {
    let (_, errors) = emit_and_run::real_blog()
        .write("app/views/articles/_fields.html.erb", "<%= form.color_field :title %>\n")
        .edit("app/views/articles/_form.html.erb", "<%= form.submit", "<%= render \"fields\", form: form %>\n  <%= form.submit")
        .emit(roundhouse::project::BuildTarget::Roda);
    assert!(errors.iter().any(|e| e.contains("form builder field not supported") && e.contains("roda")), "{errors:?}");
}

/// A builder derived from the form's (`form.fields_for(:child) do |child|`)
/// is a builder too: its field is refused on Roda.
#[test]
fn a_color_field_on_a_nested_fields_for_builder_is_refused_on_roda() {
    let (_, errors) = emit_and_run::real_blog()
        .edit(
            "app/views/articles/_form.html.erb",
            "<%= form.submit",
            "<%= form.fields_for :extra do |child| %><%= child.color_field :title %><% end %>\n  <%= form.submit",
        )
        .emit(roundhouse::project::BuildTarget::Roda);
    assert!(errors.iter().any(|e| e.contains("form builder field not supported") && e.contains("roda")), "{errors:?}");
}

/// A builder handed through two partials (`_form` → `_outer` → `_inner`)
/// is still a builder in the second: its field is refused on Roda.
#[test]
fn a_color_field_two_partials_down_is_refused_on_roda() {
    let (_, errors) = emit_and_run::real_blog()
        .write("app/views/articles/_outer.html.erb", "<%= render \"inner\", f: form %>\n")
        .write("app/views/articles/_inner.html.erb", "<%= f.color_field :title %>\n")
        .edit("app/views/articles/_form.html.erb", "<%= form.submit", "<%= render \"outer\", form: form %>\n  <%= form.submit")
        .emit(roundhouse::project::BuildTarget::Roda);
    assert!(errors.iter().any(|e| e.contains("form builder field not supported") && e.contains("roda")), "{errors:?}");
}
