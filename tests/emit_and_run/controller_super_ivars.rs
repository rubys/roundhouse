//! An action override that calls `super` runs the overridden action
//! first, so the ivars that action writes are set when the override
//! reads them. campfire main's `Messages::ByBotsController#create`
//! reads `@message` after `super`; here a subclass of real-blog's
//! `ArticlesController` reads an ivar only the parent's `index` writes.

use super::emit_and_run;

#[test]
fn an_override_reads_the_ivars_its_super_call_wrote() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n    @listed = Article.count\n",
        )
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n\n  get \"drafts\", to: \"drafts#index\"\n",
        )
        .write(
            "app/controllers/drafts_controller.rb",
            "class DraftsController < ArticlesController\n  def index\n    super\n    @listed_label = \"#{@listed + 1} listed\"\n  end\nend\n",
        )
        .write(
            "app/views/drafts/index.html.erb",
            "<p id=\"listed\"><%= @listed_label %></p>\n<p id=\"count\">count <%= @listed %></p>\n",
        )
        .write(
            "test/controllers/drafts_controller_test.rb",
            r##"require "test_helper"

class DraftsControllerTest < ActionDispatch::IntegrationTest
  test "the override reads what super wrote" do
    Article.create!(title: "One", body: "A body long enough.")
    get "/drafts"
    assert_response :success
    assert_includes response.body, "#{Article.count + 1} listed"
    assert_includes response.body, "count #{Article.count}"
  end
end
"##,
        )
        .run_test("test/controllers/drafts_controller_test.rb")
        .assert_passes();
}
