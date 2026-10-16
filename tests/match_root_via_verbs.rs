//! `match "/", via: [...]` is one route per verb, beside a `root`.
//!
//! Rails' `Mapper#match` adds one route per `via:` entry, and routes
//! match on verb AND path, so `root` (GET /) and a `match "/"` for the
//! other verbs are separate routes. The dispatch table put every route
//! on path "/" into `RouteTable.root` and kept only the first: next to
//! a `root` the whole `match` was dropped, and without one only its
//! first verb survived.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app(routes: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/errors_controller.rb",
            "class ErrorsController < ApplicationController\n  def routing\n    render json: { error: \"No route matches [#{request.request_method}] #{request.path}\" }, status: :not_found\n  end\nend\n",
        )
        .write(
            "app/controllers/home_controller.rb",
            "class HomeController < ApplicationController\n  def index\n    render json: { home: true }\n  end\nend\n",
        )
        .write("config/routes.rb", routes)
}

const PROBE: &str = r##"
def probe(verb)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => verb, "PATH_INFO" => "/", "QUERY_STRING" => "",
                                         "CONTENT_TYPE" => "application/json", "rack.input" => StringIO.new(""))
  text = +""
  body.each { |part| text << part }
  [status, text]
end
"##;

#[test]
fn match_root_path_keeps_every_verb_beside_root() {
    app("Rails.application.routes.draw do\n  root to: \"home#index\"\n  match \"/\", to: \"errors#routing\", via: %i[post put patch delete]\nend\n")
        .run_ruby(&format!(
            "{PROBE}{}",
            r##"
got = probe("GET")
raise "GET #{got.inspect}" unless got == [200, %({"home":true})]
%w[POST PUT PATCH DELETE].each do |verb|
  got = probe(verb)
  raise "#{verb} #{got.inspect}" unless got == [404, %({"error":"No route matches [#{verb}] /"})]
end
puts "ALL OK"
"##
        ))
        .assert_passes();
}

#[test]
fn match_root_path_keeps_every_verb_without_root() {
    app("Rails.application.routes.draw do\n  match \"/\", to: \"errors#routing\", via: %i[post put]\nend\n")
        .run_ruby(&format!(
            "{PROBE}{}",
            r##"
%w[POST PUT].each do |verb|
  got = probe(verb)
  raise "#{verb} #{got.inspect}" unless got == [404, %({"error":"No route matches [#{verb}] /"})]
end
got = probe("DELETE")
raise "DELETE #{got.inspect}" unless got[0] == 404 && got[1].include?("404 Not Found")
puts "ALL OK"
"##
        ))
        .assert_passes();
}
