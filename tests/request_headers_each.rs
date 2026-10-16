//! `request.headers` walks every env pair, as Rails' Enumerable
//! `ActionDispatch::Http::Headers` does.
//!
//! The runtime's `Headers` defined `[]`, `key?`, `include?` and `fetch`
//! only, so `request.headers.map { |key, _value| key }` (a request
//! logger, a header filter) passed `check`, emitted as written, and
//! raised `NoMethodError: undefined method 'map'` on the request. Rails
//! (actionpack 8.1) answers `{"id":"1","names":["HTTP_X_OTHER","HTTP_X_TRACE"]}`
//! for the request below. Without a block, `each` and `map` answer an
//! Enumerator over the `[key, value]` pairs, as Rails' do; they raised
//! LocalJumpError at `yield`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const WIDGETS_CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def show
    names = request.headers.map { |key, _value| key }.select { |key| key.start_with?("HTTP_X_") }
    render json: { id: params[:id], names: names.sort }
  end

  def blockless
    each_names = request.headers.each.map { |key, _value| key }.select { |key| key.start_with?("HTTP_X_") }
    map_names = request.headers.map.to_a.map { |pair| pair[0] }.select { |key| key.start_with?("HTTP_X_") }
    render json: { each: each_names.sort, map: map_names.sort }
  end

  def count
    seen = 0
    request.headers.each { |_key, value| seen += 1 if value == "2" }
    render json: { seen: seen }
  end
end
"#;

#[test]
fn request_headers_map_and_each_walk_the_env() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table :widgets do |t|\n    t.string :name\n  end\nend\n",
        )
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write("app/controllers/widgets_controller.rb", WIDGETS_CONTROLLER)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get '/widgets/count', to: 'widgets#count'\n  get '/widgets/blockless', to: 'widgets#blockless'\n  resources :widgets, only: [:show]\nend\n",
        )
        .run_ruby(
            r##"
def get(path)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "",
    "HTTP_X_TRACE" => "1", "HTTP_X_OTHER" => "2", "HTTP_ACCEPT" => "*/*", "rack.input" => StringIO.new(""))
  text = +""
  body.each { |part| text << part }
  raise "#{path}: status #{status} #{text}" unless status == 200
  text
end
text = get("/widgets/1")
raise "map: #{text}" unless text == %({"id":"1","names":["HTTP_X_OTHER","HTTP_X_TRACE"]})
text = get("/widgets/count")
raise "each: #{text}" unless text == %({"seen":1})
text = get("/widgets/blockless")
raise "blockless: #{text}" unless text == %({"each":["HTTP_X_OTHER","HTTP_X_TRACE"],"map":["HTTP_X_OTHER","HTTP_X_TRACE"]})
puts "ALL OK"
"##,
        )
        .assert_passes();
}
