//! A 404 the app does not rescue answers in the request's format, as
//! Rails' production exceptions app does.
//!
//! In production Rails renders `ActionController::RoutingError`,
//! `AbstractController::ActionNotFound` and
//! `ActiveRecord::RecordNotFound` through
//! `ActionDispatch::PublicExceptions`: when `request.formats.first` is
//! JSON the body is `{"status":404,"error":"Not Found"}` (typed
//! `application/json; charset=UTF-8`), otherwise the static HTML page.
//! `formats` reads the `format` parameter, then an Accept header Rails
//! trusts (not a browser's list with `*/*` in it), then the path's
//! extension. Both ruby-family dispatchers answered every 404 with the
//! HTML page, so a JSON client got `<h1>404 Not Found</h1>`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/gizmos_controller.rb",
            "class GizmosController < ApplicationController\n  def index\n    render json: { gizmos: [] }\n  end\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    render json: Widget.find(params[:id])\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :gizmos, only: %i[index show]\n  resources :widgets, only: %i[show]\nend\n",
        )
}

#[test]
fn not_found_answers_json_to_a_json_request() {
    app()
        .run_ruby(
            r##"
def probe(path, accept)
  env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => path.split("?", 2)[0], "QUERY_STRING" => path.split("?", 2)[1].to_s,
          "rack.input" => StringIO.new("") }
  env["HTTP_ACCEPT"] = accept unless accept.empty?
  status, headers, body = Main.run_rack(env)
  text = +""
  body.each { |part| text << part }
  [status, headers["content-type"] || headers["Content-Type"], text]
end

json = [404, "application/json; charset=UTF-8", %({"status":404,"error":"Not Found"})]
{
  ["/gizmos/1", "application/json"] => json,           # routed, action missing
  ["/nope", "application/json"] => json,               # no route
  ["/nope.json", ""] => json,                          # no route, extension
  ["/nope?format=json", ""] => json,                   # no route, format param
  ["/widgets/99", "application/json"] => json,         # RecordNotFound
  ["/widgets/99.json", ""] => json,                    # RecordNotFound, extension
  ["/widgets/99", "text/html;q=0.5, application/json"] => json,
}.each do |(path, accept), want|
  got = probe(path, accept)
  raise "#{path} (#{accept}): #{got.inspect}" unless got == want
end

# A browser's Accept (a list with */* in it) is not trusted, and an
# html request keeps the page.
[["/gizmos/1", "application/json, text/plain, */*"], ["/gizmos/1", "text/html"], ["/nope", ""]].each do |path, accept|
  status, type, text = probe(path, accept)
  raise "#{path} (#{accept}): #{status} #{type} #{text}" unless status == 404 && type.start_with?("text/html") && text.include?("404")
end
puts "ALL OK"
"##,
        )
        .assert_passes();
}
