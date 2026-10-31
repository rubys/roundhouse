//! A route constraint's `[\w-]+` is word characters and `-`, as in Ruby,
//! not the letters `w` and `-` (roundhouse#1374). `parse_bracket` read
//! every escaped char inside a bracket as the literal char, so an
//! ordinary token missed the route and answered 404. Rails 8.1.4 answers
//! 200 for each request below (#1374's table).

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const TEST: &str = r##"require "test_helper"

class TicketsControllerTest < ActionDispatch::IntegrationTest
  test "a word-class constraint matches word tokens" do
    ["abc", "abc_1", "abc-def", "w", "w-w"].each do |token|
      get "/tickets/#{token}"
      assert_response :success
      assert_equal %({"token":"#{token}"}), response.body
    end
  end
end
"##;

#[test]
fn a_word_escape_in_a_route_constraint_matches_word_characters() {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/tickets_controller.rb",
            "class TicketsController < ApplicationController\n  def show\n    render json: { token: params[:token] }\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"tickets/:token\", to: \"tickets#show\", constraints: { token: /[\\w-]+/ }\nend\n",
        )
        .write("test/test_helper.rb", "ENV[\"RAILS_ENV\"] ||= \"test\"\nrequire_relative \"../config/environment\"\nrequire \"rails/test_help\"\n")
        .write("test/controllers/tickets_controller_test.rb", TEST)
        .run_test("test/controllers/tickets_controller_test.rb")
        .assert_passes();
}
