//! A session read answers whatever was stored. Rails keeps the value's
//! type in the session, so `session[:authenticated_at] = 1760000000`
//! reads back an Integer, and `ts.present? && Time.at(ts.to_i) > ...`
//! is the sign-in freshness check an app writes over it (hindmost's
//! `ApplicationController#authenticated?`). The expected bodies are what
//! Rails 8.1.3 answers for the same controller through
//! `ActionDispatch::Integration::Session`.

use super::emit_and_run;

#[test]
fn a_session_read_answers_the_integer_that_was_stored() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n\n  get \"stamp\", to: \"stamps#show\"\n  get \"stamp/create\", to: \"stamps#create\"\n  get \"stamp/forget\", to: \"stamps#forget\"\n",
        )
        .write(
            "app/controllers/stamps_controller.rb",
            r##"class StampsController < ApplicationController
  def create
    session[:authenticated_at] = 1760000000
    render plain: "create fresh=#{fresh?}"
  end

  def show
    render plain: "show fresh=#{fresh?} at=#{session[:authenticated_at]}"
  end

  def forget
    gone = session.delete(:authenticated_at)
    render plain: "forget #{gone.present? ? "was " + gone.to_s : "nothing"} fresh=#{fresh?}"
  end

  private

  def fresh?
    ts = session[:authenticated_at]
    ts.present? && Time.at(ts.to_i) > Time.at(1_700_000_000)
  end
end
"##,
        )
        .write(
            "test/controllers/stamps_controller_test.rb",
            r##"require "test_helper"

class StampsControllerTest < ActionDispatch::IntegrationTest
  test "the stored Integer reads back and passes the freshness check" do
    get "/stamp"
    assert_equal "show fresh=false at=", response.body
    get "/stamp/create"
    assert_equal "create fresh=true", response.body
    get "/stamp"
    assert_equal "show fresh=true at=1760000000", response.body
    get "/stamp/forget"
    assert_equal "forget was 1760000000 fresh=false", response.body
    get "/stamp"
    assert_equal "show fresh=false at=", response.body
  end
end
"##,
        )
        .run_test("test/controllers/stamps_controller_test.rb")
        .assert_passes();
}
