//! `1.hour.from_now.httpdate`, as Lobsters sets `Expires`: the
//! `httpdate` grounding builds `t.getutc.strftime(...)` after analysis,
//! and the `getutc` it built had no type, so the post-lowering
//! diagnostics stopped the emit with "no known method `getutc` on
//! untyped" while `check` was clean.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "  def show\n",
            "  def show\n    response.headers[\"Expires\"] = 1.hour.from_now.httpdate\n",
        )
        .write("test/controllers/expires_controller_test.rb", TEST)
}

const TEST: &str = r##"require "test_helper"

class ExpiresControllerTest < ActionDispatch::IntegrationTest
  test "show sends an HTTP date" do
    get article_url(articles(:one))
    assert_response :success
    assert_match(/\A[A-Z][a-z]{2}, \d{2} [A-Z][a-z]{2} \d{4} \d{2}:\d{2}:\d{2} GMT\z/, response.headers["Expires"])
  end
end
"##;

#[test]
fn an_http_date_an_hour_ahead_is_sent() {
    app().run_test("test/controllers/expires_controller_test.rb").assert_passes();
}

// Control: without the header the same test fails.
#[test]
fn the_expires_test_fails_without_the_header() {
    let run = emit_and_run::real_blog().write("test/controllers/expires_controller_test.rb", TEST).run_test("test/controllers/expires_controller_test.rb");
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    assert!(!run.success, "no Expires header passed:\n{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("FAIL ExpiresControllerTest"), "the test did not run:\n{}\n{}", run.stdout, run.stderr);
}

#[test]
#[ignore = "requires spin and spinel on PATH plus SQLite development libraries"]
fn an_http_date_an_hour_ahead_is_sent_in_the_spinel_build() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(emitted.join("test/expires_controller_test.rb").exists(), "no emitted test file in {}", emitted.display());
    let output = std::process::Command::new("spin")
        .args(["test", "test/expires_controller_test.rb"])
        .current_dir(&*emitted)
        .output()
        .expect("spawn spin");
    assert!(
        output.status.success(),
        "spin test failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        emitted.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
