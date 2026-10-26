//! Rails' `assert_recognizes`. The test below was run first in the
//! generated blog on Rails 8.1 (`bin/rails test`): the first test
//! passes, the four others fail.

use super::emit_and_run;

const TEST: &str = r##"
require "test_helper"

class RoutesControllerTest < ActionDispatch::IntegrationTest
  test "recognizes the routes" do
    assert_recognizes({ controller: "articles", action: "index" }, "/articles")
    assert_recognizes({ controller: "articles", action: "index" }, "/")
    assert_recognizes({ controller: "articles", action: "show", id: "1" }, "/articles/1")
    assert_recognizes({ controller: "articles", action: "show", id: "1", format: "json" }, "/articles/1.json")
    assert_recognizes({ controller: "articles", action: "create" }, { path: "/articles", method: :post })
    assert_recognizes({ controller: "comments", action: "destroy", article_id: "1", id: "2" }, { path: "/articles/1/comments/2", method: :delete })
    assert_recognizes({ controller: "articles", action: "index", page: "2" }, "/articles", { page: "2" })
    assert_recognizes({ controller: "articles", action: "show", id: "1" }, "articles//1?x=1")
    assert_recognizes({ controller: "articles", action: "show", id: "1" }, "http://blog.test/articles/1#top")
    assert_recognizes({ controller: "articles", action: "index" }, "http://blog.test?next=/articles/1")
    assert_recognizes({ controller: "articles", action: "show", id: "1" }, "/articles/1?next=http://x.test/y")
    assert_recognizes({ controller: "active_storage/disk", action: "show", encoded_key: "abc", filename: "x" }, "/rails/active_storage/disk/abc/x")
  end

  test "a wrong expectation fails" do
    assert_recognizes({ controller: "articles", action: "show", id: 1 }, "/articles/1")
  end

  test "an unknown path fails" do
    assert_recognizes({ controller: "articles", action: "index" }, "/nowhere")
  end

  test "a slash before the query fails" do
    assert_recognizes({ controller: "articles", action: "show", id: "1" }, "/articles/1/?x=1")
  end

  test "a wrong verb fails" do
    assert_recognizes({ controller: "articles", action: "create" }, "/articles")
  end
end
"##;

const FILE: &str = "test/controllers/routes_controller_test.rb";

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write(FILE, TEST.trim_start())
}

#[test]
fn recognized_routes_pass_and_the_controls_fail() {
    let run = app().run_test(FILE);
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    let out = format!("{}{}", run.stdout, run.stderr);
    for failing in [
        "test_a_wrong_expectation_fails",
        "test_an_unknown_path_fails",
        "test_a_slash_before_the_query_fails",
        "test_a_wrong_verb_fails",
    ] {
        assert!(out.contains(&format!("FAIL RoutesControllerTest#{failing}")), "{failing} did not fail:\n{out}");
    }
    assert!(!out.contains("FAIL RoutesControllerTest#test_recognizes_the_routes"), "{out}");
    assert_eq!(out.matches("FAIL ").count(), 4, "{out}");
}

#[test]
#[ignore = "requires spin and spinel on PATH plus SQLite development libraries"]
fn recognized_routes_pass_in_the_spinel_build() {
    // Only the passing test: a failing one would fail the binary.
    let passing = TEST.split("\n  test \"a wrong expectation fails\"").next().unwrap().to_string() + "\nend\n";
    let (emitted, errors) = emit_and_run::real_blog().write(FILE, passing.trim_start()).emit(roundhouse::project::BuildTarget::Spinel);
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
