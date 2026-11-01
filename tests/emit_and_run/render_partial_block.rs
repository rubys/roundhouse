//! #1591: `render "partial" do … end`. Rails renders the partial with the
//! block's markup as its `yield` (actionview 8.1.4, `oracle.out`), and a
//! partial rendered without a block yields nothing. The emit kept a bare
//! `render … do`, which no view module defines (Spinel: `unsupported
//! call`).

use super::emit_and_run;

const BLURB: &str = "<div class=\"blurb\"><%= yield %></div>\n";
const TITLED: &str = "<section><h2><%= title %></h2><%= yield %></section>\n";

fn app(markup: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/views/shared/_blurb.html.erb", BLURB)
        .write("app/views/shared/_titled.html.erb", TITLED)
        .edit("app/views/articles/index.html.erb", "<% content_for :title, \"Articles\" %>\n", &format!("<% content_for :title, \"Articles\" %>\n{markup}"))
}

const MARKUP: &str = concat!(
    "<p id=\"block\"><%= render \"shared/blurb\" do %>Inside <b>bold</b><% end %></p>\n",
    "<p id=\"no-block\"><%= render \"shared/blurb\" %></p>\n",
    "<p id=\"locals\"><%= render \"shared/titled\", title: \"T\" do %>Body<% end %></p>\n",
    "<p id=\"escaped\"><%= render \"shared/blurb\" do %><%= \"<i>\" %><% end %></p>\n",
);

const TEST: &str = r#"require "test_helper"

class RenderPartialBlockControllerTest < ActionDispatch::IntegrationTest
  test "a partial renders a block as its yield" do
    get articles_url
    assert_response :success
    assert_match(%r{<p id="block"><div class="blurb">Inside <b>bold</b></div>}, response.body)
    assert_match(%r{<p id="no-block"><div class="blurb"></div>}, response.body)
    assert_match(%r{<p id="locals"><section><h2>T</h2>Body</section>}, response.body)
    assert_match(%r{<p id="escaped"><div class="blurb">&lt;i&gt;</div>}, response.body)
  end
end
"#;

#[test]
fn a_partial_renders_its_block_as_its_yield() {
    app(MARKUP)
        .write("test/controllers/render_partial_block_controller_test.rb", TEST)
        .run_test("test/controllers/render_partial_block_controller_test.rb")
        .assert_passes();
}

/// A captured string is Rails' answer only for a partial that yields once
/// at its top level, with the body in a record slot nothing else uses.
/// The other shapes are refused, each with its reason, as is a block-form
/// render the lowering doesn't model (`render partial: …, locals: … do`
/// raises in Rails).
#[test]
fn a_block_render_rails_answers_otherwise_is_refused() {
    for (partial, markup, reason) in [
        ("<p><%= yield %>|<%= yield %></p>\n", "<%= render \"shared/blurb\" do %>x<% end %>\n", "yield exactly once"),
        ("<% [1, 2].each do |i| %><%= yield %><% end %>\n", "<%= render \"shared/blurb\" do %>x<% end %>\n", "yield exactly once"),
        ("<%# locals: (title:) -%>\n<h2><%= title %></h2><%= yield %>\n", "<%= render \"shared/blurb\", title: \"T\" do %>x<% end %>\n", "strict locals"),
        ("<p><%= shared %></p><%= yield %>\n", "<%= render \"shared/blurb\" do %>x<% end %>\n", "record local"),
        (BLURB, "<%= render \"shared/blurb\" do |x| %>x<% end %>\n", "block with parameters"),
        (BLURB, "<%= render partial: \"shared/blurb\", locals: { a: 1 } do %>x<% end %>\n", "are modeled"),
    ] {
        let errors = emit_and_run::real_blog()
            .write("app/views/shared/_blurb.html.erb", partial)
            .edit("app/views/articles/index.html.erb", "<% content_for :title, \"Articles\" %>\n", &format!("<% content_for :title, \"Articles\" %>\n{markup}"))
            .check_errors();
        assert!(errors.iter().any(|e| e.contains("render with a block not supported") && e.contains(reason)), "{markup}: {errors:?}");
    }
}

/// Only the ruby family carries the captured body (measured: TypeScript
/// redeclares the capture local, Crystal and Go pass the builder where
/// the partial takes a String, Python a list); elsewhere, and on Roda, the
/// render is refused, naming the target.
#[test]
fn a_block_render_is_refused_off_the_ruby_family() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Typescript, BuildTarget::Crystal, BuildTarget::Go, BuildTarget::Python, BuildTarget::Rust, BuildTarget::Roda] {
        let (_, errors) = app(MARKUP).emit(target);
        assert!(errors.iter().any(|e| e.contains("render with a block not supported")), "{target:?}: {errors:?}");
    }
}

/// The index in the spinel binary, where #1591 was found.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_partial_renders_its_block_as_its_yield_on_spinel() {
    app(MARKUP)
        .run_spinel(concat!(
            "Db.configure(\":memory:\")\n",
            "Schema.statements.each { |sql| Db.exec(sql) }\n",
            "ActiveRecord.adapter = SqliteAdapter\n",
            "request = ActionDispatch::Request.new\n",
            "request.request_method = \"GET\"\n",
            "request.path = \"/articles\"\n",
            "ActionController::Current.request = request\n",
            "controller = ArticlesController.new\n",
            "controller.request_method = \"GET\"\n",
            "controller.params = {}\n",
            "controller.process_action(:index)\n",
            "body = controller.body\n",
            "raise body unless body.include?('<p id=\"block\"><div class=\"blurb\">Inside <b>bold</b></div>')\n",
            "raise body unless body.include?('<p id=\"no-block\"><div class=\"blurb\"></div>')\n",
            "raise body unless body.include?('<p id=\"locals\"><section><h2>T</h2>Body</section>')\n",
            "raise body unless body.include?('<p id=\"escaped\"><div class=\"blurb\">&lt;i&gt;</div>')\n",
            "puts \"render partial block passed\"\n",
        ))
        .assert_passes();
}
