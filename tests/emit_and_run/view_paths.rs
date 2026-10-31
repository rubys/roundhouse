//! View paths: an initializer prepends `app/views/themes/next` to every
//! controller's lookup and `ApplicationMailer` prepends `app/views/mailers`
//! to the mailers'. The prepended layout wraps every page, a record and an
//! association partial render from the prepended path while a template only
//! `app/views` has still renders, and a mailer finds its template and its
//! layout under its own path. Expected bodies are Rails 8.1's for the same
//! tree; before view paths were read, each `themes/next` template was one no
//! controller rendered.

use super::emit_and_run;

const TEST: &str = r##"
require "test_helper"

class ViewPathsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @article = Article.create!(title: "Getting Started", body: "A body that is long enough.")
    @article.comments.create!(commenter: "Ann", body: "First!")
  end

  test "an index renders in the prepended layout, with the prepended record partial" do
    get "/articles"
    assert_response :success
    assert_includes response.body, %(<body data-layout="next">)
    assert_includes response.body, %(<p class="next-article">Getting Started (1)</p>)
  end

  test "a template only app/views has renders the prepended association partial" do
    get "/articles/#{@article.id}"
    assert_response :success
    assert_includes response.body, %(<body data-layout="next">)
    assert_includes response.body, %(<p class="next-comment">Ann: First!</p>)
  end

  test "a mailer renders its template and its layout from its own path" do
    mail = TitleMailer.titled(@article).deliver_now
    assert_equal "[mailers] Title: GETTING STARTED\n\n", mail.body.to_s
  end
end
"##;

const FILE: &str = "test/controllers/view_paths_controller_test.rb";

fn view_paths_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/initializers/view_paths.rb", "ActionController::Base.prepend_view_path \"app/views/themes/next\"\n")
        .write("app/views/themes/next/layouts/application.html.erb", "<!DOCTYPE html>\n<html><body data-layout=\"next\"><%= yield %></body></html>\n")
        .write(
            "app/views/themes/next/articles/_article.html.erb",
            "<p class=\"next-article\"><%= article.title %> (<%= article.comments.size %>)</p>\n",
        )
        .write("app/views/themes/next/comments/_comment.html.erb", "<p class=\"next-comment\"><%= comment.commenter %>: <%= comment.body %></p>\n")
        .edit(
            "app/mailers/application_mailer.rb",
            "  layout \"mailer\"\n",
            "  layout \"mailer\"\n  prepend_view_path \"app/views/mailers\"\n",
        )
        .write(
            "app/mailers/title_mailer.rb",
            "class TitleMailer < ApplicationMailer\n  def titled(article)\n    @article = article\n    mail(to: \"reader@example.com\", subject: \"Titled\")\n  end\nend\n",
        )
        .write("app/views/mailers/title_mailer/titled.text.erb", "Title: <%= @article.title.upcase %>\n")
        .write("app/views/mailers/layouts/mailer.text.erb", "[mailers] <%= yield %>\n")
        .write(FILE, TEST.trim_start())
}

#[test]
fn templates_render_from_the_view_paths_the_app_prepends() {
    let run = view_paths_app().run_test(FILE);
    run.assert_passes();
    assert!(run.stdout.contains("ViewPathsControllerTest: 3 tests passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires spin and spinel on PATH plus SQLite development libraries"]
fn templates_render_from_the_view_paths_the_app_prepends_in_the_spinel_build() {
    let (emitted, errors) = view_paths_app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let file = "test/view_paths_controller_test.rb";
    assert!(emitted.join(file).exists(), "{file} was not emitted in {}", emitted.display());
    let output = std::process::Command::new("spin").args(["test", file]).current_dir(&*emitted).output().expect("spawn spin");
    assert!(
        output.status.success(),
        "spin test failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        emitted.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
