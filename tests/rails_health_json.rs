//! The synthesized `Rails::HealthController` answers JSON to a JSON
//! request, as Rails 8.1's does.
//!
//! Rails 8.1's `Rails::HealthController#show` is a `respond_to` with an
//! HTML arm (the green page) and a JSON arm, `render json: { status:
//! "up", timestamp: Time.current.iso8601 }`. The synthesized controller
//! was 8.0's, `render html:` only, so `GET /up.json` got the page.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn health_check_answers_json_and_html() {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"up\" => \"rails/health#show\"\nend\n")
        .run_ruby(
            r##"
def probe(path)
  status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  text = +""
  body.each { |part| text << part }
  [status, (headers["content-type"] || headers["Content-Type"]).to_s, text]
end
status, type, text = probe("/up.json")
raise "json #{status} #{type} #{text}" unless status == 200 && type.start_with?("application/json") &&
  text.match?(/\A\{"status":"up","timestamp":"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(Z|[+-]\d\d:\d\d)"\}\z/)
status, type, text = probe("/up")
raise "html #{status} #{type} #{text}" unless status == 200 && type.start_with?("text/html") &&
  text == %(<!DOCTYPE html><html><body style="background-color: green"></body></html>)
puts "ALL OK"
"##,
        )
        .assert_passes();
}
