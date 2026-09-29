//! Constructs that `check` accepts must run once emitted.
//!
//! See `tests/support/emit_and_run.rs` for the harness and why it
//! exists. The ignored tests below are known places where the two
//! disagree: `check` is clean and the emitted program fails. Each is a
//! complete statement of the fix: make it pass and drop the `#[ignore]`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// The harness itself: the unedited blog emits and its controller
/// suite, which renders every page, passes.
#[test]
fn the_unedited_blog_runs() {
    emit_and_run::real_blog()
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// #139 typed `Model.human_attribute_name` as a String, which took the
/// call from an error to clean, but no runtime defines it, so every
/// page rendering the form raises `undefined method
/// 'human_attribute_name' for class Article`. It belongs once, in
/// `runtime/ruby/active_record/base.rb`, where every target gets it.
#[test]
#[ignore = "check is clean but the emitted view raises NoMethodError: no runtime defines human_attribute_name (#147)"]
fn human_attribute_name_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/views/articles/_form.html.erb",
            "<%= form.label :title %>",
            "<%= form.label :title %><%= Article.human_attribute_name(:title) %>",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// #140 bound `form_with builder: X`'s block param to `X`, which took a
/// custom builder's own helpers from errors to clean. But the emitted
/// tree cannot load `X` (no runtime `ActionView::Helpers::FormBuilder`
/// to subclass), and the view calls `form.marker_field` on a `form`
/// that no longer exists, because lowering expands the stock builder
/// inline. Passing needs a builder the emitted view can call; until
/// then, the honest state is an error in `check`.
#[test]
#[ignore = "check is clean but the emitted tree fails to load: no runtime FormBuilder, and the inlined form has no builder object (#148)"]
fn a_custom_form_builder_runs() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/custom_form_builder.rb",
            "class CustomFormBuilder < ActionView::Helpers::FormBuilder\n  \
               def marker_field(name)\n    \
                 @template.content_tag(:span, name.to_s, class: \"builder-marker\")\n  \
               end\n\
             end\n",
        )
        .edit(
            "app/views/articles/_form.html.erb",
            "form_with(model: article, class: \"contents\")",
            "form_with(model: article, class: \"contents\", builder: CustomFormBuilder)",
        )
        .edit(
            "app/views/articles/_form.html.erb",
            "<%= form.label :title %>",
            "<%= form.label :title %><%= form.marker_field :title %>",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// The trailing-keyword-hash `enum` mapping (`enum :kind, kind:
/// 'kind'`) — the dominant Rails 7 spelling, previously ledgered as
/// "enum :x mapping must be an array or hash literal" because
/// `enum_label_values` only recognized a braced `HashNode`. Overlays a
/// single-label string-backed enum onto real-blog's Article, renders
/// the predicate from the show view (a bare `run_ruby` probe would
/// never call it, and treeshaking would drop the synthesized method as
/// dead code — see the emitted tree's own treeshake log), and proves
/// it evaluates true against a seeded article, not just that `check`
/// accepts the declaration.
#[test]
fn enum_keyword_hash_mapping_predicate_runs() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "t.string \"title\"\n    t.text \"body\"\n    t.datetime \"created_at\", null: false",
            "t.string \"title\"\n    t.text \"body\"\n    t.string \"kind\", default: \"kind\", null: false\n    t.datetime \"created_at\", null: false",
        )
        .edit(
            "app/models/article.rb",
            "has_many :comments, dependent: :destroy\n",
            "has_many :comments, dependent: :destroy\n\n  enum :kind, kind: 'kind'\n",
        )
        .edit(
            "app/views/articles/show.html.erb",
            "<h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>",
            "<h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>\n  <p id=\"kind-predicate\"><%= @article.kind? %></p>",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}
