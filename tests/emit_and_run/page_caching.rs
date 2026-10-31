//! actionpack-page_caching: `caches_page :show, if: CACHE_PAGE` writes a
//! 200 GET's body under `page_cache_directory`, at the request path with
//! the response's extension, only while the guard the constant holds is
//! true; and an `ActionDispatch::Integration::Session` answers a request
//! in process, as lobsters' CachePageJob replays a page to cache it.
//! Before, `caches_page` was dropped with its guard constant, and the
//! session was an unsupported constant.

use super::emit_and_run;

const TEST_PATH: &str = "test/controllers/sites_controller_test.rb";

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/sites/:id\" => \"sites#show\"\n",
        )
        .edit(
            "app/controllers/application_controller.rb",
            "  stale_when_importmap_changes\n",
            "  stale_when_importmap_changes\n\n  CACHE_PAGE = proc { params[:nocache].blank? }\n",
        )
        .write(
            "app/controllers/sites_controller.rb",
            concat!(
                "class SitesController < ApplicationController\n",
                "  caches_page :show, if: CACHE_PAGE\n\n",
                "  def show\n",
                "    if params[:id] == \"missing\"\n",
                "      head :not_found\n",
                "    else\n",
                "      render html: \"<p>site #{params[:id]}</p>\".html_safe\n",
                "    end\n",
                "  end\n",
                "end\n",
            ),
        )
}

#[test]
fn caches_page_writes_a_guarded_200_get_under_the_cache_directory() {
    app()
        .write(
            TEST_PATH,
            r#"require "test_helper"
require "tmpdir"

class SitesControllerTest < ActionDispatch::IntegrationTest
  setup do
    @dir = Dir.mktmpdir
    ActionController::Base.perform_caching = true
    ActionController::Base.page_cache_directory = @dir
  end

  teardown do
    ActionController::Base.perform_caching = false
    FileUtils.rm_rf(@dir)
  end

  test "a guarded 200 GET is written at its path, and nothing else is" do
    get "/sites/a"
    assert_equal "<p>site a</p>", response.body
    assert_equal "<p>site a</p>", File.read(File.join(@dir, "sites/a.html"))
    get "/sites/b?nocache=1"
    assert_equal false, File.exist?(File.join(@dir, "sites/b.html"))
    get "/sites/missing"
    assert_equal false, File.exist?(File.join(@dir, "sites/missing.html"))
  end

  test "nothing is written unless perform_caching" do
    ActionController::Base.perform_caching = false
    get "/sites/c"
    assert_equal false, File.exist?(File.join(@dir, "sites/c.html"))
  end
end
"#,
        )
        .run_test(TEST_PATH)
        .assert_passes();
}

#[test]
fn an_integration_session_answers_in_process_for_a_job_to_cache() {
    app()
        .write(
            "app/jobs/cache_site_job.rb",
            concat!(
                "class CacheSiteJob < ApplicationJob\n",
                "  def perform(path)\n",
                "    session = ActionDispatch::Integration::Session.new(Rails.application)\n",
                "    session.host! \"example.org\"\n",
                "    session.https!\n",
                "    session.get(path)\n",
                "    return unless session.response.status == 200\n",
                "    ApplicationController.cache_page(session.response.body, path, nil, false)\n",
                "  end\n",
                "end\n",
            ),
        )
        .write(
            TEST_PATH,
            r#"require "test_helper"
require "tmpdir"
require_relative "../../main"

class SitesControllerTest < ActionDispatch::IntegrationTest
  test "the job's session renders the page and the class-side cache_page writes it" do
    dir = Dir.mktmpdir
    ActionController::Base.perform_caching = true
    ActionController::Base.page_cache_directory = dir
    CacheSiteJob.new.perform("/sites/d")
    assert_equal "<p>site d</p>", File.read(File.join(dir, "sites/d.html"))
    CacheSiteJob.new.perform("/sites/missing")
    assert_equal false, File.exist?(File.join(dir, "sites/missing.html"))
  ensure
    ActionController::Base.perform_caching = false
    FileUtils.rm_rf(dir)
  end
end
"#,
        )
        .run_test(TEST_PATH)
        .assert_passes();
}

#[test]
fn a_page_cached_under_public_is_served_ahead_of_the_routes() {
    app()
        .write(
            TEST_PATH,
            r#"require "test_helper"

class SitesControllerTest < ActionDispatch::IntegrationTest
  test "a page written under public/ is what the static lookup answers, until it is expired" do
    ActionController::Base.perform_caching = true
    ActionController::Base.page_cache_directory = Rails.public_path.to_s
    assert_equal "", ActionController::PublicFiles.lookup("/sites/e")
    get "/sites/e"
    assert_equal "/sites/e.html", ActionController::PublicFiles.lookup("/sites/e")
    assert_equal "/sites/e.html", ActionController::PublicFiles.lookup("/sites/e.html")
    SitesController.expire_page("/sites/e")
    assert_equal "", ActionController::PublicFiles.lookup("/sites/e")
    assert_equal false, File.exist?("public/sites/e.html")
  ensure
    ActionController::Base.perform_caching = false
    FileUtils.rm_rf("public/sites")
  end
end
"#,
        )
        .run_test(TEST_PATH)
        .assert_passes();
}
