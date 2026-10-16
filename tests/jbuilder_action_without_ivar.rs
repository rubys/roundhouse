//! A jbuilder template whose action sets no ivar takes no parameter.
//!
//! An action view's render call passes exactly the template's ivar
//! closure, but the lowered template fell back to the name convention
//! when that closure was empty: `def index; end` beside `json.ok true`
//! emitted `def self.index_json(gizmos)` and a call with no argument,
//! an ArgumentError where Rails renders `{"ok":true}`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn jbuilder_action_without_ivar_renders() {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/gizmos_controller.rb",
            "class GizmosController < ApplicationController\n  def index; end\n\n  def show; end\nend\n",
        )
        .write("app/views/gizmos/index.json.jbuilder", "json.ok true\n")
        .write("app/views/gizmos/show.json.jbuilder", "json.name \"x\"\njson.id params[:id].to_i\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :gizmos, only: %i[index show]\nend\n")
        .run_ruby(
            r##"
def probe(path)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  text = +""
  body.each { |part| text << part }
  [status, text]
end
got = probe("/gizmos.json")
raise "index #{got.inspect}" unless got == [200, %({"ok":true})]
got = probe("/gizmos/7.json")
raise "show #{got.inspect}" unless got == [200, %({"name":"x","id":7})]
puts "ALL OK"
"##,
        )
        .assert_passes();
}
