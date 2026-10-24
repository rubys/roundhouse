//! `Rails.application.config.session_options[:key]` in a controller, in
//! an app that reopens `class << Rails.application` (Lobsters): the read
//! is grounded after analysis to `Rails.application.session_cookie_key`,
//! which had no type, so with `Rails.application` untyped the emit
//! stopped with "no known method `session_cookie_key` on untyped".

use super::emit_and_run;

const FILTER: &str = r##"  before_action :keep_only_the_session_cookie

  def keep_only_the_session_cookie
    cookies.delete("other") unless "other" == Rails.application.config.session_options[:key]
    response.headers["X-Session-Key"] = Rails.application.config.session_options[:key]
  end
"##;

const TEST: &str = r##"require "test_helper"

class SessionKeyControllerTest < ActionDispatch::IntegrationTest
  test "the session cookie key is the application's" do
    get articles_url
    assert_response :success
    assert_equal "_real_blog_session", response.headers["X-Session-Key"]
  end
end
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/application.rb",
            "module RealBlog\n",
            "class << Rails.application\n  def name\n    \"Real Blog\"\n  end\nend\n\nmodule RealBlog\n",
        )
        .edit(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n",
            &format!("class ApplicationController < ActionController::Base\n{FILTER}"),
        )
        .write("test/controllers/session_key_controller_test.rb", TEST)
}

#[test]
fn the_session_cookie_key_reads_as_the_application_s() {
    app().run_test("test/controllers/session_key_controller_test.rb").assert_passes();
}

#[test]
#[ignore = "requires spin and spinel on PATH plus SQLite development libraries"]
fn the_session_cookie_key_reads_as_the_application_s_in_the_spinel_build() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(emitted.join("test/session_key_controller_test.rb").exists(), "no emitted test file in {}", emitted.display());
    let output = std::process::Command::new("spin")
        .args(["test", "test/session_key_controller_test.rb"])
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

// Control: another expected key fails the same test.
#[test]
fn another_session_cookie_key_fails_the_test() {
    let run = app()
        .write("test/controllers/session_key_controller_test.rb", &TEST.replace("_real_blog_session", "_other_session"))
        .run_test("test/controllers/session_key_controller_test.rb");
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    assert!(!run.success, "a wrong key passed:\n{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("FAIL SessionKeyControllerTest"), "the test did not run:\n{}\n{}", run.stdout, run.stderr);
}
