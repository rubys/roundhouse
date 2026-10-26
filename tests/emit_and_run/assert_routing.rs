//! Rails' `assert_generates` and `assert_routing`. The test below was run
//! first in the generated blog on Rails 8.1 (`bin/rails test`), with
//! these routes: the first test passes, the four others fail.

use super::emit_and_run;

const ROUTES: &str = r##"Rails.application.routes.draw do
  root "articles#index"
  get "/rss" => "articles#index", :format => "rss"
  get "/~:author" => "articles#index"
  get "/archive(/page/:page)" => "articles#index"

  resources :articles do
    resources :comments, only: [:create, :destroy]
  end
end
"##;

const TEST: &str = r##"
require "test_helper"

class RoutesControllerTest < ActionDispatch::IntegrationTest
  test "generates the routes" do
    assert_routing "/", controller: "articles", action: "index"
    assert_routing "/articles/1", controller: "articles", action: "show", id: "1"
    assert_routing "/articles/1.json", controller: "articles", action: "show", id: "1", format: "json"
    assert_routing({ method: :delete, path: "/articles/1/comments/2" }, controller: "comments", action: "destroy", article_id: "1", id: "2")
    assert_generates "/articles", controller: "articles", action: "create"
    assert_generates "/articles/1", controller: "articles", action: "show", id: 1
    assert_routing "/rss", controller: "articles", action: "index", format: "rss"
    assert_routing "/~alice", controller: "articles", action: "index", author: "alice"
    assert_generates "/~a%20b", controller: "articles", action: "index", author: "a b"
    assert_routing "/archive/page/2", controller: "articles", action: "index", page: "2"
    assert_generates "/articles/1", { controller: "articles", action: "show", id: "1", x: "y" }, {}, { x: "y" }
    assert_generates "/~alice", { controller: "articles", action: "index", author: "alice", page: "2" }, {}, { page: "2" }
  end

  test "the root declared first wins" do
    assert_routing "/articles", controller: "articles", action: "index"
  end

  test "undeclared extras fail" do
    assert_generates "/articles/1", controller: "articles", action: "show", id: "1", x: "y"
  end

  test "a wrong path fails" do
    assert_generates "/articles/2", controller: "articles", action: "show", id: "1"
  end

  test "a missing segment fails" do
    assert_generates "/articles", controller: "articles", action: "show"
  end
end
"##;

const FILE: &str = "test/controllers/routes_controller_test.rb";

fn app(test: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("config/routes.rb", ROUTES).write(FILE, test.trim_start())
}

#[test]
fn generated_routes_pass_and_the_controls_fail() {
    let run = app(TEST).run_test(FILE);
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    let out = format!("{}{}", run.stdout, run.stderr);
    for (failing, says) in [
        ("test_the_root_declared_first_wins", "The generated path </> did not match </articles>"),
        ("test_undeclared_extras_fail", "found extras <{x: \"y\"}>, not <{}>"),
        ("test_a_wrong_path_fails", "The generated path </articles/1> did not match </articles/2>"),
        ("test_a_missing_segment_fails", "No route matches {action: \"show\", controller: \"articles\"}"),
    ] {
        assert!(out.contains(&format!("FAIL RoutesControllerTest#{failing}: {says}")), "{failing} did not fail with `{says}`:\n{out}");
    }
    assert!(!out.contains("FAIL RoutesControllerTest#test_generates_the_routes"), "{out}");
    assert_eq!(out.matches("FAIL ").count(), 4, "{out}");
}

#[test]
#[ignore = "requires spin and spinel on PATH plus SQLite development libraries"]
fn generated_routes_pass_in_the_spinel_build() {
    // Only the passing test: a failing one would fail the binary.
    let passing = TEST.split("\n  test \"the root declared first wins\"").next().unwrap().to_string() + "\nend\n";
    let (emitted, errors) = app(&passing).emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let file = "test/routes_controller_test.rb";
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
