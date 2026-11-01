//! #1593: `time_tag`. Rails 8.1.4's helper is `content_tag("time", text,
//! options.reverse_merge(datetime: t.iso8601), &block)`: the text (or the
//! block) inside, `datetime` first unless the options give one, then the
//! options in their order. A UTC time's iso8601 ends in `Z` and drops the
//! fraction. The builder call it lowers to, `tag.time "…", datetime:
//! t.iso8601`, is tested beside it: on the ruby target its `iso8601` was
//! printed `to_rfc3339` (CRuby's Time has none) once the view lowering
//! rebuilt the call.

use super::emit_and_run;

/// real-blog's show page, with the article's `created_at` fixed.
fn app(markup: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/views/articles/show.html.erb",
            "  <h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>\n",
            &format!("  <h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>\n{markup}"),
        )
        .edit(
            "test/fixtures/articles.yml",
            "  title: Getting Started with Rails\n",
            "  title: Getting Started with Rails\n  created_at: 2026-11-01 09:30:15.5\n",
        )
}

const MARKUP: &str = concat!(
    "  <p id=\"text\"><%= time_tag @article.created_at, \"Posted\" %></p>\n",
    "  <p id=\"block\"><%= time_tag @article.created_at do %>In block<% end %></p>\n",
    "  <p id=\"class\"><%= time_tag @article.created_at, \"Posted\", class: \"when\" %></p>\n",
    "  <p id=\"override\"><%= time_tag @article.created_at, \"Posted\", datetime: \"2026\" %></p>\n",
    "  <p id=\"escaped\"><%= time_tag @article.created_at, \"<b>x</b>\" %></p>\n",
    "  <p id=\"format\"><%= time_tag @article.created_at, \"Posted\", format: :short %></p>\n",
    "  <p id=\"builder\"><%= tag.time \"Posted\", datetime: @article.created_at.iso8601 %></p>\n",
);

const TEST: &str = r#"require "test_helper"

class TimeTagControllerTest < ActionDispatch::IntegrationTest
  test "time_tag renders as Rails does" do
    get article_url(articles(:one))
    assert_response :success
    assert_match(/<p id="text"><time datetime="2026-11-01T09:30:15Z">Posted<\/time><\/p>/, response.body)
    assert_match(/<p id="block"><time datetime="2026-11-01T09:30:15Z">In block<\/time><\/p>/, response.body)
    assert_match(/<p id="class"><time datetime="2026-11-01T09:30:15Z" class="when">Posted<\/time><\/p>/, response.body)
    assert_match(/<p id="override"><time datetime="2026">Posted<\/time><\/p>/, response.body)
    assert_match(/<p id="escaped"><time datetime="2026-11-01T09:30:15Z">&lt;b&gt;x&lt;\/b&gt;<\/time><\/p>/, response.body)
    assert_match(/<p id="format"><time datetime="2026-11-01T09:30:15Z">Posted<\/time><\/p>/, response.body)
    assert_match(/<p id="builder"><time datetime="2026-11-01T09:30:15Z">Posted<\/time><\/p>/, response.body)
  end
end
"#;

#[test]
fn time_tag_renders_as_rails_does() {
    app(MARKUP)
        .write("test/controllers/time_tag_controller_test.rb", TEST)
        .run_test("test/controllers/time_tag_controller_test.rb")
        .assert_passes();
}

/// With no text and no block, Rails' text is `I18n.l(t, format: …)`
/// ("November 01, 2026 09:30" for the en default), which is not modeled.
#[test]
fn a_time_tag_without_text_is_refused() {
    for markup in [
        "  <%= time_tag @article.created_at %>\n",
        "  <%= time_tag @article.created_at, format: :short %>\n",
    ] {
        let errors = app(markup).check_errors();
        assert!(errors.iter().any(|e| e.contains("time_tag not supported") && e.contains("I18n.l")), "{markup}: {errors:?}");
    }
}

/// The same page in the spinel binary, where #1593 was found.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn time_tag_renders_as_rails_does_on_spinel() {
    app(MARKUP)
        .run_spinel(concat!(
            "Db.configure(\":memory:\")\n",
            "Schema.statements.each { |sql| Db.exec(sql) }\n",
            "ActiveRecord.adapter = SqliteAdapter\n",
            "Db.exec(\"INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (1, 'Getting Started with Rails', 'Body', '2026-11-01 09:30:15.5', '2026-11-01 09:30:15.5')\")\n",
            "controller = ArticlesController.new\n",
            "controller.request_method = \"GET\"\n",
            "controller.params = {\"id\" => \"1\"}\n",
            "controller.process_action(:show)\n",
            "body = controller.body\n",
            "raise body unless body.include?('<p id=\"text\"><time datetime=\"2026-11-01T09:30:15Z\">Posted</time></p>')\n",
            "raise body unless body.include?('<p id=\"block\"><time datetime=\"2026-11-01T09:30:15Z\">In block</time></p>')\n",
            "raise body unless body.include?('<p id=\"class\"><time datetime=\"2026-11-01T09:30:15Z\" class=\"when\">Posted</time></p>')\n",
            "raise body unless body.include?('<p id=\"builder\"><time datetime=\"2026-11-01T09:30:15Z\">Posted</time></p>')\n",
            "puts \"time_tag passed\"\n",
        ))
        .assert_passes();
}

/// A record time's `iso8601` on the other targets is not Rails' text
/// (TypeScript `…T09:30:15.500Z`, Python `…T09:30:15.500000+00:00`;
/// Crystal's, Go's and Rust's reads don't build), so `time_tag` is
/// refused there, naming the target.
#[test]
fn time_tag_is_refused_off_the_ruby_family() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Typescript, BuildTarget::Crystal, BuildTarget::Go, BuildTarget::Python, BuildTarget::Rust] {
        let (_, errors) = app("  <%= time_tag @article.created_at, \"Posted\" %>\n").emit(target);
        assert!(errors.iter().any(|e| e.contains("time_tag not supported")), "{target:?}: {errors:?}");
    }
}
