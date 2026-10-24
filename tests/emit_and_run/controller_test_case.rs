//! Rails' `ActionController::TestCase`. The test below was run first in
//! the generated blog on Rails 8.1 (`bin/rails test`): four tests pass,
//! and the two that ask for an action without a route (an unknown
//! action, `show` without its `id`) fail with
//! ActionController::UrlGenerationError.

use super::emit_and_run;

const TEST: &str = r##"
require "test_helper"

class ArticlesControllerTest < ActionController::TestCase
  setup do
    @article = Article.create!(title: "Getting Started", body: "A body that is long enough.")
  end

  test "index renders" do
    get :index
    assert_response :success
    assert_includes response.body, "Getting Started"
  end

  test "params arrive as strings" do
    get :show, params: { id: @article.id }
    assert_response :success
    assert_equal @article.id.to_s, @controller.params["id"]
  end

  test "create redirects" do
    assert_difference("Article.count", 1) do
      post :create, params: { article: { title: "New one", body: "A body that is long enough." } }
    end
    assert_redirected_to article_path(Article.last)
  end

  test "the session is there before the request" do
    session[:seen] = "yes"
    get :index
    assert_equal "yes", @controller.session[:seen]
  end

  test "an action without a route fails" do
    get :nope
  end

  test "a missing path parameter fails" do
    get :show
  end
end
"##;

const FILE: &str = "test/controllers/articles_controller_test.rb";

fn app(test: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog().write(FILE, test.trim_start())
}

#[test]
fn functional_tests_pass_and_unrouted_actions_fail() {
    let run = app(TEST).run_test(FILE);
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    let out = format!("{}{}", run.stdout, run.stderr);
    for failing in ["test_an_action_without_a_route_fails", "test_a_missing_path_parameter_fails"] {
        assert!(out.contains(&format!("FAIL ArticlesControllerTest#{failing}: No route matches")), "{failing} did not fail:\n{out}");
    }
    assert_eq!(out.matches("FAIL ").count(), 2, "{out}");
}

#[test]
#[ignore = "requires spin and spinel on PATH plus SQLite development libraries"]
fn functional_tests_pass_in_the_spinel_build() {
    // Only the passing tests: a failing one would fail the binary.
    let passing = TEST.split("\n  test \"an action without a route fails\"").next().unwrap().to_string() + "\nend\n";
    let (emitted, errors) = emit_and_run::real_blog().write(FILE, passing.trim_start()).emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let file = "test/articles_controller_test.rb";
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
