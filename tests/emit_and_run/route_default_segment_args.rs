//! A route segment under `scope defaults:` is typed `String` by its
//! default (#930), and a test that passes it a record or a Symbol hands
//! it a String, as Rails' `to_param` does.
//!
//! campfire's own tests write both: `user_profile_url(users(:jason))`
//! and `user_sidebar_url(:me)`, against `scope defaults: { user_id:
//! "me" }`. The app's bodies already reached the slot through
//! `apply_route_param_lowering`; the test classes did not, and Spinel
//! refused `--rbs seed contradicted: parameter user_id … is declared
//! String but this call passes Integer` (and `… Symbol`).
//!
//! CRuby interpolates any of the three, so the Ruby run pins the URLs
//! and the Spinel emit pins what reaches the String slot.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

const TEST_PATH: &str = "test/controllers/summaries_controller_test.rb";

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "    resources :comments, only: [:create, :destroy]\n",
            concat!(
                "    resources :comments, only: [:create, :destroy]\n",
                "    scope defaults: { article_id: \"latest\" } do\n",
                "      resource :summary, only: :show\n",
                "    end\n",
            ),
        )
        .write(
            "app/controllers/summaries_controller.rb",
            "class SummariesController < ApplicationController\n  def show\n    render plain: params[:article_id]\n  end\nend\n",
        )
        .write(
            TEST_PATH,
            r#"require "test_helper"

class SummariesControllerTest < ActionDispatch::IntegrationTest
  test "a defaulted segment takes a record, a symbol, or nothing" do
    article = articles(:one)
    assert_equal "/articles/#{article.id}/summary", article_summary_path(article)
    assert_equal "/articles/latest/summary", article_summary_path(:latest)
    assert_equal "/articles/latest/summary", article_summary_path
  end

  test "the default reaches the action" do
    get article_summary_path
    assert_equal "latest", response.body
  end
end
"#,
        )
}

#[test]
fn a_defaulted_segment_renders_a_record_a_symbol_and_its_default() {
    overlay().run_test(TEST_PATH).assert_passes();
}

#[test]
fn a_defaulted_segment_is_handed_a_string_on_spinel() {
    let (emitted, errors) = overlay().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let rbs = std::fs::read_to_string(emitted.join("app/route_helpers.rbs")).expect("route_helpers.rbs");
    assert!(
        rbs.contains("def self.article_summary_path: (?String article_id) -> String"),
        "the defaulted segment is declared by its default:\n{rbs}"
    );
    let test = std::fs::read_to_string(emitted.join("test/summaries_controller_test.rb"))
        .expect("emitted test");
    let calls: Vec<&str> = test.lines().filter(|l| l.contains("article_summary_path(")).collect();
    assert!(
        calls.iter().any(|l| l.contains("article_summary_path(article.id.to_s)")),
        "a record reaches the String slot as `id.to_s`: {calls:#?}"
    );
    assert!(
        calls.iter().any(|l| l.contains(r#"article_summary_path("latest")"#)),
        "a Symbol reaches the String slot as its text: {calls:#?}"
    );
}
