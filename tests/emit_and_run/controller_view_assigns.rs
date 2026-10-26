//! Rails' `view_assigns` on a functional test's controller. The test
//! below was run first in the generated blog on Rails 8.1 (`bin/rails
//! test`): the first two tests pass, the control fails.

use super::emit_and_run;

const TEST: &str = r##"
require "test_helper"

class ArticlesControllerTest < ActionController::TestCase
  setup do
    @article = Article.create!(title: "Getting Started", body: "A body that is long enough.")
  end

  test "view_assigns holds what the action assigned" do
    get :show, params: { id: @article.id }
    assert_equal @article, @controller.view_assigns["article"]
    assert_nil @controller.view_assigns["articles"]
  end

  test "a later request does not see the earlier one's assigns" do
    get :show, params: { id: @article.id }
    get :index
    assert_nil @controller.view_assigns["article"]
    assert_includes @controller.view_assigns["articles"].to_a, @article
  end

  test "control: a wrong value fails" do
    get :show, params: { id: @article.id }
    assert_nil @controller.view_assigns["article"]
  end
end
"##;

const FILE: &str = "test/controllers/articles_controller_test.rb";

#[test]
fn view_assigns_hold_what_the_last_request_assigned() {
    let run = emit_and_run::real_blog().write(FILE, TEST.trim_start()).run_test(FILE);
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    let out = format!("{}{}", run.stdout, run.stderr);
    assert!(out.contains("FAIL ArticlesControllerTest#test_control_a_wrong_value_fails"), "{out}");
    assert_eq!(out.matches("FAIL ").count(), 1, "{out}");
}

#[test]
#[ignore = "requires spin and spinel on PATH plus SQLite development libraries"]
fn view_assigns_pass_in_the_spinel_build() {
    let passing = TEST.split("\n  test \"control: a wrong value fails\"").next().unwrap().to_string() + "\nend\n";
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
