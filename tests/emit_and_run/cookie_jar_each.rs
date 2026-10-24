//! `cookies.each { |key, value| ... }`, as Lobsters' `ApplicationController`
//! clears unknown cookies: Rails' cookie jar is Enumerable over names and
//! values. The runtime jar defines `each`; the controller registry did
//! not, so the emit stopped with "no known method `each` on
//! ActionDispatch::Cookies::CookieJar".

use super::emit_and_run;

const FILTER: &str = r##"  before_action :remove_unknown_cookies

  def remove_unknown_cookies
    cookies.each do |key, _value|
      next if key == "keep"
      next unless key.start_with?("junk")
      cookies.delete(key)
    end
  end
"##;

const TEST: &str = r##"require "test_helper"

class CookieFilterControllerTest < ActionDispatch::IntegrationTest
  test "unknown cookies are deleted and known ones kept" do
    cookies["junk_a"] = "1"
    cookies["keep"] = "2"
    get articles_url
    assert_response :success
    assert cookies["junk_a"].blank?
    assert_equal "2", cookies["keep"]
  end
end
"##;

fn app(filter: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n",
            &format!("class ApplicationController < ActionController::Base\n{filter}"),
        )
        .write("test/controllers/cookie_filter_controller_test.rb", TEST)
}

#[test]
fn unknown_cookies_are_deleted_from_the_jar_s_each() {
    app(FILTER).run_test("test/controllers/cookie_filter_controller_test.rb").assert_passes();
}

// Control: a filter that deletes nothing fails the same test.
#[test]
fn the_cookie_test_fails_when_nothing_is_deleted() {
    let run = app(&FILTER.replace("      cookies.delete(key)\n", "")).run_test("test/controllers/cookie_filter_controller_test.rb");
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    assert!(!run.success, "a kept junk cookie passed:\n{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("FAIL CookieFilterControllerTest"), "the test did not run:\n{}\n{}", run.stdout, run.stderr);
}

#[test]
#[ignore = "requires spin and spinel on PATH plus SQLite development libraries"]
fn unknown_cookies_are_deleted_in_the_spinel_build() {
    let (emitted, errors) = app(FILTER).emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(emitted.join("test/cookie_filter_controller_test.rb").exists(), "no emitted test file in {}", emitted.display());
    let output = std::process::Command::new("spin")
        .args(["test", "test/cookie_filter_controller_test.rb"])
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
