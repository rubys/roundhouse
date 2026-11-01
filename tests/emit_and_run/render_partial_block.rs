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

/// A partial that yields and also reads its record local (`articles/_card`
/// reads `article`) keeps the record's model type, and rendered without a
/// block its `yield` renders nothing (Rails: no block, nil). Only a
/// partial that never reads its record carries the block's body in that
/// slot.
#[test]
fn a_partial_reading_its_record_keeps_it_and_yields_nothing_without_a_block() {
    let overlay = || {
        emit_and_run::real_blog()
            .write("app/views/articles/_card.html.erb", "<b><%= article.title %></b><%= yield %>\n")
            .edit(
                "app/views/articles/show.html.erb",
                "<% content_for :title, \"Showing article\" %>\n",
                "<% content_for :title, \"Showing article\" %>\n<p id=\"card\"><%= render \"articles/card\", article: @article %></p>\n",
            )
    };
    let (out, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let sig = std::fs::read_to_string(out.join("sig/app/views/articles/_card.rbs")).unwrap();
    assert!(sig.contains("Article article") && !sig.contains("String article"), "{sig}");
    let test = "require \"test_helper\"\n\nclass CardControllerTest < ActionDispatch::IntegrationTest\n  test \"card\" do\n    get article_url(articles(:one))\n    assert_match(%r{<p id=\"card\"><b>Getting Started with Rails</b>\\n?</p>}, response.body)\n  end\nend\n";
    overlay()
        .write("test/controllers/card_controller_test.rb", test)
        .run_test("test/controllers/card_controller_test.rb")
        .assert_passes();
}

/// A collection render of a partial that carries the yield: each element
/// would land in its String slot and `yield` print it, where Rails'
/// `yield` with no block renders nothing (review, partial.rs:1295).
#[test]
fn a_collection_render_of_a_yielding_partial_yields_nothing() {
    let test = "require \"test_helper\"\n\nclass WrapControllerTest < ActionDispatch::IntegrationTest\n  test \"wrap\" do\n    get articles_url\n    assert_match(%r{<div id=\"wraps\">(<p class=\"wrap\"></p>\\n?)+</div>}, response.body)\n  end\nend\n";
    emit_and_run::real_blog()
        .write("app/views/articles/_wrap.html.erb", "<p class=\"wrap\"><%= yield %></p>\n")
        .edit("app/views/articles/index.html.erb", "<% content_for :title, \"Articles\" %>\n", "<% content_for :title, \"Articles\" %>\n<div id=\"wraps\"><%= render partial: \"articles/wrap\", collection: @articles, as: :article %></div>\n")
        .write("test/controllers/wrap_controller_test.rb", test)
        .run_test("test/controllers/wrap_controller_test.rb")
        .assert_passes();
}

/// A partial collected `as: :card` that reads `card` and yields: its
/// record local is `card`, so a block render to it is refused, as for
/// any partial reading its record local (review, mod.rs:3826).
#[test]
fn a_block_render_to_a_partial_reading_its_collection_local_is_refused() {
    let errors = emit_and_run::real_blog()
        .write("app/views/articles/_card.html.erb", "<b><%= card.title %></b><%= yield %>\n")
        .edit(
            "app/views/articles/index.html.erb",
            "<% content_for :title, \"Articles\" %>\n",
            "<% content_for :title, \"Articles\" %>\n<%= render partial: \"articles/card\", collection: @articles, as: :card %>\n<%= render \"articles/card\" do %>x<% end %>\n",
        )
        .check_errors();
    assert!(errors.iter().any(|e| e.contains("render with a block not supported") && e.contains("record local")), "{errors:?}");
}

/// Two formats of one partial: the HTML one carries the yield, the text
/// one reads its record. They share one key, so neither carries it: the
/// text variant keeps its `Article` (review, mod.rs:3827).
#[test]
fn a_partial_carries_the_yield_only_when_every_format_does() {
    let (out, errors) = emit_and_run::real_blog()
        .write("app/views/articles/_card.html.erb", "<p><%= yield %></p>\n")
        .write("app/views/articles/_card.text.erb", "<%= article.title %>\n")
        .edit("app/views/articles/show.html.erb", "<% content_for :title, \"Showing article\" %>\n", "<% content_for :title, \"Showing article\" %>\n<%= render \"articles/card\", article: @article %>\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let sigs: String = std::fs::read_dir(out.join("sig/app/views/articles")).unwrap()
        .filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().contains("card"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap()).collect();
    assert!(!sigs.contains("String article"), "{sigs}");
}
